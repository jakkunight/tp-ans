//! # JSON REST API (`/api/v1/...`).
//!
//! Request/response shapes live in [`dto`]; every DTO derives [`Clone`],
//! [`Serialize`](serde::Serialize) and [`Deserialize`](serde::Deserialize).
//! All SQL and transactions live in [`crate::db`]: handlers validate request
//! shapes, enforce JWT actor scoping and map [`crate::db::DbError`]
//! to HTTP. Traces carry numeric ids/counts only — credentials (CI, names,
//! RUC, PSKs, OTP codes) and tokens are never logged.
//!
//! Failures share one shape: [`ApiError`], i.e. the HTTP status plus a JSON
//! [`dto::ErrorResponse`] body (see each handler's docs for
//! the codes it can produce).
//!
//! Authentication:
//!
//! * Partners log in with RUC + pre-shared key
//!   (`POST /api/v1/partners/login`, public) and receive an 8-hour JWT.
//! * Clients log in with a one-time code sent over SMS/email
//!   (`POST /api/v1/clients/otp/request` then
//!   `POST /api/v1/clients/otp/verify`, both public) and receive a 1-hour JWT.
//! * Every other route sits behind an auth + scope middleware layer:
//!   [`crate::jwt::client_auth`] for customer routes (the token
//!   must be a client JWT whose id matches the `{client_id}` path segment)
//!   and [`crate::jwt::partner_auth`] for pharmacy routes (the
//!   token must be a partner JWT scoped to the targeted partner). Handlers
//!   never see a mismatched actor.
//!
//! | Method & path | Handler | Auth |
//! |---|---|---|
//! | `POST /api/v1/partners/login` | [`partner_login`] | public |
//! | `POST /api/v1/clients/otp/request` | [`request_client_otp`] | public |
//! | `POST /api/v1/clients/otp/verify` | [`verify_client_otp`] | public |
//! | `POST /api/v1/partners/tickets` | [`add_ticket`] | `partner_auth` layer |
//! | `DELETE /api/v1/partners/tickets` | [`delete_ticket`] | `partner_auth` layer |
//! | `GET /api/v1/clients/{client_id}` | [`get_client_data`] | `client_auth` layer |
//! | `POST /api/v1/clients/{client_id}/redeem_points` | [`redeem_points`] | `client_auth` layer |
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    routing::{delete, get, post},
};
use chrono::{Duration, Utc};

use crate::{
    AppState,
    db::{self, DbError, NewClient, NewTicket, RedeemLine, TicketLine},
    jwt::{ClientClaims, PartnerClaims, client_auth, create_token, partner_auth},
    otp::{self, OtpChannel, OtpError},
};

use self::dto::{
    ClientDataResponse, ClientOtpRequest, ClientOtpResponse, ClientOtpVerify, CreateTicketRequest,
    CreateTicketResponse, DeleteTicketRequest, DeleteTicketResponse, ErrorResponse, LoginResponse,
    PartnerLoginRequest, RedeemPointsRequest, RedeemPointsResponse,
};

pub mod dto;

// NOTE on logging: traces below deliberately carry only operational,
// non-sensitive fields (numeric DB ids, counts, point totals). Credentials
// (CI, names, RUC, PSKs, OTP codes) and JWT tokens are NEVER logged.

/// Error shape returned by every REST handler: the HTTP status plus a JSON
/// [`ErrorResponse`] body. Axum turns it into the response automatically.
pub type ApiError = (StatusCode, Json<ErrorResponse>);

/// Builds an [`ApiError`] from a status code and a machine-readable message
/// (see each handler's docs for the codes it can produce).
fn api_error(code: StatusCode, message: &str) -> ApiError {
    (code, Json(ErrorResponse::new(message)))
}

/// Maps a [`DbError`] to its [`ApiError`], logging transport failures.
fn db_api_error(who: &str, e: DbError) -> ApiError {
    if let DbError::Db(sqlx_err) = &e {
        tracing::error!("{who}: db error: {sqlx_err:?}");
    }
    let (code, body) = e.status_and_body();
    (code, Json(body))
}

/// Maps an [`OtpError`] to its [`ApiError`].
fn otp_api_error(who: &str, e: OtpError) -> ApiError {
    match e {
        OtpError::UnknownChannel => api_error(StatusCode::BAD_REQUEST, "unknown channel"),
        OtpError::ChannelUnavailable { channel } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorResponse::with_message(
                "channel unavailable",
                format!("client has no {} destination", channel.name()),
            )),
        ),
        OtpError::TooManyRequests => {
            tracing::warn!("{who}: rejected (otp resend throttled)");
            api_error(StatusCode::TOO_MANY_REQUESTS, "too many requests")
        }
        OtpError::Locked { retry_after } => {
            tracing::warn!("{who}: rejected (otp locked out)");
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse::with_message(
                    "locked out",
                    format!("too many wrong codes; retry after {retry_after}"),
                )),
            )
        }
    }
}

