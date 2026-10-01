/// # Data models derived from the database schema.
/// Each model derives `Serialize`, `Deserialize`, `Clone`, `Debug`,
/// `PartialEq`, `PartialOrd` and `FromRow` from `sqlx::FromRow`.
///
/// Fiscal identity (invoice number, timbrado, CDC, RUC) is validated by the
/// value types in [`crate::fiscal`] (see `lab/database/schema.sql` CHECKs).
/// Prices, IVA, sale conditions and totals are intentionally absent: they are
/// irrelevant for the loyalty-points domain.
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
///     ruc varchar(32) not null unique check (ruc ~ '^[0-9]{1,8}-[0-9]$'),
///     psk_hash text not null,
///     is_active boolean not null default true,
///     domicilio varchar(128) not null default 'N/A',
///     actividad_economica varchar(64) not null default 'N/A'
/// );
/// ```
///
/// Emisor fiscal: `name` is the razón social, `ruc` is `base-DV` validated
/// with [`crate::fiscal::Ruc`] (mod-11), plus the address (`domicilio`) and
/// the economic activity printed on the factura.
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Partners {
    /// `partners.id` primary key.
    pub id: i32,
    /// `partners.name` razón social (`varchar(32)`).
    pub name: String,
    /// `partners.ruc` `base-DV` (`varchar(32)`, unique), the login natural key.
    pub ruc: String,
    /// Argon2 PHC hash of the partner pre-shared key (`psk_hash`).
    ///
    /// Never serialized to API responses; only read for login verification
    /// (see [`crate::db::authenticate_partner`]).
    pub psk_hash: String,
    /// `partners.is_active`; inactive partners cannot log in or bill.
    pub is_active: bool,
    /// `partners.domicilio` fiscal address printed on the factura.
    pub domicilio: String,
    /// `partners.actividad_economica` activity printed on the factura.
    pub actividad_economica: String,
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
///     razon_social varchar(64) null,
///     domicilio varchar(128) null,
///     phone_number varchar(13) null check (
///         email is not null or phone_number is not null
///     ),
///     email varchar(128) null check (
///         phone_number is not null or email is not null
///     )
/// );
/// ```
///
/// Receptor fiscal: personas físicas use `first_name`/`last_name` + CI;
/// personas jurídicas use `razon_social` + RUC (`ci`-`verification_digit`,
/// DV verified with [`crate::fiscal::Ruc`]). `domicilio` is the address
/// printed on the factura. Every client has at least one OTP channel:
/// `phone_number` (SMS), `email`, or both.
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Clients {
    /// `clients.id` primary key.
    pub id: i32,
    /// `clients.ci` national id / RUC base, unique and `>= 0`.
    pub ci: i32,
    /// RUC check digit: `<ci>-<digit>`; when present its mod-11 must match
    /// `ci` (see [`crate::fiscal::Ruc`]).
    pub verification_digit: Option<i32>,
    /// `clients.first_name` (`varchar(32)`).
    pub first_name: String,
    /// `clients.last_name` (`varchar(32)`).
    pub last_name: String,
    /// `clients.razon_social` company name for juridical receptors, if any.
    pub razon_social: Option<String>,
    /// `clients.domicilio` fiscal address printed on the factura, if known.
    pub domicilio: Option<String>,
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
///     ticket_id varchar(64) not null check (
///         ticket_id ~ '^[0-9]{3}-[0-9]{3}-[0-9]{7}$'
///     ),
///     timbrado char(8) null check (timbrado ~ '^[0-9]{8}$'),
///     cdc char(44) null unique check (cdc ~ '^[0-9]{44}$'),
///     date timestamptz not null default current_timestamp,
///     partner_id int not null references partners(id),
///     client_id int not null references clients(id),
///     unique (partner_id, ticket_id),
///     check (cdc is null or timbrado is not null)
/// );
/// ```
///
/// Fiscal identity per <https://4invoices.net/py/modelo-factura>:
/// `ticket_id` is the printed number `EEE-PPP-NNNNNNN` (see
/// [`crate::fiscal::InvoiceNumber`]), scoped per partner
/// (`unique (partner_id, ticket_id)`). `timbrado` is the 8-digit DNIT
/// authorization ([`crate::fiscal::Timbrado`]); `cdc` is the 44-digit SIFEN
/// code ([`crate::fiscal::Cdc`], globally unique). Three cases are accepted:
/// paper (number only), timbrado paper (number + timbrado) and electronic
/// (number + timbrado + CDC); CDC without timbrado is rejected.
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Tickets {
    /// `tickets.id` primary key (internal; never the invoice number).
    pub id: i32,
    /// Printed invoice number `EEE-PPP-NNNNNNN` (see [`crate::fiscal`]).
    pub ticket_id: String,
    /// 8-digit DNIT timbrado, if the invoice carries one.
    pub timbrado: Option<String>,
    /// 44-digit SIFEN CDC, if the invoice is electronic.
    pub cdc: Option<String>,
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
/// [`products.id`](Products) — redemption prices come from the redeemable
/// catalog only. The API still accepts `products.id` lines and
/// [`crate::db`] resolves them to the catalog entry. `points_per_unit`
/// snapshots the price at redemption time so later catalog changes do not
/// rewrite history. Repeat redemptions of the same prize are allowed (no
/// uniqueness beyond the primary key).
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
