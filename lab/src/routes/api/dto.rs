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
/// The partner authenticates with its natural key (`ruc`) plus the
/// pre-shared key whose Argon2 hash is stored in `partners.psk_hash`.
/// Unknown RUC, wrong PSK and inactive partners are all rejected; the
/// failure does not reveal which check failed, except inactive (403).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartnerLoginRequest {
    /// `partners.ruc` natural key of the pharmacy logging in.
    pub ruc: String,
    /// Pre-shared key; verified against `partners.psk_hash`, never stored.
    pub psk: String,
}

/// `POST /api/v1/clients/otp/request`
///
/// Starts the OTP login for the client identified by `ci`: a 6-digit code is
/// generated and dispatched over the client's SMS/email channel (see
/// [`crate::otp`]). `channel` optionally forces `"sms"` or `"email"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientOtpRequest {
    /// `clients.ci` (unique) of the customer requesting the code.
    pub ci: i32,
    /// Optional forced channel: `"sms"` or `"email"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

/// Response for `POST /api/v1/clients/otp/request`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientOtpResponse {
    /// Channel the code was sent over: `"sms"` or `"email"`.
    pub channel: String,
    /// Masked destination the code went to (never the full number/address).
    pub destination_masked: String,
    /// Seconds the code stays valid (see [`crate::otp::OTP_TTL_SECS`]).
    pub expires_in_secs: u64,
}

/// `POST /api/v1/clients/otp/verify`
///
/// Exchanges the 6-digit code for the client JWT (1-hour expiry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientOtpVerify {
    /// `clients.ci` (unique) the code was issued for.
    pub ci: i32,
    /// The 6-digit code from the SMS/email.
    pub code: String,
}

/// Successful login response for partner logins and OTP verifications.
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
/// suffix) is optional and never required. A new client needs at least one
/// contact (`phone_number` and/or `email`): the `clients` table requires a
/// reachable OTP channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketClientDto {
    /// `clients.ci` (unique): the lookup key for find-or-create.
    pub ci: i32,
    /// Optional `<ci>-<digit>` RUC suffix digit (`0-9`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_digit: Option<i32>,
    /// Buyer first name as printed on the factura.
    pub first_name: String,
    /// Buyer last name as printed on the factura.
    pub last_name: String,
    /// SMS channel for the client (required for new clients without email).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
    /// Email channel for the client (required for new clients without phone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// `POST /api/v1/partners/tickets`
///
/// Creates a ticket issued by `partner_id` for the invoice `ticket_id`
/// (the factura number, unique per partner), registering the buyer from the
/// ticket data when needed. `date` defaults to `current_timestamp`
/// server-side, so it is not part of the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTicketRequest {
    /// `partners.id` of the issuing pharmacy (must be active; must match the
    /// JWT actor).
    pub partner_id: i32,
    /// Invoice number from the factura (`varchar(64)`, unique per partner).
    pub ticket_id: String,
    /// Buyer identity from the factura; registered when unknown.
    pub client: TicketClientDto,
    /// At least one product line.
    pub details: Vec<TicketDetailDto>,
}

/// Response for `POST /api/v1/partners/tickets`.
///
/// `earned_points` is the snapshot posted to `point_ledger` for the new
/// ticket. `client_id` is the id of the existing or newly registered buyer.
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
/// entry: the original ticket rows stay intact, a `ticket_cancellations` row
/// is recorded, and awarded points are reversed with a compensating
/// `point_ledger` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteTicketRequest {
    /// `tickets.id` of the invoice to void.
    pub ticket_id: i32,
    /// Why the invoice is voided (`varchar(256)`, required).
    pub reason: String,
}

/// Response for `DELETE /api/v1/partners/tickets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteTicketResponse {
    /// `tickets.id` that was voided.
    pub ticket_id: i32,
    /// `ticket_cancellations.id` of the audit entry.
    pub cancellation_id: i32,
    /// Always `true` when this response is returned.
    pub deleted: bool,
}

// ============================================================
// Clients
// ============================================================

/// Public client profile.
///
/// Mirrors the `clients` table (`id`, `ci`, `verification_digit`,
/// `first_name`, `last_name`, `phone_number`, `email`). Contacts are the
/// OTP channels, so they are visible to the profile owner.
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
    /// `clients.phone_number` (SMS channel), if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_number: Option<String>,
    /// `clients.email` (email channel), if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Full ticket view embedded in the client dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketDto {
    /// `tickets.id` primary key (internal id, not the invoice number).
    pub id: i32,
    /// Invoice number from the factura (`tickets.ticket_id`).
    pub ticket_number: String,
    /// `tickets.date` issue timestamp.
    pub date: DateTime<Utc>,
    /// `partners.id` of the issuing pharmacy.
    pub partner_id: i32,
    /// `clients.id` of the buyer.
    pub client_id: i32,
    /// `ticket_details` lines of this ticket.
    pub details: Vec<TicketDetailDto>,
    /// Award snapshot from the ledger, if points were awarded.
    pub earned_points: Option<i32>,
    /// Whether the invoice was voided (original rows stay intact).
    pub cancelled: bool,
    /// Void reason when [`TicketDto::cancelled`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_reason: Option<String>,
}

/// `GET /api/v1/clients/{client_id}`
///
/// Everything the client dashboard needs: identity, point balance breakdown
/// and ticket history. All totals derive from `point_ledger` (the single
/// source of truth): `balance = earned − redeemed − cancelled`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientDataResponse {
    /// Profile of the requested client.
    pub client: ClientInfoDto,
    /// Points awarded by ticket purchases (ledger credits).
    pub total_earned_points: i32,
    /// Points spent on redemptions (ledger debits).
    pub total_redeemed_points: i32,
    /// Awarded points reversed by invoice cancellations (ledger debits).
    pub cancelled_points: i32,
    /// Spendable points: earned minus redeemed minus cancelled.
    pub balance_points: i32,
    /// Ticket history, oldest first (voided invoices included, flagged).
    pub tickets: Vec<TicketDto>,
}

// ============================================================
// Point redemption
// ============================================================

/// Single redemption line: `quantity` units of `product_id` exchanged for
/// points. The price is resolved through `redeemable_products`
/// (`quantity * points_needed`) and snapshotted per line, so later catalog
/// changes do not rewrite history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemItemDto {
    /// `products.id` of the prize to redeem.
    pub product_id: i32,
    /// Units to redeem (`quantity >= 1`).
    pub quantity: i32,
}

/// `POST /api/v1/clients/{client_id}/redeem_points`
///
/// `client_id` comes from the URL path, so the body only carries the items
/// to redeem. The redemption is persisted atomically: `redemptions` +
/// `redemption_items` rows, a negative `point_ledger` entry and the
/// `redemption_discounts` link, all in one transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemPointsRequest {
    /// At least one line to redeem.
    pub items: Vec<RedeemItemDto>,
}

/// Response for `POST /api/v1/clients/{client_id}/redeem_points`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemPointsResponse {
    /// `clients.id` the redemption was charged to.
    pub client_id: i32,
    /// `redemptions.id` of the persisted redemption.
    pub redemption_id: i32,
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
