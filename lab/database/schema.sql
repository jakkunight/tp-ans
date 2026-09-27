-- lab/database/schema.sql
-- Sistema de Validación de Facturas - Base de Datos
-- Fuente: docs/database-schema.md
--
-- Normalization notes (3NF):
-- * point_earnings has no client_id: the client is functionally dependent
--   on the ticket (ticket_id -> client_id via tickets), so storing it here
--   would be a transitive dependency. Join tickets to get the client.
-- * redemption line totals are NOT stored: redeemed points are derived as
--   quantity * redeemable_products.points_needed. Storing the total would
--   duplicate a fact about the product (transitive dependency) and go stale
--   if points_needed changes. Compute it with a JOIN.
-- * earned_points IS stored: it is the event fact for the ticket (rates can
--   change over time, so the awarded amount is a snapshot of the event).
-- * An invoice cancellation is recorded as an audit entry; the original
--   invoice and all its data (including already-earned points) remain intact.

-- ============================================================
-- Partners
-- ============================================================

create table partners (
    id serial not null primary key,
    name varchar(32) not null,
    ruc varchar(32) not null,
    is_active boolean not null default true
);

-- ============================================================
-- Clients
-- ============================================================

create table clients (
    id serial not null primary key,
    ci int not null unique check (ci >= 0),
    verification_digit int null check (
        verification_digit is null
        or (verification_digit >= 0 and verification_digit <= 9)
    ),
    first_name varchar(32) not null,
    last_name varchar(32) not null
);

-- ============================================================
-- Products
-- ============================================================

create table products (
    id serial not null primary key,
    name varchar(64) not null,
    description varchar(128) not null default 'N/A'
);

-- Products that earn points when purchased
create table promotion_products (
    id serial not null primary key,
    product_id int not null references products(id),
    points_cost int not null check (points_cost >= 1)
);

-- Products that can be redeemed for points
create table redeemable_products (
    id serial not null primary key,
    product_id int not null references products(id),
    points_needed int not null check (points_needed >= 1)
);

-- ============================================================
-- Invoices / Tickets
-- ============================================================

create table tickets (
    id serial not null primary key,
    date timestamptz not null default current_timestamp,
    partner_id int not null references partners(id),
    client_id int not null references clients(id)
);

create table ticket_details (
    id serial not null primary key,
    ticket_id int not null references tickets(id) on delete cascade,
    product_id int not null references products(id),
    quantity int not null check (quantity >= 1),
    unique (ticket_id, product_id)
);

-- ============================================================
-- Point accumulation
-- ============================================================

create table point_earnings (
    id serial not null primary key,
    ticket_id int not null unique references tickets(id) on delete cascade,
    earned_points int not null check (earned_points >= 1)
);
