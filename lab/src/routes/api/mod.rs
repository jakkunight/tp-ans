//! # JSON REST API (`/api/v1/...`).
//!
//! Request/response shapes live in [`dto`]; every DTO derives [`Clone`],
//! [`Serialize`](serde::Serialize) and [`Deserialize`](serde::Deserialize).
//! Handlers query Postgres through [`crate::AppState`] and mint
//! JWTs via [`crate::jwt`]. Traces carry numeric ids/counts only —
//! credentials (CI, names, RUC, secrets) and tokens are never logged.
//!
//! | Method & path | Handler | Auth |
//! |---|---|---|
//! | `POST /api/v1/partners/login` | [`partner_login`] | public |
//! | `POST /api/v1/clients/login` | [`client_login`] | public |
//! | `POST /api/v1/partners/tickets` | [`add_ticket`] | partner JWT |
//! | `DELETE /api/v1/partners/tickets` | [`delete_ticket`] | partner JWT |
//! | `GET /api/v1/clients/{client_id}` | [`get_client_data`] | client JWT |
//! | `POST /api/v1/clients/{client_id}/redeem_points` | [`redeem_points`] | client JWT |
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use chrono::{Duration, Utc};

use crate::{
    AppState,
    jwt::{ClientClaims, PartnerClaims, create_token},
    models::{Clients, Partners},
};

use self::dto::{
    ClientDataResponse, ClientInfoDto, ClientLoginRequest, CreateTicketRequest,
    CreateTicketResponse, DeleteTicketRequest, DeleteTicketResponse, LoginResponse,
    PartnerLoginRequest, RedeemPointsRequest, RedeemPointsResponse, TicketDetailDto, TicketDto,
};

pub mod dto;

// NOTE on logging: traces below deliberately carry only operational,
// non-sensitive fields (numeric DB ids, counts, point totals). Credentials
// (CI, names, RUC, secrets) and JWT tokens are NEVER logged.

/// Builds the `/api/v1/...` router: two public login routes plus the
/// ticket/client routes (intended to sit behind [`jwt_middleware`](crate::jwt::jwt_middleware)).
///
/// # Errors
///
/// Currently infallible; returns [`anyhow::Error`] to allow future fallible setup.
pub fn create_api() -> anyhow::Result<Router<Arc<AppState>>> {
    let public_api = Router::new()
        .route("/api/v1/partners/login", post(partner_login))
        .route("/api/v1/clients/login", post(client_login));
    let private_api = Router::new()
        .route("/api/v1/partners/tickets", post(add_ticket))
        .route("/api/v1/partners/tickets", delete(delete_ticket))
        .route("/api/v1/clients/{client_id}", get(get_client_data))
        .route(
            "/api/v1/clients/{client_id}/redeem_points",
            post(redeem_points),
        );
    let router = Router::new().merge(public_api).merge(private_api);
    Ok(router)
}

