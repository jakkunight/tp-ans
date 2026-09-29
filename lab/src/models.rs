/// # Data models derived from the database schema.
/// Each model derives `Serialize`, `Deserialize`, `Clone`, `Debug`,
/// `PartialEq`, `PartialOrd` and `FromRow` from `sqlx::FromRow`.
///
/// See `lab/database/schema.sql` for the full schema.
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use chrono::{DateTime, Utc};

// ============================================================
/// ## Partners
///
/// **Schema:**
/// ```sql
/// create table partners (
///     id serial not null primary key,
///     name varchar(32) not null,
///     ruc varchar(32) not null unique,
///     psk_hash text not null,
///     is_active boolean not null default true
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Partners {
    /// `partners.id` primary key.
    pub id: i32,
    /// `partners.name` display name (`varchar(32)`).
    pub name: String,
    /// `partners.ruc` tax id (`varchar(32)`, unique), the login natural key.
    pub ruc: String,
    /// Argon2 PHC hash of the partner pre-shared key (`psk_hash`).
    ///
    /// Never serialized to API responses; only read for login verification
    /// (see [`crate::db::authenticate_partner`]).
    pub psk_hash: String,
    /// `partners.is_active`; inactive partners cannot log in or bill.
    pub is_active: bool,
}

// ============================================================
/// ## Clients
///
/// **Schema:**
/// ```sql
/// create table clients (
///     id serial not null primary key,
///     ci int not null unique check (ci >= 0),
///     verification_digit int null check (
///         verification_digit is null
///         or (verification_digit >= 0 and verification_digit <= 9)
///     ),
///     first_name varchar(32) not null,
///     last_name varchar(32) not null,
///     phone_number varchar(13) null check (
///         email is not null or phone_number is not null
///     ),
///     email varchar(128) null check (
///         phone_number is not null or email is not null
///     )
/// );
/// ```
///
/// Every client has at least one OTP channel: `phone_number` (SMS),
/// `email`, or both.
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Clients {
    /// `clients.id` primary key.
    pub id: i32,
    /// `clients.ci` national id, unique and `>= 0`; identifies the client alone.
    pub ci: i32,
    /// Optional `<ci>-<digit>` RUC suffix digit (`0-9`); never required.
    pub verification_digit: Option<i32>,
    /// `clients.first_name` (`varchar(32)`).
    pub first_name: String,
    /// `clients.last_name` (`varchar(32)`).
    pub last_name: String,
    /// `clients.phone_number` (`varchar(13)`); SMS channel for OTP codes.
    pub phone_number: Option<String>,
    /// `clients.email` (`varchar(128)`); email channel for OTP codes.
    pub email: Option<String>,
}

// ============================================================
/// ## Products
///
/// **Schema:**
/// ```sql
/// create table products (
///     id serial not null primary key,
///     name varchar(64) not null,
///     description varchar(128) not null default 'N/A'
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Products {
    /// `products.id` primary key.
    pub id: i32,
    /// `products.name` (`varchar(64)`).
    pub name: String,
    /// `products.description` (`varchar(128)`, defaults to `'N/A'`).
    pub description: String,
}

// ============================================================
/// ## PromotionProducts
///
/// **Schema:**
/// ```sql
/// create table promotion_products (
///     id serial not null primary key,
///     product_id int not null unique references products(id),
///     points_cost int not null check (points_cost >= 1)
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct PromotionProducts {
    /// `promotion_products.id` primary key.
    pub id: i32,
    /// `products.id` whose purchase earns points.
    pub product_id: i32,
    /// Points awarded per purchased unit (`>= 1`).
    pub points_cost: i32,
}

// ============================================================
/// ## RedeemableProducts
///
/// **Schema:**
/// ```sql
/// create table redeemable_products (
///     id serial not null primary key,
///     product_id int not null unique references products(id),
///     points_needed int not null check (points_needed >= 1)
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct RedeemableProducts {
    /// `redeemable_products.id` primary key.
    pub id: i32,
    /// `products.id` that can be exchanged for points.
    pub product_id: i32,
    /// Points charged per redeemed unit (`>= 1`).
    pub points_needed: i32,
}

