//! # JWT authentication.
//!
//! Issues and validates the bearer tokens used by the REST API
//! ([`crate::routes::api`]). Two claim shapes exist — one per actor:
//!
//! * [`PartnerClaims`] for pharmacies ([`partners`](crate::models::Partners)),
//!   minted by `POST /api/v1/partners/login` with an 8-hour expiry.
//! * [`ClientClaims`] for end customers ([`clients`](crate::models::Clients)),
//!   minted by `POST /api/v1/clients/otp/verify` with a 1-hour expiry.
//!
//! Tokens are HS256-signed with the `LAB_JWT_SECRET` environment secret via
//! [`create_token`] and checked with [`validate_token`]. Authentication and
//! actor scoping live in the middleware layers [`partner_auth`] (pharmacy
//! routes), [`client_auth`] (customer API routes) and [`client_page_auth`]
//! (server-rendered client pages, which redirect browser navigations to the
//! login page instead of answering `401` JSON): each validates the bearer
//! token and additionally checks the token actor matches the targeted
//! resource, so handlers never see a mismatched actor.
//!
//! Claim payloads carry internal numeric ids only — never passwords, PSKs,
//! OTP codes or other secrets.
use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::{AppState, routes::api::dto::ErrorResponse};

/// JWT claims identifying an authenticated partner (pharmacy).
///
/// Encoded as the [`Claims::Partner`] variant. Carries the partner id plus
/// an absolute expiry only: the pre-shared key is verified at login and
/// never echoed into the token (JWT payloads are merely base64, not
/// encrypted).
#[derive(Clone, Serialize, Deserialize)]
pub struct PartnerClaims {
    /// `partners.id` of the authenticated pharmacy.
    partner_id: i32,
    /// Absolute expiry (login time + 8 hours).
    expires: DateTime<Utc>,
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
    pub fn new(partner_id: i32, expires: DateTime<Utc>) -> Self {
        Self {
            partner_id,
            expires,
        }
    }

    /// `partners.id` of the authenticated pharmacy.
    pub fn partner_id(&self) -> i32 {
        self.partner_id
    }
}

/// JWT claims identifying an authenticated end customer.
///
/// Encoded as the [`Claims::Client`] variant. The client proves ownership of
/// his SMS/email channel with a one-time code, and the resulting token
/// always expires one hour after issue.
#[derive(Clone, Serialize, Deserialize)]
pub struct ClientClaims {
    /// `clients.id` of the authenticated customer.
    client_id: i32,
    /// `clients.ci` of the authenticated customer.
    client_ci: i32,
    /// Absolute expiry (verification time + 1 hour).
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
    pub fn new(client_id: i32, client_ci: i32, expires: DateTime<Utc>) -> Self {
        Self {
            client_id,
            client_ci,
            expires,
        }
    }

    /// `clients.id` of the authenticated customer.
    pub fn client_id(&self) -> i32 {
        self.client_id
    }
}

/// Either actor that may hold a token: a [`Partner`](PartnerClaims) pharmacy
/// or a [`Client`](ClientClaims) customer.
///
/// Deserialized `untagged` because [`create_token`] signs the bare inner
/// claims object (e.g. `{"partner_id": ...}`), not an enveloped one.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
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
/// The bare inner claims object is signed (e.g. `{"partner_id": ...}`),
/// which is why [`Claims`] deserializes `untagged`.
///
/// # Errors
///
/// Fails when `LAB_JWT_SECRET` is unset or when `jsonwebtoken` cannot encode
/// the claims.
pub fn create_token(claims: Claims) -> anyhow::Result<String> {
    let secret = match std::env::var("LAB_JWT_SECRET") {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("{e:?}");
            anyhow::bail!(e)
        }
    };
    let payload = match &claims {
        Claims::Partner(p) => serde_json::to_value(p),
        Claims::Client(c) => serde_json::to_value(c),
    }
    .map_err(|_| anyhow::anyhow!("Failed to encode token claims"))?;
    match encode(
        &Header::default(),
        &payload,
        &EncodingKey::from_secret(secret.as_bytes()),
    ) {
        Ok(t) => {
            tracing::info!("Token created");
            Ok(t)
        }
        Err(_) => {
            tracing::error!("Failed to create token");
            anyhow::bail!("Failed to create token")
        }
    }
}