/// `POST /api/v1/clients/login` — identifies a client and mints his JWT.
///
/// Checks `ci` + `first_name` + `last_name` against `clients` (`ci` alone is
/// unique; the verification digit is optional data and is not checked) and
/// returns a [`LoginResponse`] whose token expires after 1 hour.
///
/// # Status codes
///
/// * `200` with the token on success.
/// * `401` for unknown credentials.
/// * `500` on DB or token-signing failures.
#[tracing::instrument(skip(state, credentials), name = "client_login")]
pub async fn client_login(
    State(state): State<Arc<AppState>>,
    Json(credentials): Json<ClientLoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    // Clients are uniquely identified by `ci` alone. The verification digit
    // (`<ci>-<digit>` RUC suffix) is government-issued optional data: it is
    // neither required for login nor for ticket generation, so only
    // `ci` + `first_name` + `last_name` are checked here.
    tracing::debug!("client login attempt");
    let client: Clients = sqlx::query_as::<_, Clients>(
        "SELECT id, ci, verification_digit, first_name, last_name FROM clients WHERE ci = $1 AND first_name = $2 AND last_name = $3",
    )
    .bind(credentials.ci)
    .bind(&credentials.first_name)
    .bind(&credentials.last_name)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("client_login: db error: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or_else(|| {
        tracing::warn!("client_login: rejected (unknown credentials)");
        StatusCode::UNAUTHORIZED
    })?;

    let claims = ClientClaims::new(
        client.id.to_string(),
        client.ci.to_string(),
        Utc::now() + Duration::hours(1),
    );
    let token = create_token(claims.into()).map_err(|e| {
        tracing::error!("client_login: token error: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    tracing::info!(client_id = client.id, "client_login: succeeded");
    Ok(Json(LoginResponse::bearer(token)))
}

/// `POST /api/v1/partners/login` — authenticates a pharmacy and mints its JWT.
///
/// Looks the partner up by `ruc` (must be active) and returns a
/// [`LoginResponse`]. The optional request `secret` is echoed into
/// [`crate::jwt::PartnerClaims`]; the token itself carries no expiry.
///
/// # Status codes
///
/// * `200` with the token on success.
/// * `401` for an unknown `ruc`.
/// * `403` for an inactive partner.
/// * `500` on DB or token-signing failures.
#[tracing::instrument(skip(state, credentials), name = "partner_login")]
pub async fn partner_login(
    State(state): State<Arc<AppState>>,
    Json(credentials): Json<PartnerLoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    tracing::debug!("partner login attempt");
    let partner: Partners = sqlx::query_as::<_, Partners>(
        "SELECT id, name, ruc, is_active FROM partners WHERE ruc = $1",
    )
    .bind(&credentials.ruc)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("partner_login: db error: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or_else(|| {
        tracing::warn!("partner_login: rejected (unknown credentials)");
        StatusCode::UNAUTHORIZED
    })?;

    if !partner.is_active {
        tracing::warn!(
            partner_id = partner.id,
            "partner_login: rejected (inactive partner)"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    let secret = credentials.secret.unwrap_or_default();
    let claims = PartnerClaims::new(partner.id.to_string(), secret, None);
    let token = create_token(claims.into()).map_err(|e| {
        tracing::error!("partner_login: token error: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    tracing::info!(partner_id = partner.id, "partner_login: succeeded");
    Ok(Json(LoginResponse::bearer(token)))
}

/// `POST /api/v1/partners/tickets` — registers an invoice and awards points.
///
/// Validates the buyer data from the factura, registers the client on the
/// fly (find-or-create by unique `ci`), then — inside one transaction —
/// inserts the `tickets` row, its `ticket_details` lines and, when at least
/// one point was earned (`quantity × promotion_products.points_cost`), the
/// `point_earnings` snapshot. Responds with the new `ticket_id`, the
/// resolved `client_id` and the awarded points.
///
/// # Status codes
///
/// * `200` with [`CreateTicketResponse`] on success.
/// * `400` for empty/invalid lines or buyer data.
/// * `403` for an inactive partner.
/// * `404` for an unknown partner or product.
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
) -> Result<Json<CreateTicketResponse>, StatusCode> {
    // 0. Validate the buyer data from the ticket (also used to register him)
    // 1. Register the client from the ticket data (find-or-create by CI)
    // 2. Insert the new ticket into the DB
    // 3. Extract the products and check if they can be exchanged by points
    // 4. Register the point gained from the purchase to the corresponding user
    tracing::debug!("add_ticket: request received");
    if request.details.is_empty() {
        tracing::warn!("add_ticket: rejected (empty details)");
        return Err(StatusCode::BAD_REQUEST);
    }
    for d in &request.details {
        if d.quantity < 1 || d.product_id < 1 {
            tracing::warn!("add_ticket: rejected (invalid line quantity/product id)");
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    // Buyer identity comes from the factura itself: `ci` is unique and
    // enough to identify him; the verification digit is optional.
    // (Values are not logged: CI and names are sensitive.)
    if request.client.ci < 0 {
        tracing::warn!("add_ticket: rejected (invalid buyer ci)");
        return Err(StatusCode::BAD_REQUEST);
    }
    let first_name = request.client.first_name.trim();
    let last_name = request.client.last_name.trim();
    if first_name.is_empty() || last_name.is_empty() {
        tracing::warn!("add_ticket: rejected (missing buyer name)");
        return Err(StatusCode::BAD_REQUEST);
    }
    if first_name.len() > 32 || last_name.len() > 32 {
        tracing::warn!("add_ticket: rejected (buyer name too long)");
        return Err(StatusCode::BAD_REQUEST);
    }
    if let Some(vd) = request.client.verification_digit {
        if !(0..=9).contains(&vd) {
            tracing::warn!("add_ticket: rejected (invalid verification digit)");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let partner: Partners = sqlx::query_as::<_, Partners>(
        "SELECT id, name, ruc, is_active FROM partners WHERE id = $1",
    )
    .bind(request.partner_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("add_ticket: partner lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or_else(|| {
        tracing::warn!(
            partner_id = request.partner_id,
            "add_ticket: partner not found"
        );
        StatusCode::NOT_FOUND
    })?;
    if !partner.is_active {
        tracing::warn!(
            partner_id = partner.id,
            "add_ticket: rejected (inactive partner)"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    let product_ids: Vec<i32> = request.details.iter().map(|d| d.product_id).collect();
    let found_ids: Vec<i32> = sqlx::query_scalar("SELECT id FROM products WHERE id = ANY($1)")
        .bind(&product_ids)
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("add_ticket: product lookup failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let found: HashSet<i32> = found_ids.into_iter().collect();
    if request
        .details
        .iter()
        .any(|d| !found.contains(&d.product_id))
    {
        tracing::warn!("add_ticket: rejected (unknown product id in details)");
        return Err(StatusCode::NOT_FOUND);
    }

    // Points earned per unit come from `promotion_products.points_cost`.
    let promo_rows: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT product_id, points_cost FROM promotion_products WHERE product_id = ANY($1)",
    )
    .bind(&product_ids)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("add_ticket: promotion lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let promo: HashMap<i32, i32> = promo_rows.into_iter().collect();
    let earned_points: i64 = request
        .details
        .iter()
        .map(|d| *promo.get(&d.product_id).unwrap_or(&0) as i64 * d.quantity as i64)
        .sum();
    let earned_points: i32 = earned_points
        .try_into()
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let mut tx = state.db.begin().await.map_err(|e| {
        tracing::error!("add_ticket: begin failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Register the buyer from the ticket data: reuse the client matching
    // `ci`, or insert him when unknown. The `ON CONFLICT DO NOTHING` keeps
    // concurrent registrations for the same CI race-safe.
    let preexisting: Option<i32> = sqlx::query_scalar("SELECT id FROM clients WHERE ci = $1")
        .bind(request.client.ci)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("add_ticket: client lookup failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let (client_id, client_created) = match preexisting {
        Some(id) => (id, false),
        None => {
            sqlx::query(
                "INSERT INTO clients (ci, verification_digit, first_name, last_name)
                 VALUES ($1, $2, $3, $4) ON CONFLICT (ci) DO NOTHING",
            )
            .bind(request.client.ci)
            .bind(request.client.verification_digit)
            .bind(first_name)
            .bind(last_name)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                tracing::error!("add_ticket: client insert failed: {e:?}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            let id: i32 = sqlx::query_scalar("SELECT id FROM clients WHERE ci = $1")
                .bind(request.client.ci)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| {
                    tracing::error!("add_ticket: client lookup failed: {e:?}");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            (id, true)
        }
    };
    tracing::Span::current().record("client_id", client_id);
    if client_created {
        tracing::info!(client_id, "add_ticket: client registered");
    } else {
        tracing::debug!(client_id, "add_ticket: existing client reused");
    }

    let ticket_id: i32 = sqlx::query_scalar(
        "INSERT INTO tickets (partner_id, client_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(request.partner_id)
    .bind(client_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("add_ticket: insert ticket failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    for d in &request.details {
        sqlx::query(
            "INSERT INTO ticket_details (ticket_id, product_id, quantity) VALUES ($1, $2, $3)",
        )
        .bind(ticket_id)
        .bind(d.product_id)
        .bind(d.quantity)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("add_ticket: insert detail failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    if earned_points >= 1 {
        sqlx::query("INSERT INTO point_earnings (ticket_id, earned_points) VALUES ($1, $2)")
            .bind(ticket_id)
            .bind(earned_points)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                tracing::error!("add_ticket: insert earnings failed: {e:?}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    tx.commit().await.map_err(|e| {
        tracing::error!("add_ticket: commit failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let earned = if earned_points >= 1 { earned_points } else { 0 };
    tracing::info!(
        ticket_id,
        client_id,
        earned_points = earned,
        "add_ticket: ticket created"
    );
    Ok(Json(CreateTicketResponse {
        ticket_id,
        client_id,
        earned_points: earned,
    }))
}

/// `DELETE /api/v1/partners/tickets` — voids an invoice by id from the body.
///
/// The delete cascades to `ticket_details` and `point_earnings` via
/// `ON DELETE CASCADE`.
///
/// # Status codes
///
/// * `200` with [`DeleteTicketResponse`] when a row was deleted.
/// * `404` when no ticket with that id exists.
/// * `500` on DB failures.
#[tracing::instrument(skip(state, request), fields(ticket_id = request.ticket_id))]
pub async fn delete_ticket(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DeleteTicketRequest>,
) -> Result<Json<DeleteTicketResponse>, StatusCode> {
    // 1. Delete the inserted ticket
    // 2. Delete the associated point exchanges (DB should do this automaticaly
    //    via `ON DELETE CASCADE` on `ticket_details` and `point_earnings`)
    tracing::debug!("delete_ticket: request received");
    let deleted_id: Option<i32> =
        sqlx::query_scalar("DELETE FROM tickets WHERE id = $1 RETURNING id")
            .bind(request.ticket_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("delete_ticket: delete failed: {e:?}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    match deleted_id {
        Some(ticket_id) => {
            tracing::info!(ticket_id, "delete_ticket: ticket deleted");
            Ok(Json(DeleteTicketResponse {
                ticket_id,
                deleted: true,
            }))
        }
        None => {
            tracing::warn!("delete_ticket: ticket not found");
            Err(StatusCode::NOT_FOUND)
        }
    }
}

/// Sums the `point_earnings` snapshots of every ticket owned by a client.
///
/// Helper shared by [`get_client_data`] and [`redeem_points`].
///
/// # Errors
///
/// Returns [`StatusCode::INTERNAL_SERVER_ERROR`] when the `SUM` query fails.
#[tracing::instrument(skip(db), fields(client_id))]
async fn total_earned_points(db: &sqlx::PgPool, client_id: i32) -> Result<i32, StatusCode> {
    let total: Option<i64> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(pe.earned_points), 0) FROM point_earnings pe
         JOIN tickets t ON t.id = pe.ticket_id WHERE t.client_id = $1",
    )
    .bind(client_id)
    .fetch_one(db)
    .await
    .map_err(|e| {
        tracing::error!("points total lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(total.unwrap_or(0).try_into().unwrap_or(i32::MAX))
}

/// `GET /api/v1/clients/{client_id}` — serves the points dashboard payload.
///
/// Returns the client profile, the earned/redeemed/balance breakdown and the
/// full ticket history with per-ticket details and `point_earnings`
/// snapshots. Redeemed history is `0` until a `redemptions` table exists
/// (redemption totals are derived, never stored).
///
/// # Status codes
///
/// * `200` with [`ClientDataResponse`] on success.
/// * `404` for an unknown client.
/// * `500` on DB failures.
#[tracing::instrument(skip(state), fields(client_id))]
pub async fn get_client_data(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<i32>,
) -> Result<Json<ClientDataResponse>, StatusCode> {
    // NOTE:
    // This function should get all the data needed for the user to make the point redemtion dashboard.
    tracing::debug!("get_client_data: request received");
    let client: Clients = sqlx::query_as::<_, Clients>(
        "SELECT id, ci, verification_digit, first_name, last_name FROM clients WHERE id = $1",
    )
    .bind(client_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("get_client_data: client lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or_else(|| {
        tracing::warn!("get_client_data: client not found");
        StatusCode::NOT_FOUND
    })?;

    let ticket_rows: Vec<(i32, chrono::DateTime<Utc>, i32, i32)> = sqlx::query_as(
        "SELECT id, date, partner_id, client_id FROM tickets WHERE client_id = $1 ORDER BY date ASC, id ASC",
    )
    .bind(client_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("get_client_data: tickets lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut tickets = Vec::with_capacity(ticket_rows.len());
    for (id, date, partner_id, ticket_client_id) in ticket_rows {
        let detail_rows: Vec<(i32, i32)> = sqlx::query_as(
            "SELECT product_id, quantity FROM ticket_details WHERE ticket_id = $1 ORDER BY product_id ASC",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("get_client_data: details lookup failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let earned: Option<i32> =
            sqlx::query_scalar("SELECT earned_points FROM point_earnings WHERE ticket_id = $1")
                .bind(id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| {
                    tracing::error!("get_client_data: earnings lookup failed: {e:?}");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        tickets.push(TicketDto {
            id,
            date,
            partner_id,
            client_id: ticket_client_id,
            details: detail_rows
                .into_iter()
                .map(|(product_id, quantity)| TicketDetailDto {
                    product_id,
                    quantity,
                })
                .collect(),
            earned_points: earned,
        });
    }

    let total_earned_points = total_earned_points(&state.db, client_id).await?;
    // No `redemptions` table exists in `schema.sql`; redeemed totals are
    // derived (`quantity * redeemable_products.points_needed`) only when a
    // redemption history table exists. Until then the history is 0.
    let total_redeemed_points = 0;
    let balance_points = total_earned_points - total_redeemed_points;

    tracing::info!(
        tickets = tickets.len(),
        total_earned_points,
        "get_client_data: dashboard served"
    );
    Ok(Json(ClientDataResponse {
        client: ClientInfoDto {
            id: client.id,
            ci: client.ci,
            verification_digit: client.verification_digit,
            first_name: client.first_name,
            last_name: client.last_name,
        },
        total_earned_points,
        total_redeemed_points,
        balance_points,
        tickets,
    }))
}

/// `POST /api/v1/clients/{client_id}/redeem_points` — prices a points redemption.
///
/// Validates every line against `redeemable_products`, derives the cost as
/// `Σ quantity × points_needed` and rejects the request when the balance
/// (`total_earned − redeemed`) would go negative. Nothing is persisted yet:
/// `schema.sql` has no `redemptions` table, so only the computed totals are
/// returned.
///
/// # Status codes
///
/// * `200` with [`RedeemPointsResponse`] on success.
/// * `400` for empty/invalid items.
/// * `404` for an unknown client.
/// * `409` for insufficient points.
/// * `422` for a product that is not redeemable.
/// * `500` on DB failures.
#[tracing::instrument(skip(state, request), fields(client_id, items = request.items.len()))]
pub async fn redeem_points(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<i32>,
    Json(request): Json<RedeemPointsRequest>,
) -> Result<Json<RedeemPointsResponse>, StatusCode> {
    // 1. Add a redemption generated by the user to the database.
    //    NOTE: `schema.sql` currently has no `redemptions` table, so the
    //    redemption is validated and priced against `redeemable_products`
    //    but only the computed totals are returned (nothing to persist).
    tracing::debug!("redeem_points: request received");
    if request.items.is_empty() {
        tracing::warn!("redeem_points: rejected (empty items)");
        return Err(StatusCode::BAD_REQUEST);
    }
    for item in &request.items {
        if item.quantity < 1 || item.product_id < 1 {
            tracing::warn!("redeem_points: rejected (invalid item quantity/product id)");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let exists: Option<i32> = sqlx::query_scalar("SELECT id FROM clients WHERE id = $1")
        .bind(client_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("redeem_points: client lookup failed: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if exists.is_none() {
        tracing::warn!("redeem_points: client not found");
        return Err(StatusCode::NOT_FOUND);
    }

    let product_ids: Vec<i32> = request.items.iter().map(|i| i.product_id).collect();
    let price_rows: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT product_id, points_needed FROM redeemable_products WHERE product_id = ANY($1)",
    )
    .bind(&product_ids)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("redeem_points: redeemable lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let prices: HashMap<i32, i32> = price_rows.into_iter().collect();

    let mut redeemed: i64 = 0;
    for item in &request.items {
        let Some(price) = prices.get(&item.product_id) else {
            tracing::warn!(
                product_id = item.product_id,
                "redeem_points: rejected (product not redeemable)"
            );
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        };
        redeemed += *price as i64 * item.quantity as i64;
    }
    let redeemed_points: i32 = redeemed.try_into().map_err(|_| StatusCode::BAD_REQUEST)?;

    let total_earned = total_earned_points(&state.db, client_id).await?;
    // See note in `get_client_data`: no redemption history table exists yet.
    let remaining_points = total_earned - redeemed_points;
    if remaining_points < 0 {
        tracing::warn!(
            redeemed_points,
            total_earned,
            "redeem_points: rejected (insufficient points)"
        );
        return Err(StatusCode::CONFLICT);
    }

    tracing::info!(
        redeemed_points,
        remaining_points,
        "redeem_points: redemption priced"
    );
    Ok(Json(RedeemPointsResponse {
        client_id,
        redeemed_points,
        remaining_points,
    }))
}
