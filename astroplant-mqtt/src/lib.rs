use log::{debug, trace, warn};

use capnp::serialize_packed;
use futures::channel::oneshot;
use futures::task::SpawnExt;
use futures::FutureExt;
use rumqtt::{MqttClient, MqttOptions, Notification, QoS, SecurityOptions};
use std::future::Future;

mod server_rpc;
pub use server_rpc::{ServerRpcRequest, ServerRpcResponder};

mod kit_rpc;
pub use kit_rpc::{KitRpc, KitsRpc};

const MQTT_API_MESSAGE_BUFFER: usize = 128;

pub mod astroplant_capnp {
    include!(concat!(env!("OUT_DIR"), "/proto/astroplant_capnp.rs"));
}

#[derive(Debug)]
pub struct RawMeasurement {
    pub kit_serial: String,
    pub datetime: u64,
    pub peripheral: i32,
    pub physical_quantity: String,
    pub physical_unit: String,
    pub value: f64,
}

#[derive(Debug)]
pub struct AggregateMeasurement {
    pub kit_serial: String,
    pub datetime_start: u64,
    pub datetime_end: u64,
    pub peripheral: String,
    pub physical_quantity: String,
    pub physical_unit: String,
    pub value: f64,
}

#[derive(Debug)]
pub enum MqttApiMessage {
    RawMeasurement(RawMeasurement),
    AggregateMeasurement(AggregateMeasurement),
    ServerRpcRequest(ServerRpcRequest),
}

enum MqttMessage {
    Api(MqttApiMessage, Option<ServerRpcResponder<'static>>),
    KitRpcResponse(String, Vec<u8>),
}

fn establish_subscriptions(mqtt_client: &mut MqttClient) {
    if let Err(err) = mqtt_client.subscribe("kit/#", QoS::AtLeastOnce) {
        warn!("error occurred while subscribing {:?}", err);
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidTopic,
    Capnp(capnp::Error),
    // The response is the error to send over MQTT. This is hacky.
    ServerRpcError(server_rpc::ServerRpcResponse),
}

fn parse_raw_measurement(kit_serial: String, mut payload: &[u8]) -> Result<MqttApiMessage, Error> {
    let message_reader =
        serialize_packed::read_message(&mut payload, capnp::message::ReaderOptions::default())
            .unwrap();
    let raw_measurement = message_reader
        .get_root::<astroplant_capnp::raw_measurement::Reader>()
        .map_err(Error::Capnp)?;

    let measurement = RawMeasurement {
        kit_serial: kit_serial,
        datetime: raw_measurement.get_datetime(),
        peripheral: raw_measurement.get_peripheral(),
        physical_quantity: raw_measurement
            .get_physical_quantity()
            .map_err(Error::Capnp)?
            .to_owned(),
        physical_unit: raw_measurement
            .get_physical_unit()
            .map_err(Error::Capnp)?
            .to_owned(),
        value: raw_measurement.get_value(),
    };

    Ok(MqttApiMessage::RawMeasurement(measurement))
}

fn parse_aggregate_measurement(
    kit_serial: String,
    payload: &[u8],
) -> Result<MqttApiMessage, Error> {
    unimplemented!()
}

fn proxy<'a>(
    rpc_bytes: ServerRpcResponder<'a>,
    mut mqtt_client: MqttClient,
) -> impl Future<Output = ()> + 'a {
    rpc_bytes.map(move |response| {
        if let Some(server_rpc::ServerRpcResponse { kit_serial, bytes }) = response {
            if let Err(err) = mqtt_client.publish(
                format!("kit/{}/server-rpc/response", kit_serial),
                QoS::AtLeastOnce,
                false,
                bytes,
            ) {
                debug!("error occurred when sending an RPC response: {:?}", err);
            }
        }

        ()
    })
}

struct Handler {
    server_rpc_handler: server_rpc::ServerRpcHandler,
}

impl Handler {
    pub fn new() -> Self {
        Self {
            server_rpc_handler: server_rpc::ServerRpcHandler::new(),
        }
    }

