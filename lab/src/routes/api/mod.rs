use crate::AppState;
use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{StatusCode, header, Method},
    middleware::Next,
    response::Response,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sqlx::{query, query_as};
use std::{env, sync::Arc};

// ============================================================
// JWT
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Claims {
    subject: String,
    expires: DateTime<Utc>,
}

pub fn create_token(subject: &str) -> anyhow::Result<JwtToken> {
    let claims = Claims {
        subject: subject.to_string(),
        expires: Utc::now() + chrono::Duration::days(30),
    };

    let secret = env::var("LAB_JWT_SECRET").map_err(|_| anyhow::anyhow!("LAB_JWT_SECRET not set"))?;

    let jwt_string = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(JwtToken(jwt_string))
}

pub struct JwtToken(String);

// ============================================================
// JWT Middleware
// ============================================================

async fn jwt_auth_middleware(
    mut req: Request,
    state: State<Arc<AppState>>,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h["Bearer ".len()..],
        _ => return Err(StatusCode::UNAUTHORIZED),
    };

    match validate_token(token, &state).await {
        Ok(claims) => {
            req.extensions_mut().insert(claims);
            Ok(next.run(req).await)
        }
        Err(_) => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn validate_token(
    token: &str,
    state: &State<Arc<AppState>>,
) -> Result<Claims, jsonwebtoken::errors::Error> {
    let secret = env::var("LAB_JWT_SECRET").map_err(|_| jsonwebtoken::errors::Error::TokenInvalidSignature)?;
    decode(token, &DecodingKey::from_secret(secret.as_bytes()), &Validation::default())
}

// ============================================================
// API Routes
// ============================================================

pub fn create_api() -> anyhow::Result<Router<Arc<AppState>>> {
    // Public routes
    let public_api = Router::new()
        .route("/api/v1/partners/:partner_id/login", post(partner_login))
        .route("/api/v1/customers/login", post(customer_login))
        .route("/api/v1/customers", post(create_customer))
        .route("/api/v1/partners", post(create_partner))
        .route("/api/v1/invoices", post(create_invoice))
        .route("/api/v1/invoices/:ticket_id", get(get_invoice))
        .route("/api/v1/invoices/:ticket_id/cancel", post(cancel_invoice))
        .route("/api/v1/customers/:client_id/balance", get(get_balance));

    // Protected routes
    let protected_api = Router::new()
        .route(
            "/api/v1/invoices/:ticket_id/points",
            post(deposit_points),
        )
        .route("/api/v1/customers/:client_id/points", get(get_customer_points))
        .route("/api/v1/customers/:client_id/redeem", post(redeem_points))
        .route("/api/v1/customers/:client_id/redeem/validate", post(redeem_validate));

    let protected_api = Router::new()
        .route(
            "/api/v1/invoices/:ticket_id/points",
            post(deposit_points),
        )
        .route("/api/v1/customers/:client_id/points", get(get_customer_points))
        .route("/api/v1/customers/:client_id/redeem", post(redeem_points))
        .route("/api/v1/customers/:client_id/redeem/validate", post(redeem_validate));

    let api = Router::new()
        .merge(public_api)
        .merge(protected_api);

    Ok(api)
}

// ============================================================
// Partners
// ============================================================

pub async fn partner_login(
    Path(partner_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let count = query("SELECT COUNT(*) as count FROM partners WHERE id = $1")
        .bind(&partner_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if count.0 == 0 {
        tracing::error!("Partner not found");
        return Err(StatusCode::NOT_FOUND);
    }

    create_token(&partner_id)
        .map_err(|e| {
            tracing::error!("Token error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .0
        .parse::<serde_json::Value>()
        .map_err(|e| {
            tracing::error!("JSON error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

// ============================================================
// Customers
// ============================================================

pub async fn customer_login(
    State(state): State<Arc<AppState>>,
    Json(data): Json<CustomerLoginRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let count = query(
        "SELECT COUNT(*) as count FROM clients WHERE ci = $1 AND first_name = $2 AND last_name = $3",
    )
    .bind(&data.ci)
    .bind(&data.first_name)
    .bind(&data.last_name)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("DB error: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if count.0 == 0 {
        tracing::error!("Customer not found");
        return Err(StatusCode::NOT_FOUND);
    }

    create_token(&format!("C{}", data.ci))
        .map_err(|e| {
            tracing::error!("Token error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .0
        .parse::<serde_json::Value>()
        .map_err(|e| {
            tracing::error!("JSON error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

pub async fn create_customer(
    State(state): State<Arc<AppState>>,
    Json(data): Json<CreateCustomerRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let result: Result<_, sqlx::Error> = query(
        r#"INSERT INTO clients (ci, first_name, last_name, verification_digit)
           VALUES ($1, $2, $3, $4)
           RETURNING id, ci, first_name, last_name, verification_digit
           ORDER BY id DESC LIMIT 1;"#
    )
    .bind(&data.ci)
    .bind(&data.first_name)
    .bind(&data.last_name)
    .bind(data.verification_digit)
    .fetch_one(&state.db);

    match result {
        Ok(row) => Ok(Json(serde_json::json!({
            "id": row.id,
            "ci": row.ci,
            "first_name": row.first_name,
            "last_name": row.last_name,
            "verification_digit": row.verification_digit,
        }))),
        Err(e) => {
            tracing::error!("Insert error: {e:?}");
            if e.to_string().contains("duplicate") {
                Err(StatusCode::CONFLICT)
            } else {
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

// ============================================================
// Invoices
// ============================================================

pub async fn create_invoice(
    State(state): State<Arc<AppState>>,
    Json(data): Json<CreateInvoiceRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Check if partner exists
    let partner_count = query("SELECT COUNT(*) FROM partners WHERE id = $1")
        .bind(&data.partner_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if partner_count.0 == 0 {
        tracing::error!("Partner not found");
        return Err(StatusCode::NOT_FOUND);
    }

    // Check if client exists
    let client_count = query(
        "SELECT COUNT(*) FROM clients WHERE id = $1",
    )
    .bind(&data.client_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("DB error: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if client_count.0 == 0 {
        tracing::error!("Client not found");
        return Err(StatusCode::NOT_FOUND);
    }

    // Insert ticket
    let result: Result<_, sqlx::Error> = query(
        r#"INSERT INTO tickets (date, partner_id, client_id)
           VALUES ($1, $2, $3)
           RETURNING id, date, partner_id, client_id;"#
    )
    .bind(&data.date)
    .bind(&data.partner_id)
    .bind(&data.client_id)
    .fetch_one(&state.db);

    match result {
        Ok(row) => {
            // Insert ticket details
            let mut detail_inserts = Vec::::new();
            for item in &data.items {
                let item = item;
                detail_inserts.push(format!(
                    "INSERT INTO ticket_details (ticket_id, product_id, quantity) \n \
                     VALUES ({}, {}, {})",
                    row.id, item.product_id, item.quantity
                ));
            }

            detail_inserts.join(";\n").as_str()
        }
        Err(e) => {
            tracing::error!("Insert error: {e:?}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================
// Invoice Cancellation
// ============================================================

pub async fn cancel_invoice(
    Path(ticket_id): Path<i32>,
    State(state): State<Arc<AppState>>,
    Json(data): Json<CancelInvoiceRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let exists: bool = query(
        "SELECT EXISTS(SELECT 1 FROM tickets WHERE id = $1)",
    )
        .bind(&ticket_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .exists;

    if !exists {
        tracing::error!("Invoice not found");
        return Err(StatusCode::NOT_FOUND);
    }

    let partner_count: (i32,) = query(
        "SELECT COUNT(*) FROM partners WHERE id = $1",
    )
        .bind(&data.partner_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if partner_count.0 == 0 {
        tracing::error!("Partner not found");
        return Err(StatusCode::NOT_FOUND);
    }

    let invoice_count: (i32,) = query(
        "SELECT COUNT(*) FROM tickets WHERE id = $1 AND partner_id = $2",
    )
        .bind(&ticket_id)
        .bind(&data.partner_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if invoice_count.0 == 0 {
        tracing::error!("Invoice not found or partner mismatch");
        return Err(StatusCode::NOT_FOUND);
    }

    query("INSERT INTO redemptions (date, client_id, points_deducted, reason) \n VALUES ($1, $2, $3, $4)")
        .bind(Utc::now())
        .bind(&data.client_id)
        .bind(0)
        .bind(&data.reason)
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Insert error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "status": "cancelled",
        "reason": data.reason,
    }))))
}

// ============================================================
// Balance & Points
// ============================================================

pub async fn get_balance(
    Path(ticket_id): Path<i32>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let row = query(
        r#"SELECT t.id, t.date, t.partner_id, t.client_id,
               c.id as client_id, c.first_name || ' ' || c.last_name as name,
               COALESCE((SELECT SUM(earned_points)
                        FROM point_earnings WHERE ticket_id = t.id), 0) as earned_points,
               COALESCE((SELECT SUM(rd.points_deducted)
                        FROM redemptions r
                        JOIN redemption_details rd ON rd.redemption_id = r.id
                        WHERE r.client_id = t.client_id AND rd.product_id = (
                            SELECT product_id FROM ticket_details WHERE ticket_id = t.id LIMIT 1
                        )), 0) as deducted_points,
               (COALESCE((SELECT SUM(earned_points) FROM point_earnings WHERE ticket_id = t.id), 0)
                - COALESCE((SELECT SUM(rd.points_deducted)
                           FROM redemptions r
                           JOIN redemption_details rd ON rd.redemption_id = r.id
                           WHERE r.client_id = t.client_id), 0)) as balance
           FROM tickets t
           LEFT JOIN clients c ON c.id = t.client_id
           WHERE t.id = $1
           LIMIT 1;"#
    )
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "ticket_id": row.id,
        "ticket_date": row.date,
        "partner_id": row.partner_id,
        "name": row.name,
        "earned_points": row.earned_points,
        "deducted_points": row.deducted_points,
        "balance": row.balance,
    }))))
}

pub async fn get_customer_points(
    State(state): State<Arc<AppState>>,
    Json(data): Json<GetCustomerPointsRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let row = query(
        r#"SELECT c.id, c.first_name || ' ' || c.last_name as name,
               COALESCE((SELECT SUM(earned_points) FROM point_earnings pe
                        WHERE pe.client_id = c.id), 0) as earned_points,
               COALESCE((SELECT SUM(rd.points_deducted) FROM redemptions r
                        JOIN redemption_details rd ON rd.redemption_id = r.id
                        WHERE r.client_id = c.id), 0) as deducted_points,
               (COALESCE((SELECT SUM(earned_points) FROM point_earnings pe
                          WHERE pe.client_id = c.id), 0)
                - COALESCE((SELECT SUM(rd.points_deducted) FROM redemptions r
                           JOIN redemption_details rd ON rd.redemption_id = r.id
                           WHERE r.client_id = c.id), 0)) as balance
           FROM clients c
           WHERE c.id = $1
           LIMIT 1;"#
    )
        .bind(&data.client_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "client_id": row.id,
        "name": row.name,
        "earned_points": row.earned_points,
        "deducted_points": row.deducted_points,
        "balance": row.balance,
    }))))
}

// ============================================================
// Deposit Points
// ============================================================

pub async fn deposit_points(
    State(state): State<Arc<AppState>>,
    Json(data): Json<DepositPointsRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ticket_exists: (bool,) = query(
        "SELECT EXISTS(SELECT 1 FROM tickets WHERE id = $1)",
    )
        .bind(&data.ticket_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !ticket_exists.0 {
        tracing::error!("Ticket not found");
        return Err(StatusCode::NOT_FOUND);
    }

    let client_id: (i32,) = query(
        "SELECT client_id FROM tickets WHERE id = $1",
    )
        .bind(&data.ticket_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let earned: (i32,) = query(
        "SELECT SUM(earned_points) FROM point_earnings WHERE ticket_id = $1",
    )
        .bind(&data.ticket_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let result: Result<_, sqlx::Error> = query(
        r#"INSERT INTO point_earnings (ticket_id, earned_points)
           VALUES ($1, $2)
           RETURNING id, ticket_id, earned_points;"#,
    )
        .bind(&data.ticket_id)
        .bind(&earned.0)
        .fetch_one(&state.db);

    match result {
        Ok(row) => Ok(Json(serde_json::json!({
            "id": row.id,
            "ticket_id": row.ticket_id,
            "earned_points": row.earned_points,
        }))),
        Err(e) => {
            tracing::error!("Insert error: {e:?}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================
// Redeem Points
// ============================================================

pub async fn redeem_points(
    State(state): State<Arc<AppState>>,
    Json(data): Json<RedeemPointsRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ticket_exists: (bool,) = query(
        "SELECT EXISTS(SELECT 1 FROM tickets WHERE id = $1)",
    )
        .bind(&data.ticket_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !ticket_exists.0 {
        tracing::error!("Ticket not found");
        return Err(StatusCode::NOT_FOUND);
    }

    let client_id: (i32,) = query(
        "SELECT client_id FROM tickets WHERE id = $1",
    )
        .bind(&data.ticket_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let points_needed: (i32,) = query(
        "SELECT points_needed FROM redeemable_products WHERE id = $1",
    )
        .bind(&data.product_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let balance: (i32,) = query(
        r#"SELECT (COALESCE((SELECT SUM(earned_points)
                        FROM point_earnings WHERE client_id = $1), 0)
                 - COALESCE((SELECT SUM(rd.points_deducted)
                           FROM redemptions r
                           JOIN redemption_details rd ON rd.redemption_id = r.id
                           WHERE r.client_id = $1), 0)) AS balance"#
    )
        .bind(&client_id.0)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if balance.0 < points_needed.0 {
        tracing::error!("Insufficient balance: {balance.0} < {points_needed.0}");
        return Err(StatusCode::INSUFFICIENT_FUNDS);
    }

    let result: Result<_, sqlx::Error> = query(
        r#"INSERT INTO redemption_details (redemption_id, product_id, quantity)
           VALUES (gen_random_sequence('redemption_seq', BIGINT, maxvalue => 1000000)::bigint, $1, $2)
           RETURNING id, redemption_id, product_id, quantity;"#,
    )
        .bind(&data.product_id)
        .bind(&data.quantity)
        .fetch_one(&state.db);

    match result {
        Ok(row) => Ok(Json(serde_json::json!({
            "id": row.id,
            "redemption_id": row.redemption_id,
            "product_id": row.product_id,
            "quantity": row.quantity,
        }))),
        Err(e) => {
            tracing::error!("Insert error: {e:?}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================
// Redeem Validate
// ============================================================

pub async fn redeem_validate(
    State(state): State<Arc<AppState>>,
    Json(data): Json<RedeemValidateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ticket_exists: (bool,) = query(
        "SELECT EXISTS(SELECT 1 FROM tickets WHERE id = $1)",
    )
        .bind(&data.ticket_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !ticket_exists.0 {
        tracing::error!("Ticket not found");
        return Err(StatusCode::NOT_FOUND);
    }

    let client_id: (i32,) = query(
        "SELECT client_id FROM tickets WHERE id = $1",
    )
        .bind(&data.ticket_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let balance: (i32,) = query(
        r#"SELECT (COALESCE((SELECT SUM(earned_points)
                        FROM point_earnings WHERE client_id = $1), 0)
                 - COALESCE((SELECT SUM(rd.points_deducted)
                           FROM redemptions r
                           JOIN redemption_details rd ON rd.redemption_id = r.id
                           WHERE r.client_id = $1), 0)) AS balance"#
    )
        .bind(&client_id.0)
        .fetch_one(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("DB error: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::json!({
        "ticket_id": data.ticket_id,
        "client_id": client_id.0,
        "balance": balance.0,
    }))))
}

// ============================================================
// Request DTOs
// ============================================================

#[derive(Debug, Deserialize)]
struct CreateCustomerRequest {
    ci: i32,
    first_name: String,
    last_name: String,
    verification_digit: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct CustomerLoginRequest {
    ci: i32,
    first_name: String,
    last_name: String,
}

#[derive(Debug, Deserialize)]
struct CreateInvoiceRequest {
    partner_id: i32,
    client_id: i32,
    date: String,
    items: Vec<InvoiceItem>,
}

#[derive(Debug, Deserialize)]
struct InvoiceItem {
    product_id: i32,
    quantity: i32,
}

#[derive(Debug, Deserialize)]
struct CancelInvoiceRequest {
    partner_id: i32,
    client_id: i32,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct GetCustomerPointsRequest {
    client_id: i32,
}

#[derive(Debug, Deserialize)]
struct DepositPointsRequest {
    ticket_id: i32,
}

#[derive(Debug, Deserialize)]
struct RedeemPointsRequest {
    ticket_id: i32,
    product_id: i32,
    quantity: i32,
}

#[derive(Debug, Deserialize)]
struct RedeemValidateRequest {
    ticket_id: i32,
}
