use futures::future::FutureExt;
use serde::Deserialize;
use validator::Validate;
use warp::{filters::BoxedFilter, path, Filter, Rejection};

use crate::database::PgPool;
use crate::problem::{self, AppResult, Problem};
use crate::response::{Response, ResponseBuilder};
use crate::{authentication, helpers, models, views};

pub fn router(pg: PgPool) -> BoxedFilter<(AppResult<Response>,)> {
    //impl Filter<Extract = (Response,), Error = Rejection> + Clone {
    trace!("Setting up users router.");

    //TODO implement deleting users.
    (user_by_username(pg.clone()))
        .or(patch_user(pg.clone()))
        .unify()
        .or(list_kit_memberships(pg.clone()))
        .unify()
        .or(create_user(pg.clone()))
        .unify()
        .boxed()
}

// Handles the `GET /users/{username}` route.
pub fn user_by_username(
    pg: PgPool,
) -> impl Filter<Extract = (AppResult<Response>,), Error = Rejection> + Clone {
    async fn implementation(
        pg: PgPool,
        object_username: String,
        actor_user_id: Option<models::UserId>,
    ) -> AppResult<Response> {
        let (_target_user, object_user) = helpers::fut_user_permission_or_forbidden(
            pg,
            actor_user_id,
            object_username,
            crate::authorization::UserAction::View,
        )
        .await?;

        Ok(ResponseBuilder::ok().body(views::User::from(object_user)))
    }

    warp::get()
        .and(path!(String))
        .and(authentication::option_by_token())
        .and_then(
            move |object_username: String, actor_user_id: Option<models::UserId>| {
                implementation(pg.clone(), object_username, actor_user_id).never_error()
            },
        )
}

// Handles the `PATCH /users/{username}` route.
pub fn patch_user(
    pg: PgPool,
) -> impl Filter<Extract = (AppResult<Response>,), Error = Rejection> + Clone {
    //TODO implement password patching.
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "camelCase")]
    struct UserPatch {
        display_name: Option<String>,
        email_address: Option<String>,
        use_email_address_for_gravatar: Option<bool>,
    }

    async fn implementation(
        pg: PgPool,
        object_username: String,
        actor_user_id: Option<models::UserId>,
        user_patch: UserPatch,
    ) -> AppResult<Response> {
        let (_actor_user, user) = helpers::fut_user_permission_or_forbidden(
            pg.clone(),
            actor_user_id,
            object_username,
            crate::authorization::UserAction::EditDetails,
        )
        .await?;

        let update_user = models::UpdateUser {
            id: user.id,
            display_name: user_patch.display_name,
            password_hash: None,
            email_address: user_patch.email_address,
            use_email_address_for_gravatar: user_patch.use_email_address_for_gravatar,
        };

        let conn = pg.get().await?;
        let patched_user = helpers::threadpool_result(move || {
            if let Some(email_address) = &update_user.email_address {
                if let Some(user_by_email_address) =
                    models::User::by_email_address(&conn, email_address)?
                {
                    if user_by_email_address.id != user.id {
                        let mut invalid_parameters = problem::InvalidParameters::new();
                        invalid_parameters.add(
                            "emailAddress",
                            problem::InvalidParameterReason::AlreadyExists,
                        );
                        return Err(problem::Problem::InvalidParameters { invalid_parameters });
                    }
                }
            }

            if let Err(validation_errors) = update_user.validate() {
                let invalid_parameters = problem::InvalidParameters::from(validation_errors);
                return Err(problem::Problem::InvalidParameters { invalid_parameters });
            }

            Ok::<_, Problem>(update_user.update(&conn)?)
        })
        .await?;

        Ok(ResponseBuilder::ok().body(views::User::from(patched_user)))
    }

    warp::patch()
        .and(path!(String))
        .and(authentication::option_by_token())
        .and(crate::helpers::deserialize())
        .and_then(
            move |object_username: String,
                  actor_user_id: Option<models::UserId>,
                  user_patch: UserPatch| {
                implementation(pg.clone(), object_username, actor_user_id, user_patch).never_error()
            },
        )
}

// Handles the `GET /users/{username}/kit-memberships` route.
pub fn list_kit_memberships(
    pg: PgPool,
) -> impl Filter<Extract = (AppResult<Response>,), Error = Rejection> + Clone {
    async fn implementation(
        pg: PgPool,
        object_username: String,
        actor_user_id: Option<models::UserId>,
    ) -> AppResult<Response> {
        let (_actor_user, user) = helpers::fut_user_permission_or_forbidden(
            pg.clone(),
            actor_user_id,
            object_username,
            crate::authorization::UserAction::ListKitMemberships,
        )
        .await?;

        let username = user.username.clone();
        let conn = pg.get().await?;
        let kit_memberships = helpers::threadpool_result(move || {
            models::KitMembership::memberships_with_kit_of_user_id(&conn, user.get_id())
        })
        .await?;
        let v: Vec<views::KitMembership<String, views::Kit>> = kit_memberships
            .into_iter()
            .map(|(kit, membership)| {
                views::KitMembership::from(membership)
                    .with_kit(views::Kit::from(kit))
                    .with_user(username.clone())
            })
            .collect();
        Ok(ResponseBuilder::ok().body(v))
    }

    warp::get()
        .and(path!(String / "kit-memberships"))
        .and(authentication::option_by_token())
        .and_then(
            move |object_username: String, actor_user_id: Option<models::UserId>| {
                implementation(pg.clone(), object_username, actor_user_id).never_error()
            },
        )
}

pub fn create_user(
    pg: PgPool,
) -> impl Filter<Extract = (AppResult<Response>,), Error = Rejection> + Clone {
    use diesel::Connection;

    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "camelCase")]
    struct User {
        username: String,
        password: String,
        email_address: String,
    }

    async fn implementation(pg: PgPool, user: User) -> AppResult<Response> {
        let username = user.username.clone();
        trace!("Got request to create user with username: {}", username);

        let conn = pg.get().await?;
        helpers::threadpool(move || {
                conn.transaction(|| {
                    let user_by_username = models::User::by_username(&conn, &user.username)?;
                    let user_by_email_address = models::User::by_email_address(&conn, &user.email_address)?;

                    let hash = astroplant_auth::hash::hash_user_password(&user.password);
                    let new_user = models::NewUser::new(user.username, hash, user.email_address);

                    if let Err(validation_errors) = new_user.validate() {
                        let invalid_parameters = problem::InvalidParameters::from(validation_errors);
                        return Err(problem::Problem::InvalidParameters { invalid_parameters })
                    }

                    let mut invalid_parameters = problem::InvalidParameters::new();
                    if user_by_username.is_some() {
                        invalid_parameters.add("username", problem::InvalidParameterReason::AlreadyExists)
                    }

                    if user_by_email_address.is_some() {
                        invalid_parameters.add("emailAddress", problem::InvalidParameterReason::AlreadyExists)
                    }

                    if !invalid_parameters.is_empty() {
                        return Err(problem::Problem::InvalidParameters { invalid_parameters })
                    }

                    let created_user = new_user.create(&conn)?;
                    if created_user.is_some() {
                        info!("Created user {:?}", username);

                        Ok(ResponseBuilder::created().empty())
                    } else {
                        warn!("Unexpected database error: username and email address don't exist, yet user could not be created: {:?}", username);
                        Err(problem::INTERNAL_SERVER_ERROR)
                    }
                })
            }).await
    }

    warp::post()
        .and(warp::path::end())
        .and(crate::helpers::deserialize())
        .and_then(move |user: User| implementation(pg.clone(), user).never_error())
}