/// Builds the `/api/v1/...` router: three public login routes plus the
/// ticket/client routes, each guarded by its auth + scope middleware layer
/// ([`crate::jwt::client_auth`] or
/// [`crate::jwt::partner_auth`]).
///
/// # Errors
///
/// Currently infallible; returns [`anyhow::Error`] to allow future fallible setup.
pub fn create_api(state: &Arc<AppState>) -> anyhow::Result<Router<Arc<AppState>>> {
    let public_api = Router::new()
        .route("/api/v1/partners/login", post(partner_login))
        .route("/api/v1/clients/otp/request", post(request_client_otp))
        .route("/api/v1/clients/otp/verify", post(verify_client_otp));
    let partner_api = Router::new()
        .route("/api/v1/partners/tickets", post(add_ticket))
        .route("/api/v1/partners/tickets", delete(delete_ticket))
        .route_layer(middleware::from_fn_with_state(state.clone(), partner_auth));
    let client_api = Router::new()
        .route("/api/v1/clients/{client_id}", get(get_client_data))
        .route(
            "/api/v1/clients/{client_id}/redeem_points",
            post(redeem_points),
        )
        .route_layer(middleware::from_fn(client_auth));
    let router = Router::new()
        .merge(public_api)
        .merge(partner_api)
        .merge(client_api);
    Ok(router)
}

/// Validates buyer fields shared by ticket creation.
///
/// Returns trimmed `(first_name, last_name)`.
///
/// # Errors
///
/// Returns `400` for invalid CI, names, verification digit or contacts.
fn validate_buyer(
    ci: i32,
    verification_digit: Option<i32>,
    first_name: &str,
    last_name: &str,
    phone_number: Option<&str>,
    email: Option<&str>,
) -> Result<(String, String), ApiError> {
    // Values are not logged: CI, names and contacts are sensitive.
    if ci < 0 {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid buyer ci"));
    }
    let first_name = first_name.trim().to_string();
    let last_name = last_name.trim().to_string();
    if first_name.is_empty() || last_name.is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "missing buyer name"));
    }
    if first_name.len() > 32 || last_name.len() > 32 {
        return Err(api_error(StatusCode::BAD_REQUEST, "buyer name too long"));
    }
    if let Some(vd) = verification_digit
        && !(0..=9).contains(&vd)
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid verification digit",
        ));
    }
    if let Some(phone) = phone_number {
        // `varchar(13)`: e.g. `+595981123456`.
        if phone.trim().is_empty() || phone.len() > 13 {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid buyer contact"));
        }
    }
    if let Some(email) = email
        && (email.trim().is_empty() || email.len() > 128 || !email.contains('@'))
    {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid buyer contact"));
    }
    Ok((first_name, last_name))
}

/// `POST /api/v1/partners/login` — authenticates a pharmacy and mints its JWT.
///
/// Verifies `ruc` + `psk` against `partners` (Argon2 `psk_hash`; the partner
/// must be active) and returns a [`LoginResponse`] expiring after 8 hours.
/// The PSK itself is never echoed into the token.
///
/// # Status codes
///
/// * `200` with the token on success.
/// * `401` for an unknown RUC or wrong PSK (deliberately indistinguishable).
/// * `403` for an inactive partner.
/// * `500` on DB or token-signing failures.
#[tracing::instrument(skip(state, credentials), name = "partner_login")]
pub async fn partner_login(
    State(state): State<Arc<AppState>>,
    Json(credentials): Json<PartnerLoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    tracing::debug!("partner login attempt");
    // RUC/PSK values are never logged.
    if credentials.ruc.trim().is_empty() || credentials.psk.is_empty() {
        tracing::warn!("partner_login: rejected (missing credentials)");
        return Err(api_error(StatusCode::UNAUTHORIZED, "unknown credentials"));
    }
    let partner = db::authenticate_partner(&state.db, credentials.ruc.trim(), &credentials.psk)
        .await
        .map_err(|e| db_api_error("partner_login", e))?;

    let claims = PartnerClaims::new(partner.id, Utc::now() + Duration::hours(8));
    let token = create_token(claims.into()).map_err(|e| {
        tracing::error!("partner_login: token error: {e:?}");
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "token error")
    })?;
    tracing::info!(partner_id = partner.id, "partner_login: succeeded");
    Ok(Json(LoginResponse::bearer(token)))
}