/// Verifies a compact JWT against `LAB_JWT_SECRET` and returns its [`Claims`].
///
/// Our tokens carry expiry in the custom `expires` field rather than the
/// standard `exp` claim, so stock `exp` validation is disabled and the
/// custom field is enforced instead: expired client tokens (minted with +1h)
/// and expired partner tokens (minted with +8h) are rejected.
///
/// # Errors
///
/// Fails when `LAB_JWT_SECRET` is unset, when decoding/validation fails
/// (bad signature, malformed token, …) or when the token has expired.
pub fn validate_token(token: &str) -> anyhow::Result<Claims> {
    let secret = match std::env::var("LAB_JWT_SECRET") {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("{e:?}");
            anyhow::bail!(e)
        }
    };
    let mut validation = Validation::default();
    validation.validate_exp = false;
    validation.required_spec_claims.clear();
    match decode(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    ) {
        Ok(d) => {
            let claims: Claims = d.claims;
            let expired = match &claims {
                Claims::Client(c) => c.expires < Utc::now(),
                Claims::Partner(p) => p.expires < Utc::now(),
            };
            if expired {
                tracing::warn!("validate_token: rejected (expired token)");
                anyhow::bail!("Token expired")
            }
            return Ok(claims);
        }
        Err(e) => {
            tracing::error!("Failed to decode the token");
            tracing::error!("{e:?}");
            anyhow::bail!("Failed to decode the token")
        }
    }
}

/// Rejection of an auth/scope middleware: the HTTP status plus a JSON
/// [`ErrorResponse`] body (same shape as every other API failure).
pub type AuthError = (StatusCode, Json<ErrorResponse>);

/// Builds a `401` [`AuthError`].
fn unauthorized(error: &'static str) -> AuthError {
    (StatusCode::UNAUTHORIZED, Json(ErrorResponse::new(error)))
}

/// Builds a `403` [`AuthError`].
fn forbidden(error: &'static str) -> AuthError {
    (StatusCode::FORBIDDEN, Json(ErrorResponse::new(error)))
}

/// Validates the `Authorization: Bearer <token>` header into [`Claims`].
///
/// # Errors
///
/// Returns `401` when the header is missing/malformed or the token is
/// invalid or expired.
fn bearer_claims(req: &Request) -> Result<Claims, AuthError> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
        .ok_or_else(|| unauthorized("missing token"))?;
    validate_token(token).map_err(|_| unauthorized("invalid token"))
}

/// Axum middleware for the pharmacy routes: bearer auth plus same-partner
/// scoping, in one choke point.
///
/// The token must carry [`Claims::Partner`]; anything else (client token,
/// missing or invalid token) is rejected. The targeted partner is then
/// checked against the token: for `POST` it comes from the JSON body's
/// `partner_id` (the body is buffered and rebuilt, so the handler still
/// receives it); for `DELETE` the body carries only `ticket_id`, so the
/// ticket owner is looked up in the DB.
///
/// Handlers behind this layer never see a mismatched actor.
pub async fn partner_auth(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let partner_id = match bearer_claims(&req)? {
        Claims::Partner(claims) => claims.partner_id(),
        Claims::Client(_) => return Err(forbidden("forbidden partner")),
    };
    if req.method() == axum::http::Method::POST {
        let (parts, body) = req.into_parts();
        let bytes = axum::body::to_bytes(body, 64 * 1024)
            .await
            .map_err(|_| unauthorized("unreadable body"))?;
        let body_partner: Option<i32> = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|body| {
                body.get("partner_id")
                    .and_then(serde_json::Value::as_i64)
                    .and_then(|id| id.try_into().ok())
            });
        if body_partner != Some(partner_id) {
            tracing::warn!("partner_auth: rejected (partner scope mismatch)");
            return Err(forbidden("forbidden partner"));
        }
        let req = Request::from_parts(parts, Body::from(bytes));
        return Ok(next.run(req).await);
    }
    if req.method() == axum::http::Method::DELETE {
        let (parts, body) = req.into_parts();
        let bytes = axum::body::to_bytes(body, 64 * 1024)
            .await
            .map_err(|_| unauthorized("unreadable body"))?;
        let ticket_id: Option<i32> = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|body| {
                body.get("ticket_id")
                    .and_then(serde_json::Value::as_i64)
                    .and_then(|id| id.try_into().ok())
            });
        let Some(ticket_id) = ticket_id else {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new("invalid body")),
            ));
        };
        let owner: Option<i32> = sqlx::query_scalar("SELECT partner_id FROM tickets WHERE id = $1")
            .bind(ticket_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| {
                tracing::error!("partner_auth: owner lookup failed: {e:?}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new("database error")),
                )
            })?;
        match owner {
            None => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse::new("ticket not found")),
                ));
            }
            Some(owner) if owner != partner_id => {
                tracing::warn!("partner_auth: rejected (partner scope mismatch)");
                return Err(forbidden("forbidden partner"));
            }
            _ => {}
        }
        let req = Request::from_parts(parts, Body::from(bytes));
        return Ok(next.run(req).await);
    }
    Ok(next.run(req).await)
}

