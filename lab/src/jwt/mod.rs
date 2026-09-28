//! # JWT authentication.
//!
//! Issues and validates the bearer tokens used by the REST API
//! ([`crate::routes::api`]). Two claim shapes exist — one per actor:
//!
//! * [`PartnerClaims`] for pharmacies ([`partners`](crate::models::Partners)),
//!   minted by `POST /api/v1/partners/login`.
//! * [`ClientClaims`] for end customers ([`clients`](crate::models::Clients)),
//!   minted by `POST /api/v1/clients/login` with a 1-hour expiry.
//!
//! Tokens are HS256-signed with the `LAB_JWT_SECRET` environment secret via
//! [`create_token`] and checked with [`validate_token`]. [`jwt_middleware`]
//! is the Axum layer that enforces `Authorization: Bearer <token>` and
//! exposes the decoded [`Claims`] through request extensions.
//!
//! Claim payloads carry internal ids only — never passwords, tokens or other
//! secrets beyond the partner's pre-shared credential.
use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

/// JWT claims identifying an authenticated partner (pharmacy).
///
/// Encoded as the [`Claims::Partner`] variant. The `partner_secret` is the
/// optional pre-shared credential supplied at login, echoed back so
/// downstream handlers can re-verify it without a DB round-trip.
#[derive(Clone, Serialize, Deserialize)]
pub struct PartnerClaims {
    /// `partners.id` of the authenticated pharmacy, as a string.
    partner_id: String,
    /// Pre-shared credential supplied at login (may be empty).
    partner_secret: String,
    /// Optional absolute expiry; `None` means the token does not expire.
    expires: Option<DateTime<Utc>>,
}

impl TryFrom<Claims> for PartnerClaims {
    /// Error type: [`anyhow::Error`].
    type Error = anyhow::Error;
    /// Narrows generic [`Claims`] down to the partner variant.
    ///
    /// # Errors
    ///
    /// Fails when `value` holds [`Claims::Client`] instead.
    fn try_from(value: Claims) -> Result<Self, Self::Error> {
        match value {
            Claims::Partner(p) => Ok(p),
            _ => anyhow::bail!("Invalid claim format"),
        }
    }
}

impl PartnerClaims {
    /// Builds partner claims from their parts.
    pub fn new(partner_id: String, partner_secret: String, expires: Option<DateTime<Utc>>) -> Self {
        Self {
            partner_id,
            partner_secret,
            expires,
        }
    }
}

/// JWT claims identifying an authenticated end customer.
///
/// Encoded as the [`Claims::Client`] variant. Client logins only check `ci`
/// plus first/last name (the verification digit is optional data), and the
/// resulting token always expires one hour after issue.
#[derive(Clone, Serialize, Deserialize)]
pub struct ClientClaims {
    /// `clients.id` of the authenticated customer, as a string.
    client_id: String,
    /// `clients.ci` of the authenticated customer, as a string.
    client_ci: String,
    /// Absolute expiry (login time + 1 hour).
    expires: DateTime<Utc>,
}

impl TryFrom<Claims> for ClientClaims {
    /// Error type: [`anyhow::Error`].
    type Error = anyhow::Error;
    /// Narrows generic [`Claims`] down to the client variant.
    ///
    /// # Errors
    ///
    /// Fails when `value` holds [`Claims::Partner`] instead.
    fn try_from(value: Claims) -> Result<Self, Self::Error> {
        match value {
            Claims::Client(c) => Ok(c),
            _ => anyhow::bail!("Invalid claim format"),
        }
    }
}

impl ClientClaims {
    /// Builds client claims from their parts.
    pub fn new(client_id: String, client_ci: String, expires: DateTime<Utc>) -> Self {
        Self {
            client_id,
            client_ci,
            expires,
        }
    }
}

