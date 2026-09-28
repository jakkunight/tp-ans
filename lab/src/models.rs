/// # Data models derived from the database schema.
///
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
///     ruc varchar(32) not null,
///     is_active boolean not null default true
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Partners {
    /// `partners.id` primary key.
    pub id: i32,
    /// `partners.name` display name (`varchar(32)`).
    pub name: String,
    /// `partners.ruc` tax id (`varchar(32)`), the login natural key.
    pub ruc: String,
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
///     last_name varchar(32) not null
/// );
/// ```
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
}

// ============================================================
/// ## Products
///
/// **Schema:**
/// ```sql
/// create table products (
///     id serial not null primary key,
///     name varchar(64) not null,
///     description varchar(128) not null default 'N/A',
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
///     product_id int not null references products(id) on delete cascade,
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
///     product_id int not null references products(id) on delete cascade,
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
///     date timestamptz not null default current_timestamp,
///     partner_id int not null references partners(id),
///     client_id int not null references clients(id)
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct Tickets {
    /// `tickets.id` primary key.
    pub id: i32,
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
///     ticket_id int not null references tickets(id) on delete cascade,
///     product_id int not null references products(id),
///     quantity int not null check (quantity >= 1),
///     unique (ticket_id, product_id)
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct TicketDetails {
    /// `ticket_details.id` primary key.
    pub id: i32,
    /// Owning `tickets.id` (cascades on delete; unique per `product_id`).
    pub ticket_id: i32,
    /// Purchased `products.id`.
    pub product_id: i32,
    /// Units purchased (`quantity >= 1`).
    pub quantity: i32,
}

// ============================================================
/// ## PointEarnings
///
/// **Schema:**
/// ```sql
/// create table point_earnings (
///     id serial not null primary key,
///     ticket_id int not null unique references tickets(id) on delete cascade,
///     earned_points int not null check (earned_points >= 1)
/// );
/// ```
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize, FromRow)]
pub struct PointEarnings {
    /// `point_earnings.id` primary key.
    pub id: i32,
    /// Awarded `tickets.id` (unique; cascades on delete).
    pub ticket_id: i32,
    /// Awarded snapshot (`earned_points >= 1`).
    pub earned_points: i32,
}
