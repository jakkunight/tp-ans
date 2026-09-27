use std::{env, sync::Arc};

use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::AppState;

// ============================================================
// Partner Claims
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PartnerClaims {
    /// Partner's unique identifier (matches `partners.id`).
    sub: i64,
    /// Partner's name (matches `partners.name`).
    name: String,
    /// Partner's RUC / tax ID (matches `partners.ruc`).
    ruc: String,
    /// Issued at (Unix timestamp).
    iat: i64,
    /// Expiration (Unix timestamp).
    exp: i64,
}

// ============================================================
// Customer Claims
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CustomerClaims {
    /// Customer's unique identifier (matches `clients.id`).
    sub: i64,
    /// Customer's CI / ID number (matches `clients.ci`).
    ci: i32,
    /// Customer's first name (matches `clients.first_name`).
    first_name: String,
    /// Customer's last name (matches `clients.last_name`).
    last_name: String,
    /// Optional verification digit (matches `clients.verification_digit`).
    verification_digit: Option<i32>,
    /// Issued at (Unix timestamp).
    iat: i64,
    /// Expiration (Unix timestamp).
    exp: i64,
}

// ============================================================
// Generic Claims (backward compatibility)
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Claims {
    subject: String,
    expires: DateTime<Utc>,
}

// ============================================================
// Token Creation Helpers
// ============================================================

pub fn create_token(subject: &str) -> anyhow::Result<JwtToken> {
    let claims = Claims {
        subject: subject.to_string(),
        expires: Utc::now() + chrono::Duration::days(30),
    };

    let secret =
        env::var("LAB_JWT_SECRET").map_err(|_| anyhow::anyhow!("LAB_JWT_SECRET not set"))?;

    let jwt_string = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(JwtToken(jwt_string))
}

pub struct JwtToken(String);

// ============================================================
// Partner Token Helpers
// ============================================================

/// Creates a JWT access token for a partner.
///
/// The token is valid for 30 days and contains the partner's ID, name,
/// and RUC. Use `verify_partner_token` to validate and decode it.
pub fn create_partner_token(
    partner_id: i64,
    partner_name: &str,
    partner_ruc: &str,
) -> anyhow::Result<JwtToken> {
    let now = Utc::now();
    let expires = now + chrono::Duration::days(30);
    let iat = now.timestamp();
    let exp = expires.timestamp();

    let claims = PartnerClaims {
        sub: partner_id,
        name: partner_name.to_string(),
        ruc: partner_ruc.to_string(),
        iat,
        exp,
    };

    let secret =
        env::var("LAB_JWT_SECRET").map_err(|_| anyhow::anyhow!("LAB_JWT_SECRET not set"))?;

    let jwt_string = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(JwtToken(jwt_string))
}

// ============================================================
// Customer Token Helpers
// ============================================================

/// Creates a JWT access token for a customer.
///
/// The token is valid for 30 days and contains the customer's ID, CI,
/// names, and optional verification digit. Use
/// `verify_customer_token` to validate and decode it.
pub fn create_customer_token(
    customer_id: i64,
    ci: i32,
    first_name: &str,
    last_name: &str,
    verification_digit: Option<i32>,
) -> anyhow::Result<JwtToken> {
    let now = Utc::now();
    let expires = now + chrono::Duration::days(30);
    let iat = now.timestamp();
    let exp = expires.timestamp();

    let claims = CustomerClaims {
        sub: customer_id,
        ci,
        first_name: first_name.to_string(),
        last_name: last_name.to_string(),
        verification_digit,
        iat,
        exp,
    };

    let secret =
        env::var("LAB_JWT_SECRET").map_err(|_| anyhow::anyhow!("LAB_JWT_SECRET not set"))?;

    let jwt_string = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(JwtToken(jwt_string))
}

// ============================================================
// Token Verification Helpers
// ============================================================

/// Verifies and decodes a partner JWT token.
///
/// Validates the token signature using `LAB_JWT_SECRET`, checks
/// expiration with a 1-hour leeway, and returns the partner claims.
/// Panics if the token is invalid, expired, or malformed.
pub fn verify_partner_token(token: &str) -> anyhow::Result<PartnerClaims> {
    let secret =
        env::var("LAB_JWT_SECRET").map_err(|_| anyhow::anyhow!("LAB_JWT_SECRET not set"))?;

    let mut validation = Validation::default();
    validation = validation.with_leeway(chrono::Duration::hours(1));

    let data = decode(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?;

    Ok(data.claims)
}

/// Verifies and decodes a customer JWT token.
///
/// Validates the token signature using `LAB_JWT_SECRET`, checks
/// expiration with a 1-hour leeway, and returns the customer claims.
/// Panics if the token is invalid, expired, or malformed.
pub fn verify_customer_token(token: &str) -> anyhow::Result<CustomerClaims> {
    let secret =
        env::var("LAB_JWT_SECRET").map_err(|_| anyhow::anyhow!("LAB_JWT_SECRET not set"))?;

    let mut validation = Validation::default();
    validation = validation.with_leeway(chrono::Duration::hours(1));

    let data = decode(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?;

    Ok(data.claims)
}

// ============================================================
// Generic Token Validation (for backward compatibility)
// ============================================================

async fn validate_token(token: &str) -> anyhow::Result<Claims> {
    let secret = match env::var("LAB_JWT_SECRET") {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to get the secrets from the environment.");
            tracing::error!("Chacke the setup and try the request again.");
            anyhow::bail!(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let data = match decode(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    ) {
        Ok(d) => d,
        Err(_) => {
            tracing::error!("Failed to decode the JWT.");
            tracing::warn!("Did the token expire?");
            anyhow::bail!(StatusCode::UNAUTHORIZED);
        }
    };

    Ok(data.claims)
}

// ============================================================
// JWT Middleware
// ============================================================

/// Middleware for protecting routes that require JWT authentication.
///
/// Expects a `Bearer` token in the `Authorization` header. Decodes the
/// token using the generic `Claims` type and attaches it to the request
/// for downstream handlers. Use `axum::middleware::from_fn_with_state`
/// to attach this to a router.
pub async fn jwt_auth_middleware(
    mut req: Request,
    _state: State<Arc<AppState>>,
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

    match validate_token(token).await {
        Ok(claims) => {
            req.extensions_mut().insert(claims);
            Ok(next.run(req).await)
        }
        Err(_) => Err(StatusCode::UNAUTHORIZED),
    }
}