/// Axum middleware for the customer routes: bearer auth plus same-client
/// scoping, in one choke point.
///
/// The token must carry [`Claims::Client`] whose id matches the `{client_id}`
/// path segment (`/api/v1/clients/{client_id}` and
/// `/api/v1/clients/{client_id}/redeem_points`); anything else is rejected.
///
/// Handlers behind this layer never see a mismatched actor.
pub async fn client_auth(req: Request, next: Next) -> Result<Response, AuthError> {
    let client_id = match bearer_claims(&req)? {
        Claims::Client(claims) => claims.client_id(),
        Claims::Partner(_) => return Err(forbidden("forbidden client")),
    };
    let target: Option<i32> = req
        .uri()
        .path()
        .strip_prefix("/api/v1/clients/")
        .and_then(|rest| rest.split('/').next())
        .and_then(|segment| segment.parse().ok());
    if target != Some(client_id) {
        tracing::warn!("client_auth: rejected (client scope mismatch)");
        return Err(forbidden("forbidden client"));
    }
    Ok(next.run(req).await)
}

/// Cookie carrying the client JWT for browser page navigations (`GET
/// `/clients/{client_id}/redeem`). Set by `POST
/// /api/v1/clients/otp/verify` (`Set-Cookie`) alongside the JSON token body
/// the redeem page keeps in `localStorage` for API calls; both hold the same
/// 1-hour token and either one satisfies [`client_page_auth`].
pub const CLIENT_SESSION_COOKIE: &str = "lab_client_token";

/// Extracts the client JWT from a page request: the `Authorization: Bearer`
/// header first, then the [`CLIENT_SESSION_COOKIE`] cookie browsers send
/// automatically on navigation (plain page `GET`s carry no `Authorization`
/// header because the token lives in `localStorage`).
fn page_bearer_token(req: &Request) -> Option<String> {
    if let Some(token) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
    {
        return Some(token.to_string());
    }
    let prefix = format!("{CLIENT_SESSION_COOKIE}=");
    req.headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(&prefix))
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

/// Builds a `303 See Other` redirect to the client login page, preserving the
/// intercepted path in `?next=` so the login page can send the client back
/// after verifying.
fn login_redirect(path: &str) -> Response {
    let mut headers = header::HeaderMap::new();
    let location = format!("/clients/login?next={path}");
    let location: header::HeaderValue = location
        .parse()
        .unwrap_or_else(|_| header::HeaderValue::from_static("/clients/login"));
    headers.insert(header::LOCATION, location);
    (StatusCode::SEE_OTHER, headers, "").into_response()
}