/// `POST /api/v1/clients/otp/request` — sends the client login code.
///
/// Looks the client up by `ci`, picks the SMS/email channel and dispatches a
/// 6-digit code valid for [`otp::OTP_TTL_SECS`] seconds. Sends are throttled
/// per client.
///
/// # Status codes
///
/// * `200` with [`ClientOtpResponse`] on success.
/// * `400` for an invalid CI or unknown channel name.
/// * `404` for an unknown CI.
/// * `422` when the requested channel has no destination on the client.
/// * `429` when throttled or locked out.
/// * `500` on DB or dispatch failures.
#[tracing::instrument(skip(state, request), name = "request_client_otp")]
pub async fn request_client_otp(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ClientOtpRequest>,
) -> Result<Json<ClientOtpResponse>, ApiError> {
    tracing::debug!("otp request received");
    if request.ci < 0 {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid ci"));
    }
    let requested = request
        .channel
        .as_deref()
        .map(OtpChannel::parse)
        .transpose()
        .map_err(|e| otp_api_error("request_client_otp", e))?;

    let client = db::client_by_ci(&state.db, request.ci)
        .await
        .map_err(|e| db_api_error("request_client_otp", e))?;
    let (channel, destination) =
        otp::channel_for(&client, requested).map_err(|e| otp_api_error("request_client_otp", e))?;

    state
        .otp_guard()
        .check_and_record_send(client.ci)
        .map_err(|e| otp_api_error("request_client_otp", e))?;

    let secret = otp::otp_secret().map_err(|e| {
        tracing::error!("request_client_otp: secret error: {e:?}");
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "otp error")
    })?;
    // Window math lives in `otp`; this only needs "now" for expiry display.
    let code = otp::generate_code(&secret, client.ci, otp::current_window());
    otp::send_otp(channel, &destination, &code)
        .await
        .map_err(|e| {
            tracing::error!("request_client_otp: dispatch failed: {e:?}");
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "otp dispatch failed")
        })?;

    let masked = match channel {
        OtpChannel::Sms => otp::mask_phone(&destination),
        OtpChannel::Email => otp::mask_email(&destination),
    };
    tracing::info!(
        client_id = client.id,
        channel = channel.name(),
        "request_client_otp: code dispatched"
    );
    Ok(Json(ClientOtpResponse {
        channel: channel.name().to_string(),
        destination_masked: masked,
        expires_in_secs: otp::OTP_TTL_SECS,
    }))
}

/// `POST /api/v1/clients/otp/verify` — exchanges the login code for a JWT.
///
/// Checks the 6-digit code for `ci` and returns a [`LoginResponse`] whose
/// token expires after 1 hour. Wrong codes count toward lockout.
///
/// # Status codes
///
/// * `200` with the token on success.
/// * `400` for a missing code.
/// * `401` for a wrong or expired code.
/// * `429` while locked out.
/// * `500` on DB or token-signing failures.
#[tracing::instrument(skip(state, request), name = "verify_client_otp")]
pub async fn verify_client_otp(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ClientOtpVerify>,
) -> Result<Json<LoginResponse>, ApiError> {
    tracing::debug!("otp verification received");
    if request.code.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "missing code"));
    }
    state
        .otp_guard()
        .check_not_locked(request.ci)
        .map_err(|e| otp_api_error("verify_client_otp", e))?;

    let secret = otp::otp_secret().map_err(|e| {
        tracing::error!("verify_client_otp: secret error: {e:?}");
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "otp error")
    })?;
    let ok = otp::verify_code(&secret, request.ci, &request.code).map_err(|e| {
        tracing::error!("verify_client_otp: verify error: {e:?}");
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "otp error")
    })?;
    state.otp_guard().record_result(request.ci, ok);
    if !ok {
        tracing::warn!("verify_client_otp: rejected (invalid code)");
        return Err(api_error(StatusCode::UNAUTHORIZED, "invalid code"));
    }

    let client = db::client_by_ci(&state.db, request.ci)
        .await
        .map_err(|e| db_api_error("verify_client_otp", e))?;
    let claims = ClientClaims::new(client.id, client.ci, Utc::now() + Duration::hours(1));
    let token = create_token(claims.into()).map_err(|e| {
        tracing::error!("verify_client_otp: token error: {e:?}");
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "token error")
    })?;
    tracing::info!(client_id = client.id, "verify_client_otp: succeeded");
    Ok(Json(LoginResponse::bearer(token)))
}

