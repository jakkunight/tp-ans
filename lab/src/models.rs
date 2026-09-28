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
    pub id: i32,
    pub name: String,
    pub ruc: String,
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
    pub id: i32,
    pub ci: i32,
    pub verification_digit: Option<i32>,
    pub first_name: String,
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
    pub id: i32,
    pub name: String,
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
    pub id: i32,
    pub product_id: i32,
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
    pub id: i32,
    pub product_id: i32,
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
    pub id: i32,
    pub date: DateTime<Utc>,
    pub partner_id: i32,
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
    pub id: i32,
    pub ticket_id: i32,
    pub product_id: i32,
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
    pub id: i32,
    pub ticket_id: i32,
    pub earned_points: i32,
}
