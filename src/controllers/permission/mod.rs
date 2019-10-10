use crate::problem::{INTERNAL_SERVER_ERROR, NOT_FOUND};

use serde::{Deserialize, Serialize};
use warp::{filters::BoxedFilter, path, Filter, Rejection};

use crate::authentication;
use crate::response::{Response, ResponseBuilder};
use crate::views;

pub fn router(pg: BoxedFilter<(crate::PgPooled,)>) -> BoxedFilter<(Response,)> {
    //impl Filter<Extract = (Response,), Error = Rejection> + Clone {
    trace!("Setting up permissions router.");

    warp::path::end()
        .and(user_kit_permissions(pg.clone().boxed()))
        .boxed()
}

/// Handles the `GET /permissions/?kitSerial={kitSerial}` route.
pub fn user_kit_permissions(
    pg: BoxedFilter<(crate::PgPooled,)>,
) -> impl Filter<Extract = (Response,), Error = Rejection> + Clone {
    use crate::PgPooled;
    use crate::{helpers, models};
    use diesel::Connection;

    use futures::future::{self, Future};

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct KitSerial {
        kit_serial: String,
    }

    authentication::option_by_token()
        .and(warp::query::query::<KitSerial>())
        .and(pg)
        .and_then(
            |user_id: Option<models::UserId>, kit_serial: KitSerial, conn: PgPooled| {
                helpers::threadpool_diesel_ok(move || {
                    conn.transaction(|| {
                        let user = if let Some(user_id) = user_id {
                            models::User::by_id(&conn, user_id)?
                        } else {
                            None
                        };

                        let kit = models::Kit::by_serial(&conn, kit_serial.kit_serial)?;
                        if kit.is_none() {
                            return Ok(None);
                        }
                        let kit = kit.unwrap();

                        let membership = if let Some(user_id) = user_id {
                            models::KitMembership::by_user_id_and_kit_id(
                                &conn,
                                user_id,
                                kit.get_id(),
                            )?
                        } else {
                            None
                        };

                        Ok(Some((user, membership, kit)))
                    })
                })
                .then(move |result| match result {
                    Ok(None) => Err(warp::reject::custom(NOT_FOUND)),
                    Ok(Some((user, membership, kit))) => {
                        use crate::authorization::KitAction;
                        use strum::IntoEnumIterator;

                        let permissions: Vec<KitAction> = KitAction::iter()
                            .filter(|action| action.permission(&user, &membership, &kit))
                            .collect();

                        Ok(ResponseBuilder::ok().body(permissions))
                    }
                    Err(e) => Err(e),
                })
            },
        )
}