/// `POST /api/v1/partners/tickets` — registers an invoice and awards points.
///
/// The [`partner_auth`](crate::jwt::partner_auth) layer guarantees the JWT
/// partner equals `partner_id` before this handler runs. Buyer data from the
/// factura is validated, the client is registered on the fly (find-or-create
/// by unique `ci`), then — inside one DB transaction (see
/// [`db::create_ticket`]) — the `tickets` row (scoped by
/// `(partner_id, ticket_id)`), its `ticket_details` lines and, when points
/// were earned, the ledger award are inserted. Responds with the new
/// `ticket_id`, the resolved `client_id` and the awarded points.
///
/// # Status codes
///
/// * `200` with [`CreateTicketResponse`] on success.
/// * `400` for empty/invalid lines, invoice number or buyer data.
/// * `403` for an inactive partner, or a token scoped to another partner
///   (rejected by the `partner_auth` layer).
/// * `404` for an unknown partner or product.
/// * `409` for a duplicate invoice number.
/// * `500` on DB failures.
#[tracing::instrument(
    skip(state, request),
    fields(
        partner_id = request.partner_id,
        lines = request.details.len(),
        client_id = tracing::field::Empty,
    )
)]
pub async fn add_ticket(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateTicketRequest>,
) -> Result<Json<CreateTicketResponse>, ApiError> {
    // 0. Validate the buyer data from the ticket (also used to register him)
    // 1. Register the client from the ticket data (find-or-create by CI)
    // 2. Insert the new invoice into the DB (scoped per partner)
    // 3. Extract the products and check if they earn points
    // 4. Post the earned points to the ledger (all inside one DB transaction)
    tracing::debug!("add_ticket: request received");
    if request.details.is_empty() {
        tracing::warn!("add_ticket: rejected (empty details)");
        return Err(api_error(StatusCode::BAD_REQUEST, "empty details"));
    }
    for d in &request.details {
        if d.quantity < 1 || d.product_id < 1 {
            tracing::warn!("add_ticket: rejected (invalid line)");
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid line"));
        }
    }
    let ticket_number = request.ticket_id.trim().to_string();
    if ticket_number.is_empty() || ticket_number.len() > 64 {
        tracing::warn!("add_ticket: rejected (invalid ticket number)");
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid ticket number"));
    }
    let (first_name, last_name) = validate_buyer(
        request.client.ci,
        request.client.verification_digit,
        &request.client.first_name,
        &request.client.last_name,
        request.client.phone_number.as_deref(),
        request.client.email.as_deref(),
    )?;

    let created = db::create_ticket(
        &state.db,
        NewTicket {
            partner_id: request.partner_id,
            ticket_number: &ticket_number,
            client: NewClient {
                ci: request.client.ci,
                verification_digit: request.client.verification_digit,
                first_name: &first_name,
                last_name: &last_name,
                phone_number: request.client.phone_number.as_deref(),
                email: request.client.email.as_deref(),
            },
            details: &request
                .details
                .iter()
                .map(|d| TicketLine {
                    product_id: d.product_id,
                    quantity: d.quantity,
                })
                .collect::<Vec<_>>(),
        },
    )
    .await
    .map_err(|e| db_api_error("add_ticket", e))?;

    tracing::Span::current().record("client_id", created.client_id);
    tracing::info!(
        ticket_id = created.ticket_id,
        client_id = created.client_id,
        earned_points = created.earned_points,
        "add_ticket: ticket created"
    );
    Ok(Json(CreateTicketResponse {
        ticket_id: created.ticket_id,
        client_id: created.client_id,
        earned_points: created.earned_points,
    }))
}

