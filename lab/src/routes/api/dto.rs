/// # REST API Data Transfer Objects (DTOs).
///
/// Request/response types for the JSON REST API declared in
/// [`crate::routes::api`](super). They are intentionally decoupled from the
/// database models in `crate::models` so the wire format can evolve
/// independently from the schema in `lab/database/schema.sql`.
///
/// Every DTO derives `Clone`, `Serialize` and `Deserialize`.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================
// Auth
// ============================================================

/// `POST /api/v1/partners/login`
///
/// `partners` has no password column yet, so the partner authenticates with
/// its natural key (`ruc`). `secret` is an optional pre-shared credential
/// that maps to [`crate::jwt::PartnerClaims::partner_secret`]; it is
/// `None` when the partner has no secret configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartnerLoginRequest {
    pub ruc: String,
    pub secret: Option<String>,
}

/// `POST /api/v1/clients/login`
///
/// Clients are uniquely identified by `ci` alone (`ci` is `UNIQUE`).
/// The government-issued verification digit (`<ci>-<digit>` RUC suffix) is
/// optional data and is NOT required to identify the client nor to generate
/// a ticket, so login only checks `ci` + `first_name` + `last_name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientLoginRequest {
    pub ci: i32,
    pub first_name: String,
    pub last_name: String,
}

/// Successful login response for both partner and client logins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    #[serde(default = "default_token_type")]
    pub token_type: String,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl LoginResponse {
    pub fn bearer(token: String) -> Self {
        Self {
            token,
            token_type: default_token_type(),
        }
    }
}

// ============================================================
// Tickets
// ============================================================

/// Single line of a ticket/invoice.
///
/// Maps to one row of `ticket_details` (`ticket_id` is implied by the
/// enclosing request/response).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketDetailDto {
    pub product_id: i32,
    pub quantity: i32,
}

/// Buyer identity printed on the ticket/invoice.
///
/// The pharmacy submits the buyer data as it appears on the factura. The
/// client is looked up by `ci` (unique) and registered on the fly when
/// unknown. The government-issued `verification_digit` (`<ci>-<digit>` RUC
/// suffix) is optional and never required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketClientDto {
    pub ci: i32,
    pub verification_digit: Option<i32>,
    pub first_name: String,
    pub last_name: String,
}

/// `POST /api/v1/partners/tickets`
///
/// Creates a ticket issued by `partner_id` with the given product lines,
/// registering the buyer from the ticket data when needed. `date` defaults
/// to `current_timestamp` server-side, so it is not part of the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTicketRequest {
    pub partner_id: i32,
    pub client: TicketClientDto,
    pub details: Vec<TicketDetailDto>,
}

/// Response for `POST /api/v1/partners/tickets`.
///
/// `earned_points` is the snapshot stored in `point_earnings` for the new
/// ticket (see schema normalization notes). `client_id` is the id of the
/// existing or newly registered buyer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTicketResponse {
    pub ticket_id: i32,
    pub client_id: i32,
    pub earned_points: i32,
}

/// `DELETE /api/v1/partners/tickets`
///
/// The route carries no path parameter, so the ticket to void is identified
/// in the request body. Per the schema notes, cancellation is an audit
/// entry: the original ticket rows stay intact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteTicketRequest {
    pub ticket_id: i32,
}

/// Response for `DELETE /api/v1/partners/tickets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteTicketResponse {
    pub ticket_id: i32,
    pub deleted: bool,
}

// ============================================================
// Clients
// ============================================================

/// Public client profile.
///
/// Mirrors the `clients` table (`id`, `ci`, `verification_digit`,
/// `first_name`, `last_name`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientInfoDto {
    pub id: i32,
    pub ci: i32,
    pub verification_digit: Option<i32>,
    pub first_name: String,
    pub last_name: String,
}

/// Full ticket view embedded in the client dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketDto {
    pub id: i32,
    pub date: DateTime<Utc>,
    pub partner_id: i32,
    pub client_id: i32,
    pub details: Vec<TicketDetailDto>,
    /// Snapshot from `point_earnings`, if points were awarded.
    pub earned_points: Option<i32>,
}

/// `GET /api/v1/clients/{client_id}`
///
/// Everything the client dashboard needs to render the points-redemption
/// view: identity, point balance breakdown, and ticket history.
/// `total_redeemed_points` is derived as
/// `quantity * redeemable_products.points_needed` (never stored), while
/// `total_earned_points` sums the `point_earnings` snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientDataResponse {
    pub client: ClientInfoDto,
    pub total_earned_points: i32,
    pub total_redeemed_points: i32,
    pub balance_points: i32,
    pub tickets: Vec<TicketDto>,
}

// ============================================================
// Point redemption
// ============================================================

/// Single redemption line: `quantity` units of `product_id` exchanged for
/// points. The points cost is derived via a join on `redeemable_products`
/// (`quantity * points_needed`), never stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemItemDto {
    pub product_id: i32,
    pub quantity: i32,
}

/// `POST /api/v1/clients/{client_id}/redeem_points`
///
/// `client_id` comes from the URL path, so the body only carries the items
/// to redeem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemPointsRequest {
    pub items: Vec<RedeemItemDto>,
}

/// Response for `POST /api/v1/clients/{client_id}/redeem_points`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemPointsResponse {
    pub client_id: i32,
    pub redeemed_points: i32,
    pub remaining_points: i32,
}

// ============================================================
// Errors
// ============================================================

/// Generic JSON error body for failed API calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ErrorResponse {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: None,
        }
    }

    pub fn with_message(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: Some(message.into()),
        }
    }
}
