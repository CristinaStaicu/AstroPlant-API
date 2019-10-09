use crate::problem::{Problem, FORBIDDEN, INTERNAL_SERVER_ERROR, NOT_FOUND};

use bytes::Buf;
use futures::future::{self, poll_fn, Future};
use serde::de::DeserializeOwned;
use warp::{Filter, Rejection};

/// Run a function on a threadpool, returning a future resolving when the function completes.
pub fn fut_threadpool<F, T>(f: F) -> impl Future<Item = T, Error = tokio_threadpool::BlockingError>
where
    F: FnOnce() -> T,
{
    let mut f_only_once = Some(f);
    poll_fn(move || {
        tokio_threadpool::blocking(|| {
            let f = f_only_once.take().unwrap();
            f()
        })
    })
}

/// Run a function on a threadpool, returning a future resolving when the function completes.
/// Any (unexpected!) threadpool error is turned into a Warp rejection, wrapping the Internal Server
/// Error problem.
pub fn threadpool<F, T>(f: F) -> impl Future<Item = T, Error = Rejection>
where
    F: FnOnce() -> T,
{
    fut_threadpool(f).map_err(|_| warp::reject::custom(INTERNAL_SERVER_ERROR))
}

/// Runs a function on a threadpool, ignoring a potential Diesel error inside the threadpool.
/// This error is turned into an internal server error (as Diesel errors are unexpected, and
/// indicative of erroneous queries).
pub fn threadpool_diesel_ok<F, T>(f: F) -> impl Future<Item = T, Error = Rejection>
where
    F: FnOnce() -> Result<T, diesel::result::Error>,
{
    threadpool(f).and_then(|result| match result {
        Ok(v) => future::ok(v),
        Err(_) => future::err(warp::reject::custom(INTERNAL_SERVER_ERROR)),
    })
}

/// Flatten a nested result with equal error types to a single result.
pub fn flatten_result<T, E>(nested: Result<Result<T, E>, E>) -> Result<T, E> {
    match nested {
        Err(e) => Err(e),
        Ok(v) => v,
    }
}

/// Create a filter to deserialize a request.
pub fn deserialize<T>() -> impl Filter<Extract = (T,), Error = Rejection> + Copy
where
    T: DeserializeOwned + Send,
{
    // TODO: Also allow e.g. XML, basing the attempted deserialization on the Content-Type header.
    // Default to JSON.

    // Allow a request of at most 64 KiB
    const CONTENT_LENGTH_LIMIT: u64 = 1024 * 64;

    warp::body::content_length_limit(CONTENT_LENGTH_LIMIT)
        .or_else(|_| {
            Err(warp::reject::custom(Problem::PayloadTooLarge {
                limit: CONTENT_LENGTH_LIMIT,
            }))
        })
        .and(warp::body::concat())
        .and_then(|body_buffer: warp::body::FullBody| {
            let body: Vec<u8> = body_buffer.collect();

            serde_json::from_slice(&body).map_err(|err| {
                debug!("Request JSON deserialize error: {}", err);
                warp::reject::custom(Problem::InvalidJson {
                    category: (&err).into(),
                })
            })
        })
}

/// Create a filter to get a PostgreSQL connection from a PostgreSQL connection pool.
pub fn pg(
    pg_pool: crate::PgPool,
) -> impl Filter<Extract = (crate::PgPooled,), Error = Rejection> + Clone {
    warp::any()
        .map(move || pg_pool.clone())
        .and_then(|pg_pool: crate::PgPool| match pg_pool.get() {
            Ok(pg_pooled) => Ok(pg_pooled),
            Err(_) => Err(warp::reject::custom(INTERNAL_SERVER_ERROR)),
        })
}

pub fn ok_or_internal_error<T, E>(r: Result<T, E>) -> Result<T, Rejection> {
    match r {
        Ok(value) => Ok(value),
        Err(_) => Err(warp::reject::custom(INTERNAL_SERVER_ERROR)),
    }
}