// ============================================================
/// ## Tickets
///
/// **Schema:**
/// ```sql
/// create table tickets (
///     id serial not null primary key,
///     ticket_id varchar(64) not null,
///     date timestamptz not null default current_timestamp,
///     partner_id int not null references partners(id),
///     client_id int not null references clients(id),
///     unique (partner_id, ticket_id)
/// );
/// ```
///
/// `ticket_id` is the invoice number printed on the factura, scoped per
/// partner (`unique (partner_id, ticket_id)`): two pharmacies may reuse the
/// same number, one pharmacy may not.
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Tickets {
    /// `tickets.id` primary key (internal; never the invoice number).
    pub id: i32,
    /// `tickets.ticket_id`: invoice number from the factura (`varchar(64)`).
    pub ticket_id: String,
    /// `tickets.date` issue timestamp (defaults to `current_timestamp`).
    pub date: DateTime<Utc>,
    /// `partners.id` of the issuing pharmacy.
    pub partner_id: i32,
    /// `clients.id` of the buyer.
    pub client_id: i32,
}

// ============================================================
/// ## TicketDetails
///
/// **Schema:**
/// ```sql
/// create table ticket_details (
///     id serial not null primary key,
///     ticket_id int not null references tickets(id),
///     product_id int not null references products(id),
///     quantity int not null check (quantity >= 1),
///     unique (ticket_id, product_id)
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct TicketDetails {
    /// `ticket_details.id` primary key.
    pub id: i32,
    /// Owning `tickets.id` (unique per `product_id`).
    pub ticket_id: i32,
    /// Purchased `products.id`.
    pub product_id: i32,
    /// Units purchased (`quantity >= 1`).
    pub quantity: i32,
}

// ============================================================
/// ## TicketCancellations
///
/// **Schema:**
/// ```sql
/// create table ticket_cancellations (
///     id serial primary key,
///     ticket_id int not null unique references tickets(id),
///     date timestamptz not null default current_timestamp,
///     reason varchar(256) not null
/// );
/// ```
///
/// Cancellation is an audit entry: the original ticket rows stay intact and
/// the awarded points are reversed with a compensating [`PointLedger`] row
/// (see [`TicketCancellationDiscounts`]).
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct TicketCancellations {
    /// `ticket_cancellations.id` primary key.
    pub id: i32,
    /// Voided `tickets.id` (a ticket can only be cancelled once).
    pub ticket_id: i32,
    /// `ticket_cancellations.date` (defaults to `current_timestamp`).
    pub date: DateTime<Utc>,
    /// Why the invoice was voided (`varchar(256)`).
    pub reason: String,
}

// ============================================================
/// ## PointLedger
///
/// **Schema:**
/// ```sql
/// create table point_ledger (
///     id serial not null primary key,
///     client_id int not null references clients(id),
///     points_delta int not null check (points_delta <> 0),
///     date timestamptz not null default current_timestamp
/// );
/// ```
///
/// Single source of truth for balances: a client's spendable points are
/// `SUM(points_delta)`. Positive rows award points (ticket earnings),
/// negative rows take them away (redemptions, cancellation reversals).
/// Every row links to exactly one event row ([`TicketEarnings`],
/// [`RedemptionDiscounts`] or [`TicketCancellationDiscounts`]).
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct PointLedger {
    /// `point_ledger.id` primary key.
    pub id: i32,
    /// `clients.id` whose balance moves.
    pub client_id: i32,
    /// Signed movement (`<> 0`): `> 0` awards, `< 0` removes points.
    pub points_delta: i32,
    /// `point_ledger.date` (defaults to `current_timestamp`).
    pub date: DateTime<Utc>,
}