/// `DELETE /api/v1/partners/tickets` — voids an invoice by id from the body.
///
/// The [`partner_auth`](crate::jwt::partner_auth) layer guarantees the JWT
/// partner owns the ticket before this handler runs. Cancellation is an
/// audit entry (see [`db::cancel_ticket`]): the original rows stay intact, a
/// `ticket_cancellations` row is recorded with the `reason`, and awarded
/// points are reversed with a compensating ledger entry.
///
/// # Status codes
///
/// * `200` with [`DeleteTicketResponse`] when the invoice was voided.
/// * `400` for a missing/oversize reason.
/// * `403` for a token scoped to another partner (rejected by the
///   `partner_auth` layer).
/// * `404` when no ticket with that id exists.
/// * `409` when already cancelled.
/// * `500` on DB failures.
#[tracing::instrument(skip(state, request), fields(ticket_id = request.ticket_id))]
pub async fn delete_ticket(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DeleteTicketRequest>,
) -> Result<Json<DeleteTicketResponse>, ApiError> {
    // Record the cancellation + reverse awarded points (one DB transaction;
    // the original rows stay intact). Ownership was already enforced by the
    // `partner_auth` layer.
    tracing::debug!("delete_ticket: request received");
    let reason = request.reason.trim().to_string();
    if reason.is_empty() || reason.len() > 256 {
        tracing::warn!("delete_ticket: rejected (invalid reason)");
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid reason"));
    }

    let cancellation_id = db::cancel_ticket(&state.db, request.ticket_id, &reason)
        .await
        .map_err(|e| db_api_error("delete_ticket", e))?;

    tracing::info!(
        ticket_id = request.ticket_id,
        cancellation_id,
        "delete_ticket: ticket voided"
    );
    Ok(Json(DeleteTicketResponse {
        ticket_id: request.ticket_id,
        cancellation_id,
        deleted: true,
    }))
}

/// `GET /api/v1/clients/{client_id}` — serves the points dashboard payload.
///
/// The [`client_auth`](crate::jwt::client_auth) layer guarantees the JWT
/// client equals `client_id` before this handler runs. Returns the client
/// profile, the earned/redeemed/cancelled/balance breakdown (all derived from
/// `point_ledger`) and the full ticket history with per-ticket details,
/// award snapshots and void flags.
///
/// # Status codes
///
/// * `200` with [`ClientDataResponse`] on success.
/// * `403` for a token scoped to another client (rejected by the
///   `client_auth` layer).
/// * `404` for an unknown client.
/// * `500` on DB failures.
#[tracing::instrument(skip(state), fields(client_id))]
pub async fn get_client_data(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<i32>,
) -> Result<Json<ClientDataResponse>, ApiError> {
    // NOTE:
    // This function should get all the data needed for the user to make the point redemtion dashboard.
    tracing::debug!("get_client_data: request received");

    let dashboard = db::client_dashboard(&state.db, client_id)
        .await
        .map_err(|e| db_api_error("get_client_data", e))?;
    tracing::info!(
        tickets = dashboard.tickets.len(),
        total_earned_points = dashboard.total_earned_points,
        "get_client_data: dashboard served"
    );
    Ok(Json(dashboard))
}

/// `POST /api/v1/clients/{client_id}/redeem_points` — redeems points.
///
/// The [`client_auth`](crate::jwt::client_auth) layer guarantees the JWT
/// client equals `client_id` before this handler runs. Every line is priced
/// against `redeemable_products`, the ledger balance must cover the cost, and
/// the redemption is persisted atomically (see [`db::redeem_points`]).
///
/// # Status codes
///
/// * `200` with [`RedeemPointsResponse`] on success.
/// * `400` for empty/invalid items.
/// * `403` for a token scoped to another client (rejected by the
///   `client_auth` layer).
/// * `404` for an unknown client.
/// * `409` for insufficient points (or an already-spent prize row).
/// * `422` for a product that is not redeemable.
/// * `500` on DB failures.
#[tracing::instrument(skip(state, request), fields(client_id, items = request.items.len()))]
pub async fn redeem_points(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<i32>,
    Json(request): Json<RedeemPointsRequest>,
) -> Result<Json<RedeemPointsResponse>, ApiError> {
    // 1. Persist the redemption (items + ledger charge) in one DB transaction.
    tracing::debug!("redeem_points: request received");

    if request.items.is_empty() {
        tracing::warn!("redeem_points: rejected (empty items)");
        return Err(api_error(StatusCode::BAD_REQUEST, "empty items"));
    }
    for item in &request.items {
        if item.quantity < 1 || item.product_id < 1 {
            tracing::warn!("redeem_points: rejected (invalid item)");
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid item"));
        }
    }

    let lines: Vec<RedeemLine> = request
        .items
        .iter()
        .map(|i| RedeemLine {
            product_id: i.product_id,
            quantity: i.quantity,
        })
        .collect();
    let redeemed = db::redeem_points(&state.db, client_id, &lines)
        .await
        .map_err(|e| db_api_error("redeem_points", e))?;

    tracing::info!(
        redemption_id = redeemed.redemption_id,
        redeemed_points = redeemed.redeemed_points,
        remaining_points = redeemed.remaining_points,
        "redeem_points: redemption persisted"
    );
    Ok(Json(RedeemPointsResponse {
        client_id,
        redemption_id: redeemed.redemption_id,
        redeemed_points: redeemed.redeemed_points,
        remaining_points: redeemed.remaining_points,
    }))
}