pub fn some_or_internal_error<T>(r: Option<T>) -> Result<T, Rejection> {
    match r {
        Some(value) => Ok(value),
        None => Err(warp::reject::custom(INTERNAL_SERVER_ERROR)),
    }
}

pub fn some_or_not_found<T>(r: Option<T>) -> Result<T, Rejection> {
    match r {
        Some(value) => Ok(value),
        None => Err(warp::reject::custom(NOT_FOUND)),
    }
}

/**
 * Ensure the user has permission to perform the action on the kit.
 * Rejects the request otherwise.
 */
pub fn permission_or_forbidden(
    user: &Option<crate::models::User>,
    kit_membership: &Option<crate::models::KitMembership>,
    kit: &crate::models::Kit,
    action: crate::authorization::KitAction,
) -> Result<(), Rejection> {
    if action.permission(user, kit_membership, kit) {
        Ok(())
    } else {
        Err(warp::reject::custom(FORBIDDEN))
    }
}

/**
 * Ensure the user has permission to perform the action on the kit.
 * Rejects the request with FORBIDDEN otherwise.
 *
 * Fetches the required information from the database.
 * If the user id is given but the user cannot be found or if the kit cannot be found with the
 * given serial, the request is rejected with NOT_FOUND. If the request is *not* rejected, this
 * returns the fetched user, membership and kit.
 */
pub fn fut_permission_or_forbidden<'a>(
    conn: crate::PgPooled,
    user_id: Option<crate::models::UserId>,
    kit_serial: String,
    action: crate::authorization::KitAction,
) -> impl Future<
    Item = (
        Option<crate::models::User>,
        Option<crate::models::KitMembership>,
        crate::models::Kit,
    ),
    Error = Rejection,
> + 'a {
    use diesel::Connection;

    threadpool_diesel_ok(move || {
        conn.transaction(|| {
            let user = if let Some(user_id) = user_id {
                match crate::models::User::by_id(&conn, user_id)? {
                    Some(user) => Some(user),
                    // User id set but user is not found.
                    None => return Ok(None),
                }
            } else {
                None
            };

            let kit = match crate::models::Kit::by_serial(&conn, kit_serial)? {
                Some(kit) => kit,
                None => return Ok(None),
            };

            let membership = if let Some(user_id) = user_id {
                crate::models::KitMembership::by_user_id_and_kit_id(&conn, user_id, kit.get_id())?
            } else {
                None
            };

            Ok(Some((user, membership, kit)))
        })
    })
    .and_then(some_or_not_found)
    .and_then(move |(user, membership, kit)| {
        permission_or_forbidden(&user, &membership, &kit, action).map(|_| (user, membership, kit))
    })
}

#[cfg(test)]
mod test {
    #[test]
    fn flatten() {
        assert_eq!(
            super::flatten_result::<_, std::convert::Infallible>(Ok(Ok(42))),
            Ok(42)
        );
        assert_eq!(
            super::flatten_result::<std::convert::Infallible, _>(Ok(Err("oops"))),
            Err("oops")
        );
        assert_eq!(
            super::flatten_result::<std::convert::Infallible, _>(Err("oops")),
            Err("oops")
        );
    }

    #[test]
    fn deserialize_json() {
        use serde::Deserialize;

        #[derive(Debug, Deserialize)]
        struct TestStruct {
            value: u64,
        }

        let value: TestStruct = warp::test::request()
            .header("Accept", "application/json")
            .header("Content-Length", 12)
            .body(r#"{"value":42}"#)
            .filter(&super::deserialize())
            .unwrap();
        assert_eq!(value.value, 42);

        // Should reject requests with too large Content-Length.
        let req = warp::test::request()
            .header("Accept", "application/json")
            .header("Content-Length", 99_999_999)
            .body(r#"{"value":42}"#);
        assert!(!req.matches(&super::deserialize::<TestStruct>()));
    }
}
