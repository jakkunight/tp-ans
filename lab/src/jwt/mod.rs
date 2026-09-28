use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct PartnerClaims {
    partner_id: String,
    partner_secret: String,
    expires: Option<DateTime<Utc>>,
}

impl TryFrom<Claims> for PartnerClaims {
    type Error = anyhow::Error;
    fn try_from(value: Claims) -> Result<Self, Self::Error> {
        match value {
            Claims::Partner(p) => Ok(p),
            _ => anyhow::bail!("Invalid claim format"),
        }
    }
}

impl PartnerClaims {
    pub fn new(partner_id: String, partner_secret: String, expires: Option<DateTime<Utc>>) -> Self {
        Self {
            partner_id,
            partner_secret,
            expires,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ClientClaims {
    client_id: String,
    client_ci: String,
    expires: DateTime<Utc>,
}

impl TryFrom<Claims> for ClientClaims {
    type Error = anyhow::Error;
    fn try_from(value: Claims) -> Result<Self, Self::Error> {
        match value {
            Claims::Client(c) => Ok(c),
            _ => anyhow::bail!("Invalid claim format"),
        }
    }
}

impl ClientClaims {
    pub fn new(client_id: String, client_ci: String, expires: DateTime<Utc>) -> Self {
        Self {
            client_id,
            client_ci,
            expires,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Claims {
    Partner(PartnerClaims),
    Client(ClientClaims),
}

impl From<ClientClaims> for Claims {
    fn from(value: ClientClaims) -> Self {
        Self::Client(value)
    }
}

impl From<PartnerClaims> for Claims {
    fn from(value: PartnerClaims) -> Self {
        Self::Partner(value)
    }
}

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