// ============================================================
/// ## TicketCancellationDiscounts
///
/// **Schema:**
/// ```sql
/// create table ticket_cancellation_discounts (
///     id serial not null primary key,
///     point_ledger_id int not null references point_ledger(id),
///     cancellation_id int not null references ticket_cancellations(id),
///     unique (point_ledger_id, cancellation_id)
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct TicketCancellationDiscounts {
    /// `ticket_cancellation_discounts.id` primary key.
    pub id: i32,
    /// Compensating (negative) [`PointLedger`] row reversing the award.
    pub point_ledger_id: i32,
    /// [`TicketCancellations`] row the reversal belongs to.
    pub cancellation_id: i32,
}

// ============================================================
/// ## TicketEarnings
///
/// **Schema:**
/// ```sql
/// create table ticket_earnings (
///     id serial not null primary key,
///     point_ledger_id int not null references point_ledger(id),
///     ticket_id int not null references tickets(id),
///     unique (point_ledger_id, ticket_id)
/// );
/// ```
///
/// Links a positive [`PointLedger`] award to the ticket that earned it. The
/// awarded amount is the snapshot stored on the ledger row (rates can change
/// over time, so the event amount is frozen at issue time).
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct TicketEarnings {
    /// `ticket_earnings.id` primary key.
    pub id: i32,
    /// Awarding (positive) [`PointLedger`] row.
    pub point_ledger_id: i32,
    /// `tickets.id` whose purchase earned the points.
    pub ticket_id: i32,
}

// ============================================================
/// ## Redemptions
///
/// **Schema:**
/// ```sql
/// create table redemptions (
///     id serial not null primary key,
///     client_id int not null references clients(id),
///     redeemed_points int not null check (redeemed_points >= 1),
///     date timestamptz not null default current_timestamp
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Redemptions {
    /// `redemptions.id` primary key.
    pub id: i32,
    /// `clients.id` spending the points.
    pub client_id: i32,
    /// Total cost (`>= 1`).
    pub redeemed_points: i32,
    /// `redemptions.date` (defaults to `current_timestamp`).
    pub date: DateTime<Utc>,
}

// ============================================================
/// ## RedemptionItems
///
/// **Schema:**
/// ```sql
/// create table redemption_items (
///     id serial not null primary key,
///     redemption_id int not null references redemptions(id),
///     product_id int not null unique references redeemable_products(
///         id
///     ) on delete cascade,
///     quantity int not null check (quantity >= 1),
///     points_per_unit int not null check (points_per_unit > 0)
/// );
/// ```
///
/// NOTE: despite its name, `product_id` references
/// [`redeemable_products.id`](RedeemableProducts) (the catalog entry), not
/// [`products.id`](Products). The API still accepts `products.id` lines and
/// [`crate::db`] resolves them to the catalog entry. `points_per_unit`
/// snapshots the price at redemption time so later catalog changes do not
/// rewrite history.
///
/// NOTE: the schema declares `product_id` globally `unique`, so a catalog
/// entry can only ever appear in a single redemption row. That looks like a
/// schema bug (probably `unique (redemption_id, product_id)` was intended);
/// the DB layer surfaces the resulting conflict as HTTP `409`.
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct RedemptionItems {
    /// `redemption_items.id` primary key.
    pub id: i32,
    /// Owning [`Redemptions`] row.
    pub redemption_id: i32,
    /// Redeemed [`RedeemableProducts`] catalog entry id.
    pub product_id: i32,
    /// Units redeemed (`quantity >= 1`).
    pub quantity: i32,
    /// Price snapshot per unit (`> 0`).
    pub points_per_unit: i32,
}

// ============================================================
/// ## RedemptionDiscounts
///
/// **Schema:**
/// ```sql
/// create table redemption_discounts (
///     id serial not null primary key,
///     point_ledger_id int not null references point_ledger(id),
///     redemption_id int not null references redemptions(id),
///     unique (point_ledger_id, redemption_id)
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct RedemptionDiscounts {
    /// `redemption_discounts.id` primary key.
    pub id: i32,
    /// Charging (negative) [`PointLedger`] row.
    pub point_ledger_id: i32,
    /// [`Redemptions`] row the charge belongs to.
    pub redemption_id: i32,
}