    fn handle_mqtt_publish(&mut self, msg: rumqtt::Publish) -> Result<MqttMessage, Error> {
        trace!("received an MQTT message on topic {}", msg.topic_name);
        let mut topic_parts = msg.topic_name.split("/");
        if topic_parts.next() != Some("kit") {
            return Err(Error::InvalidTopic);
        }

        let kit_serial: String = match topic_parts.next() {
            Some(serial) => serial.to_owned(),
            None => return Err(Error::InvalidTopic),
        };

        match topic_parts.next() {
            Some("measurement") => match topic_parts.next() {
                Some("raw") => Ok(MqttMessage::Api(
                    parse_raw_measurement(kit_serial, &msg.payload)?,
                    None,
                )),
                Some("aggregate") => Ok(MqttMessage::Api(
                    parse_aggregate_measurement(kit_serial, &msg.payload)?,
                    None,
                )),
                _ => Err(Error::InvalidTopic),
            },
            Some("server-rpc") => match topic_parts.next() {
                Some("request") => self
                    .server_rpc_handler
                    .handle_rpc_request(kit_serial, &msg.payload)
                    .map(|(request, responder)| {
                        (MqttMessage::Api(MqttApiMessage::ServerRpcRequest(request), responder))
                    }),
                _ => Err(Error::InvalidTopic),
            },
            Some("kit-rpc") => match topic_parts.next() {
                Some("response") => Ok(MqttMessage::KitRpcResponse(
                    kit_serial,
                    msg.payload.to_vec(),
                )),
                _ => Err(Error::InvalidTopic),
            },
            _ => Err(Error::InvalidTopic),
        }
    }

    fn runner(
        &mut self,
        mut thread_pool: futures::executor::ThreadPool,
        mut mqtt_client: MqttClient,
        notifications: crossbeam::channel::Receiver<Notification>,
        kit_rpc_mqtt_message_handler: crossbeam::channel::Sender<(String, Vec<u8>)>,
        mqtt_api_sender: crossbeam::channel::Sender<MqttApiMessage>,
    ) {
        establish_subscriptions(&mut mqtt_client);

        // Receive incoming notifications.
        for notification in notifications {
            trace!("Received MQTT notification: {:?}", notification);
            match notification {
                Notification::Reconnection => {
                    establish_subscriptions(&mut mqtt_client);
                }
                Notification::Publish(publish) => {
                    match self.handle_mqtt_publish(publish) {
                        Ok(MqttMessage::Api(msg, responder)) => {
                            if let Some(responder) = responder {
                                thread_pool
                                    .spawn(proxy(responder, mqtt_client.clone()))
                                    .expect("Could not spawn on threadpool");
                            }
                            if mqtt_api_sender.send(msg).is_err() {
                                // Receiver not keeping up. Disconnect.
                                break;
                            }
                        }
                        Ok(MqttMessage::KitRpcResponse(kit_serial, payload)) => {
                            if kit_rpc_mqtt_message_handler
                                .send((kit_serial, payload))
                                .is_err()
                            {
                                // Kit RPC handler not keeping up. Disconnect.
                                break;
                            }
                        }
                        Err(Error::ServerRpcError(response)) => {
                            let _ = mqtt_client.publish(
                                format!("kit/{}/server-rpc/response", response.kit_serial),
                                QoS::AtLeastOnce,
                                false,
                                response.bytes,
                            );
                        }
                        Err(err) => {
                            debug!("Error parsing MQTT message: {:?}", err);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn run() -> (crossbeam::channel::Receiver<MqttApiMessage>, KitsRpc) {
    let (mqtt_api_sender, mqtt_api_receiver) = crossbeam::channel::bounded(MQTT_API_MESSAGE_BUFFER);

    let thread_pool = futures::executor::ThreadPoolBuilder::new()
        .pool_size(1)
        .name_prefix("responder-proxy-pool")
        .create()
        .expect("Could not build thread pool");
    let (thread_pool_handle_sender, thread_pool_handle_receiver) = oneshot::channel::<()>();

    {
        let mut thread_pool = thread_pool.clone();
        std::thread::spawn(move || thread_pool.run(thread_pool_handle_receiver));
    }

    let mqtt_options =
        MqttOptions::new("astroplant-api-connector", "mqtt.ops", 1883).set_security_opts(
            SecurityOptions::UsernamePassword("server".to_owned(), "abcdef".to_owned()),
        );
    let (mqtt_client, notifications) = MqttClient::start(mqtt_options).unwrap();

    let kit_rpc_runner = kit_rpc::kit_rpc_runner(mqtt_client.clone(), thread_pool.clone());

    {
        let mut handler = Handler::new();
        let kit_rpc_mqtt_message_handler = kit_rpc_runner.mqtt_message_handler;
        std::thread::spawn(move || {
            handler.runner(
                thread_pool,
                mqtt_client,
                notifications,
                kit_rpc_mqtt_message_handler,
                mqtt_api_sender,
            );
            thread_pool_handle_sender.send(()).unwrap()
        });
    }

    (mqtt_api_receiver, kit_rpc_runner.kits_rpc)
}
