use crate::cursors;
use crate::schema::aggregate_measurements;

use chrono::{DateTime, Utc};
use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::{Identifiable, QueryResult, Queryable};
use uuid::Uuid;
use validator::Validate;

#[rustfmt::skip]
use super::{
    Kit, KitId,
    KitConfiguration, KitConfigurationId,
    Peripheral, PeripheralId,
    QuantityType, QuantityTypeId,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Identifiable)]
#[table_name = "aggregate_measurements"]
pub struct AggregateMeasurementId(#[column_name = "id"] pub Uuid);

#[derive(Clone, Debug, PartialEq, Queryable, Identifiable, Associations, AsChangeset, Validate)]
#[belongs_to(parent = "Kit", foreign_key = "kit_id")]
#[belongs_to(parent = "KitId", foreign_key = "kit_id")]
#[belongs_to(parent = "KitConfiguration", foreign_key = "kit_configuration_id")]
#[belongs_to(parent = "KitConfigurationId", foreign_key = "kit_configuration_id")]
#[belongs_to(parent = "Peripheral", foreign_key = "peripheral_id")]
#[belongs_to(parent = "PeripheralId", foreign_key = "peripheral_id")]
#[belongs_to(parent = "QuantityType", foreign_key = "quantity_type_id")]
#[belongs_to(parent = "QuantityTypeId", foreign_key = "quantity_type_id")]
pub struct AggregateMeasurement {
    pub id: Uuid,
    pub peripheral_id: i32,
    pub kit_id: i32,
    pub kit_configuration_id: i32,
    pub quantity_type_id: i32,
    pub aggregate_type: String,
    pub value: f64,
    pub datetime_start: DateTime<Utc>,
    pub datetime_end: DateTime<Utc>,
}

impl AggregateMeasurement {
    pub fn by_id(
        conn: &PgConnection,
        aggregate_measurement_id: AggregateMeasurementId,
    ) -> QueryResult<Option<Self>> {
        aggregate_measurements::table
            .find(&aggregate_measurement_id.0)
            .first(conn)
            .optional()
    }

    pub fn page(
        conn: &PgConnection,
        kit_id: KitId,
        cursor: Option<cursors::AggregateMeasurements>,
    ) -> QueryResult<Vec<Self>> {
        let mut query = aggregate_measurements::table
            .filter(aggregate_measurements::columns::kit_id.eq(kit_id.0))
            .into_boxed();
        if let Some(cursors::AggregateMeasurements(datetime, id)) = cursor {
            query = query.filter(
                aggregate_measurements::columns::datetime_start
                    .lt(datetime)
                    .or(aggregate_measurements::columns::datetime_start
                        .eq(datetime)
                        .and(aggregate_measurements::columns::id.lt(id))),
            )
        }
        query
            .order((
                aggregate_measurements::dsl::datetime_start.desc(),
                aggregate_measurements::dsl::id.desc(),
            ))
            .limit(cursors::AggregateMeasurements::PER_PAGE as i64)
            .load(conn)
    }

    pub fn get_id(&self) -> AggregateMeasurementId {
        AggregateMeasurementId(self.id)
    }
}