/// Axum middleware for the server-rendered client pages: same-client bearer
/// auth, with a login redirect for browser navigations.
///
/// The token comes from the `Authorization` header or the
/// [`CLIENT_SESSION_COOKIE`] cookie, and must be a [`Claims::Client`] whose
/// id matches the `/clients/{client_id}/redeem` path. Authenticated requests
/// pass through; unauthenticated `GET` navigations get `303 See Other` to
/// `/clients/login?next=<path>`, while non-`GET` requests without auth get a
/// `401` JSON body (same shape as the API).
pub async fn client_page_auth(req: Request, next: Next) -> Response {
    let target: Option<i32> = req
        .uri()
        .path()
        .strip_prefix("/clients/")
        .and_then(|rest| rest.split('/').next())
        .and_then(|segment| segment.parse().ok());
    let authorized = page_bearer_token(&req)
        .and_then(|token| validate_token(&token).ok())
        .and_then(|claims| ClientClaims::try_from(claims).ok())
        .is_some_and(|claims| target == Some(claims.client_id()));
    if authorized {
        return next.run(req).await;
    }
    tracing::warn!("client_page_auth: rejected (unauthenticated page view)");
    if req.method() == axum::http::Method::GET {
        return login_redirect(req.uri().path());
    }
    unauthorized("missing token").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, middleware, routing::get};
    use tower::ServiceExt as _;

    /// Stub app mirroring the frontend wiring: one page route behind
    /// [`client_page_auth`], with a stub handler so accept-path tests never
    /// touch the database.
    fn test_app() -> Router {
        Router::new()
            .route("/clients/{client_id}/redeem", get(|| async { "ok" }))
            .route_layer(middleware::from_fn(client_page_auth))
    }

    /// Mints a 1-hour client JWT with `LAB_JWT_SECRET=test-secret`.
    fn client_token(client_id: i32) -> String {
        unsafe { std::env::set_var("LAB_JWT_SECRET", "test-secret") };
        create_token(
            ClientClaims::new(client_id, 12_345, Utc::now() + chrono::Duration::hours(1)).into(),
        )
        .unwrap()
    }

    /// Mints an 8-hour partner JWT with `LAB_JWT_SECRET=test-secret`.
    fn partner_token() -> String {
        unsafe { std::env::set_var("LAB_JWT_SECRET", "test-secret") };
        create_token(PartnerClaims::new(1, Utc::now() + chrono::Duration::hours(8)).into()).unwrap()
    }

    /// Sends a `GET` page request through the stub app.
    async fn get_page(app: Router, uri: &str, headers: &[(&str, String)]) -> Response {
        let mut builder = Request::builder().method("GET").uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, value.clone());
        }
        app.oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// Unauthenticated page views bounce to the login page with `?next=`.
    #[tokio::test]
    async fn redirects_unauthenticated_get_to_login() {
        let res = get_page(test_app(), "/clients/1/redeem", &[]).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            res.headers().get(header::LOCATION).unwrap(),
            "/clients/login?next=/clients/1/redeem"
        );
    }

    /// Garbage cookies bounce just like missing ones.
    #[tokio::test]
    async fn rejects_garbage_cookie() {
        let res = get_page(
            test_app(),
            "/clients/1/redeem",
            &[(
                header::COOKIE.as_str(),
                "lab_client_token=garbage".to_string(),
            )],
        )
        .await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
    }

    /// A client token for another id bounces (same-client scoping).
    #[tokio::test]
    async fn rejects_scope_mismatch() {
        let cookie = format!("lab_client_token={}", client_token(2));
        let res = get_page(
            test_app(),
            "/clients/1/redeem",
            &[(header::COOKIE.as_str(), cookie)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            res.headers().get(header::LOCATION).unwrap(),
            "/clients/login?next=/clients/1/redeem"
        );
    }

    /// Partner tokens do not open client pages.
    #[tokio::test]
    async fn rejects_partner_token() {
        let cookie = format!("lab_client_token={}", partner_token());
        let res = get_page(
            test_app(),
            "/clients/1/redeem",
            &[(header::COOKIE.as_str(), cookie)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
    }

    /// A matching session cookie renders the page.
    #[tokio::test]
    async fn passes_matching_cookie() {
        let cookie = format!("lab_client_token={}", client_token(1));
        let res = get_page(
            test_app(),
            "/clients/1/redeem",
            &[(header::COOKIE.as_str(), cookie)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    /// The `Authorization` header transport works without any cookie.
    #[tokio::test]
    async fn passes_bearer_header() {
        let auth = format!("Bearer {}", client_token(1));
        let res = get_page(
            test_app(),
            "/clients/1/redeem",
            &[(header::AUTHORIZATION.as_str(), auth)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
    }
}