/// Either actor that may hold a token: a [`Partner`](PartnerClaims) pharmacy
/// or a [`Client`](ClientClaims) customer.
#[derive(Clone, Serialize, Deserialize)]
pub enum Claims {
    /// Claims minted for a partner via `POST /api/v1/partners/login`.
    Partner(PartnerClaims),
    /// Claims minted for a client via `POST /api/v1/clients/login`.
    Client(ClientClaims),
}

impl From<ClientClaims> for Claims {
    /// Wraps client claims in the generic [`Claims`] envelope.
    fn from(value: ClientClaims) -> Self {
        Self::Client(value)
    }
}

impl From<PartnerClaims> for Claims {
    /// Wraps partner claims in the generic [`Claims`] envelope.
    fn from(value: PartnerClaims) -> Self {
        Self::Partner(value)
    }
}

/// Signs `claims` into a compact JWT with the `LAB_JWT_SECRET` secret.
///
/// # Errors
///
/// Fails when `LAB_JWT_SECRET` is unset or when `jsonwebtoken` cannot encode
/// the claims.
pub fn create_token(claims: Claims) -> anyhow::Result<String> {
    match claims {
        Claims::Partner(p) => {
            let secret = match std::env::var("LAB_JWT_SECRET") {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("{e:?}");
                    anyhow::bail!(e)
                }
            };
            match encode(
                &Header::default(),
                &p,
                &EncodingKey::from_secret(&secret.into_bytes()),
            ) {
                Ok(t) => {
                    tracing::info!("Token created");
                    return Ok(t);
                }
                Err(_) => {
                    tracing::error!("Failed to create token");
                    anyhow::bail!("Failed to create token")
                }
            }
        }
        Claims::Client(c) => {
            let secret = match std::env::var("LAB_JWT_SECRET") {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("{e:?}");
                    anyhow::bail!(e)
                }
            };
            match encode(
                &Header::default(),
                &c,
                &EncodingKey::from_secret(&secret.into_bytes()),
            ) {
                Ok(t) => {
                    tracing::info!("Token created");
                    return Ok(t);
                }
                Err(_) => {
                    tracing::error!("Failed to create token");
                    anyhow::bail!("Failed to create token")
                }
            }
        }
    }
}

/// Verifies a compact JWT against `LAB_JWT_SECRET` and returns its [`Claims`].
///
/// # Errors
///
/// Fails when `LAB_JWT_SECRET` is unset or when decoding/validation fails
/// (bad signature, malformed token, …).
pub fn validate_token(token: &str) -> anyhow::Result<Claims> {
    let secret = match std::env::var("LAB_JWT_SECRET") {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("{e:?}");
            anyhow::bail!(e)
        }
    };
    match decode(
        token,
        &DecodingKey::from_secret(&secret.into_bytes()),
        &Validation::default(),
    ) {
        Ok(d) => {
            return Ok(d.claims);
        }
        Err(e) => {
            tracing::error!("Failed to decode the token");
            tracing::error!("{e:?}");
            anyhow::bail!("Failed to decode the token")
        }
    }
}

/// Axum middleware enforcing `Authorization: Bearer <token>`.
///
/// On success the decoded [`Claims`] are inserted into the request
/// extensions for downstream handlers; otherwise the request is rejected
/// with [`StatusCode::UNAUTHORIZED`].
pub async fn jwt_middleware(mut req: Request, next: Next) -> Result<Response, StatusCode> {
    // 1. Get the Authorization header
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    // 2. Make sure it starts with "Bearer "
    let token = match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            &header["Bearer ".len()..] // slice off "Bearer " prefix
        }
        _ => {
            // No token or wrong format — reject with 401
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // 3. Validate the token
    match validate_token(token) {
        Ok(claims) => {
            // 4. Attach the claims to the request so handlers can use them
            req.extensions_mut().insert(claims);
            // 5. Pass the request to the next layer (your handler)
            Ok(next.run(req).await)
        }
        Err(_) => {
            // Invalid or expired token — reject with 401
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}
