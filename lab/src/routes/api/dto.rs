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
    /// `partners.ruc` natural key of the pharmacy logging in.
    pub ruc: String,
    /// Optional pre-shared credential, echoed into the JWT claims.
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
    /// `clients.ci` (unique): enough on its own to identify the client.
    pub ci: i32,
    /// Must match `clients.first_name` exactly.
    pub first_name: String,
    /// Must match `clients.last_name` exactly.
    pub last_name: String,
}

/// Successful login response for both partner and client logins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginResponse {
    /// Compact JWT to send back as `Authorization: Bearer <token>`.
    pub token: String,
    /// Token scheme, always `"Bearer"`.
    #[serde(default = "default_token_type")]
    pub token_type: String,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl LoginResponse {
    /// Builds a `"Bearer"`-typed response around a freshly minted `token`.
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
    /// `products.id` of the purchased product.
    pub product_id: i32,
    /// Units purchased (`quantity >= 1`).
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
    /// `clients.ci` (unique): the lookup key for find-or-create.
    pub ci: i32,
    /// Optional `<ci>-<digit>` RUC suffix digit (`0-9`).
    pub verification_digit: Option<i32>,
    /// Buyer first name as printed on the factura.
    pub first_name: String,
    /// Buyer last name as printed on the factura.
    pub last_name: String,
}

/// `POST /api/v1/partners/tickets`
///
/// Creates a ticket issued by `partner_id` with the given product lines,
/// registering the buyer from the ticket data when needed. `date` defaults
/// to `current_timestamp` server-side, so it is not part of the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTicketRequest {
    /// `partners.id` of the issuing pharmacy (must be active).
    pub partner_id: i32,
    /// Buyer identity from the factura; registered when unknown.
    pub client: TicketClientDto,
    /// At least one product line.
    pub details: Vec<TicketDetailDto>,
}

/// Response for `POST /api/v1/partners/tickets`.
///
/// `earned_points` is the snapshot stored in `point_earnings` for the new
/// ticket (see schema normalization notes). `client_id` is the id of the
/// existing or newly registered buyer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTicketResponse {
    /// `tickets.id` of the newly created ticket.
    pub ticket_id: i32,
    /// `clients.id` of the existing or newly registered buyer.
    pub client_id: i32,
    /// Points awarded (`0` when the ticket earned none).
    pub earned_points: i32,
}

/// `DELETE /api/v1/partners/tickets`
///
/// The route carries no path parameter, so the ticket to void is identified
/// in the request body. Per the schema notes, cancellation is an audit
/// entry: the original ticket rows stay intact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteTicketRequest {
    /// `tickets.id` of the invoice to void.
    pub ticket_id: i32,
}

/// Response for `DELETE /api/v1/partners/tickets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteTicketResponse {
    /// `tickets.id` that was voided.
    pub ticket_id: i32,
    /// Always `true` when this response is returned.
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
    /// `clients.id` primary key.
    pub id: i32,
    /// `clients.ci` national id (unique).
    pub ci: i32,
    /// Optional `<ci>-<digit>` RUC suffix digit.
    pub verification_digit: Option<i32>,
    /// `clients.first_name`.
    pub first_name: String,
    /// `clients.last_name`.
    pub last_name: String,
}

/// Full ticket view embedded in the client dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketDto {
    /// `tickets.id` primary key.
    pub id: i32,
    /// `tickets.date` issue timestamp.
    pub date: DateTime<Utc>,
    /// `partners.id` of the issuing pharmacy.
    pub partner_id: i32,
    /// `clients.id` of the buyer.
    pub client_id: i32,
    /// `ticket_details` lines of this ticket.
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
    /// Profile of the requested client.
    pub client: ClientInfoDto,
    /// Sum of `point_earnings` snapshots over his tickets.
    pub total_earned_points: i32,
    /// Derived redeemed total (`0` until a `redemptions` table exists).
    pub total_redeemed_points: i32,
    /// Spendable points: earned minus redeemed.
    pub balance_points: i32,
    /// Ticket history, oldest first.
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
    /// `products.id` of the `redeemable_products` entry.
    pub product_id: i32,
    /// Units to redeem (`quantity >= 1`).
    pub quantity: i32,
}

/// `POST /api/v1/clients/{client_id}/redeem_points`
///
/// `client_id` comes from the URL path, so the body only carries the items
/// to redeem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemPointsRequest {
    /// At least one line to redeem.
    pub items: Vec<RedeemItemDto>,
}

/// Response for `POST /api/v1/clients/{client_id}/redeem_points`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemPointsResponse {
    /// `clients.id` the redemption was priced for.
    pub client_id: i32,
    /// Total cost (`Σ quantity × points_needed`).
    pub redeemed_points: i32,
    /// Balance left after the redemption.
    pub remaining_points: i32,
}

// ============================================================
// Errors
// ============================================================

/// Generic JSON error body for failed API calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Machine-readable error code.
    pub error: String,
    /// Optional human-readable detail; omitted from JSON when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ErrorResponse {
    /// Builds an error body with no detail message.
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: None,
        }
    }

    /// Builds an error body with a human-readable detail message.
    pub fn with_message(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: Some(message.into()),
        }
    }
}
