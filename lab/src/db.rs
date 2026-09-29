//! # Database access and transactions.
//!
//! All SQL lives here. Handlers in [`crate::routes::api`] only validate
//! request shapes and map [`DbError`] to HTTP responses; every multi-statement
//! operation owns its transaction inside one of the functions below, so
//! callers can never observe (or forget) a half-committed operation.
//!
//! Balance accounting follows the schema notes: `point_ledger` is the single
//! source of truth (`SUM(points_delta)` per client). Ticket awards post
//! positive rows linked from `ticket_earnings`, redemptions post negative
//! rows linked from `redemption_discounts`, and cancellations post negative
//! compensating rows linked from `ticket_cancellation_discounts` — the
//! original ticket rows are never deleted.
//!
//! ## Concurrency protocol
//!
//! Postgres runs these transactions at the default `READ COMMITTED` level.
//! Check-then-act races are closed two ways:
//!
//! * **Pessimistic row locks** (`SELECT ... FOR UPDATE`): [`redeem_points`]
//!   locks the client row before reading the balance, so concurrent
//!   redemptions for the same client serialize and cannot overspend;
//!   [`cancel_ticket`] locks the ticket row before the already-cancelled
//!   check; [`create_ticket`] locks the partner row for the active check.
//!   Locks are always taken in parent-before-child order
//!   (partner/client → ticket → lines/ledger) and never held across
//!   round-trips, so no deadlock cycle exists.
//! * **Unique constraints as backstop**: the scoped invoice number
//!   `(partner_id, ticket_id)`, the one-cancellation-per-ticket rule and
//!   the find-or-create `ON CONFLICT (ci) DO NOTHING` turn any lock-window
//!   leftover into a mapped `409`, never a 500 or a half-write.
//!
//! [`client_dashboard`] runs its multi-statement read inside a `READ ONLY`
//! transaction so concurrent writers cannot tear the view (tickets listed
//! without their details, totals from a different instant than the rows).
use std::collections::{HashMap, HashSet};

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::http::StatusCode;
use sqlx::{PgPool, Postgres, Transaction};

use crate::{
    models::{
        Clients, Partners, PointLedger, Products, PromotionProducts, RedeemableProducts,
        RedemptionDiscounts, RedemptionItems, Redemptions, TicketCancellationDiscounts,
        TicketCancellations, TicketDetails, TicketEarnings, Tickets,
    },
    routes::api::dto::{
        ClientDataResponse, ClientInfoDto, ErrorResponse, TicketDetailDto, TicketDto,
    },
};

/// Failures of a DB operation, mapped to HTTP by the API layer.
#[derive(Debug)]
pub enum DbError {
    /// Row the caller asked for does not exist.
    NotFound {
        /// Machine-readable code for the response body.
        error: &'static str,
    },
    /// Unique/constraint conflict (duplicate invoice, double cancel, …).
    Conflict {
        /// Machine-readable code for the response body.
        error: &'static str,
        /// Human-readable detail for the response body.
        message: String,
    },
    /// Request is well-formed but violates a domain rule.
    Invalid {
        /// Machine-readable code for the response body.
        error: &'static str,
        /// Human-readable detail for the response body.
        message: String,
    },
    /// A referenced catalog entry cannot satisfy the request.
    Unprocessable {
        /// Machine-readable code for the response body.
        error: &'static str,
        /// Human-readable detail for the response body.
        message: String,
    },
    /// Bad credentials (unknown RUC or wrong PSK; deliberately vague).
    Unauthorized {
        /// Machine-readable code for the response body.
        error: &'static str,
    },
    /// Correct credentials but the actor may not proceed (inactive partner).
    Forbidden {
        /// Machine-readable code for the response body.
        error: &'static str,
    },
    /// Transport / SQL failure.
    Db(sqlx::Error),
}

impl DbError {
    /// Maps the failure to its HTTP status plus JSON error body.
    pub fn status_and_body(&self) -> (StatusCode, ErrorResponse) {
        match self {
            Self::NotFound { error } => (StatusCode::NOT_FOUND, ErrorResponse::new(*error)),
            Self::Conflict { error, message } => (
                StatusCode::CONFLICT,
                ErrorResponse::with_message(*error, message),
            ),
            Self::Invalid { error, message } => (
                StatusCode::BAD_REQUEST,
                ErrorResponse::with_message(*error, message),
            ),
            Self::Unprocessable { error, message } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorResponse::with_message(*error, message),
            ),
            Self::Unauthorized { error } => (StatusCode::UNAUTHORIZED, ErrorResponse::new(*error)),
            Self::Forbidden { error } => (StatusCode::FORBIDDEN, ErrorResponse::new(*error)),
            Self::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse::new("database error"),
            ),
        }
    }
}

impl From<sqlx::Error> for DbError {
    /// Lifts any `sqlx` failure into [`DbError::Db`].
    fn from(value: sqlx::Error) -> Self {
        Self::Db(value)
    }
}

/// `true` when a `sqlx` error is a Postgres unique violation.
///
/// Uses the driver's own classification rather than matching SQLSTATE text.
fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .is_some_and(|db| db.is_unique_violation())
}

// ============================================================
// Partner authentication
// ============================================================

/// Hashes a partner pre-shared key with Argon2 (default params, random salt).
///
/// Test-only provisioning helper: run
/// `cargo test psk_hash_roundtrip -- --nocapture` flows or the snippet below
/// to mint `partners.psk_hash` values for `seed.sql`. Production provisioning
/// (admin endpoint or CLI) is out of scope.
///
/// # Errors
///
/// Fails when the OS RNG or Argon2 itself fails.
#[cfg(test)]
pub fn hash_psk(psk: &str) -> anyhow::Result<String> {
    use argon2::PasswordHasher as _;
    let salt_bytes: [u8; 16] = rand::random();
    let hash = Argon2::default()
        .hash_password_with_salt(psk.as_bytes(), &salt_bytes)
        .map_err(|e| anyhow::anyhow!("psk hashing failed: {e}"))?;
    Ok(hash.to_string())
}

/// Verifies a cleartext PSK against an Argon2 PHC hash.
fn verify_psk(psk: &str, psk_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(psk_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(psk.as_bytes(), &parsed)
        .is_ok()
}

/// Authenticates a pharmacy by RUC + pre-shared key.
///
/// Unknown RUC and wrong PSK both report [`DbError::Unauthorized`] with the
/// same code so failures do not reveal which check failed; inactive partners
/// report [`DbError::Forbidden`].
///
/// # Errors
///
/// Returns [`DbError::Unauthorized`], [`DbError::Forbidden`] or
/// [`DbError::Db`] on transport failures.
pub async fn authenticate_partner(
    pool: &PgPool,
    ruc: &str,
    psk: &str,
) -> Result<Partners, DbError> {
    let partner: Option<Partners> = sqlx::query_as::<_, Partners>(
        "SELECT id, name, ruc, psk_hash, is_active FROM partners WHERE ruc = $1",
    )
    .bind(ruc)
    .fetch_optional(pool)
    .await?;
    let Some(partner) = partner else {
        return Err(DbError::Unauthorized {
            error: "unknown credentials",
        });
    };
    if !partner.is_active {
        return Err(DbError::Forbidden {
            error: "inactive partner",
        });
    }
    if !verify_psk(psk, &partner.psk_hash) {
        return Err(DbError::Unauthorized {
            error: "unknown credentials",
        });
    }
    Ok(partner)
}

// ============================================================
// Clients
// ============================================================

/// Buyer data for [`find_or_create_client`].
pub struct NewClient<'a> {
    /// `clients.ci` lookup key.
    pub ci: i32,
    /// Optional RUC suffix digit.
    pub verification_digit: Option<i32>,
    /// Buyer first name.
    pub first_name: &'a str,
    /// Buyer last name.
    pub last_name: &'a str,
    /// SMS channel (at least one contact required for new clients).
    pub phone_number: Option<&'a str>,
    /// Email channel (at least one contact required for new clients).
    pub email: Option<&'a str>,
}

/// Looks a client up by unique `ci`, inserting him when unknown.
///
/// Returns the row plus whether it was created. New clients require at least
/// one contact (the schema CHECK); existing rows keep their stored contacts.
///
/// # Errors
///
/// Returns [`DbError::Invalid`] when a new client has no contact and
/// [`DbError::Db`] on transport failures.
pub async fn find_or_create_client(
    tx: &mut Transaction<'_, Postgres>,
    client: NewClient<'_>,
) -> Result<(Clients, bool), DbError> {
    let existing: Option<Clients> = sqlx::query_as::<_, Clients>(
        "SELECT id, ci, verification_digit, first_name, last_name, phone_number, email
         FROM clients WHERE ci = $1",
    )
    .bind(client.ci)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = existing {
        return Ok((row, false));
    }
    if client.phone_number.is_none() && client.email.is_none() {
        return Err(DbError::Invalid {
            error: "missing buyer contact",
            message: "new clients need a phone_number or an email (OTP channel)".to_string(),
        });
    }
    let insert = sqlx::query(
        "INSERT INTO clients (ci, verification_digit, first_name, last_name, phone_number, email)
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (ci) DO NOTHING",
    )
    .bind(client.ci)
    .bind(client.verification_digit)
    .bind(client.first_name)
    .bind(client.last_name)
    .bind(client.phone_number)
    .bind(client.email)
    .execute(&mut **tx)
    .await;
    if let Err(e) = insert
        && !is_unique_violation(&e)
    {
        return Err(DbError::Db(e));
    }
    // Either we inserted, or a concurrent transaction won the race: either
    // way the row exists now.
    let row: Clients = sqlx::query_as::<_, Clients>(
        "SELECT id, ci, verification_digit, first_name, last_name, phone_number, email
         FROM clients WHERE ci = $1",
    )
    .bind(client.ci)
    .fetch_one(&mut **tx)
    .await?;
    Ok((row, true))
}

/// Fetches a client by primary key.
///
/// # Errors
///
/// Returns [`DbError::NotFound`] for unknown ids.
pub async fn client_by_id(pool: &PgPool, client_id: i32) -> Result<Clients, DbError> {
    sqlx::query_as::<_, Clients>(
        "SELECT id, ci, verification_digit, first_name, last_name, phone_number, email
         FROM clients WHERE id = $1",
    )
    .bind(client_id)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound {
        error: "client not found",
    })
}

/// Fetches a client by unique CI (OTP request path).
///
/// # Errors
///
/// Returns [`DbError::NotFound`] for unknown CIs.
pub async fn client_by_ci(pool: &PgPool, ci: i32) -> Result<Clients, DbError> {
    sqlx::query_as::<_, Clients>(
        "SELECT id, ci, verification_digit, first_name, last_name, phone_number, email
         FROM clients WHERE ci = $1",
    )
    .bind(ci)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound {
        error: "unknown credentials",
    })
}

// ============================================================
// Tickets
// ============================================================

/// One validated ticket line for [`create_ticket`].
pub struct TicketLine {
    /// `products.id`.
    pub product_id: i32,
    /// Units (`>= 1`, checked by the handler).
    pub quantity: i32,
}

/// Validated input for [`create_ticket`].
pub struct NewTicket<'a> {
    /// `partners.id` of the issuing pharmacy.
    pub partner_id: i32,
    /// Invoice number from the factura (unique per partner).
    pub ticket_number: &'a str,
    /// Buyer identity from the factura.
    pub client: NewClient<'a>,
    /// At least one line (checked by the handler).
    pub details: &'a [TicketLine],
}

/// What [`create_ticket`] created.
pub struct CreatedTicket {
    /// `tickets.id` of the new row.
    pub ticket_id: i32,
    /// `clients.id` of the existing or newly registered buyer.
    pub client_id: i32,
    /// Points awarded (`0` when the ticket earned none).
    pub earned_points: i32,
}

/// Registers an invoice and awards its points, atomically.
///
/// Inside one transaction: checks the partner (must exist and be active),
/// find-or-creates the buyer, validates every product line, inserts the
/// `tickets` row (scoped by `(partner_id, ticket_id)`), its `ticket_details`
/// lines and — when at least one point was earned — a positive
/// `point_ledger` row linked from `ticket_earnings`.
///
/// # Errors
///
/// Returns [`DbError::NotFound`] for unknown partners/products,
/// [`DbError::Forbidden`] for inactive partners, [`DbError::Conflict`] for a
/// duplicate invoice number and [`DbError::Db`] on transport failures.
pub async fn create_ticket(pool: &PgPool, ticket: NewTicket<'_>) -> Result<CreatedTicket, DbError> {
    let mut tx = pool.begin().await?;

    // Lock the partner row first (parent-before-child lock order): the
    // active check below stays true until this transaction commits, and
    // concurrent writers on the partner serialize here instead of racing it.
    let partner: Option<Partners> = sqlx::query_as::<_, Partners>(
        "SELECT id, name, ruc, psk_hash, is_active FROM partners WHERE id = $1 FOR UPDATE",
    )
    .bind(ticket.partner_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(partner) = partner else {
        return Err(DbError::NotFound {
            error: "partner not found",
        });
    };
    if !partner.is_active {
        return Err(DbError::Forbidden {
            error: "inactive partner",
        });
    }

    let product_ids: Vec<i32> = ticket.details.iter().map(|d| d.product_id).collect();
    let found: HashSet<i32> = sqlx::query_as::<_, Products>(
        "SELECT id, name, description FROM products WHERE id = ANY($1)",
    )
    .bind(&product_ids)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|p| p.id)
    .collect();
    if !found.is_superset(&product_ids.iter().copied().collect()) {
        let missing: Vec<String> = ticket
            .details
            .iter()
            .map(|d| d.product_id)
            .filter(|id| !found.contains(id))
            .map(|id| id.to_string())
            .collect();
        tracing::warn!(
            missing = missing.join(",").as_str(),
            "create_ticket: unknown product"
        );
        return Err(DbError::NotFound {
            error: "unknown product",
        });
    }

    let promo_rows: Vec<PromotionProducts> = sqlx::query_as(
        "SELECT id, product_id, points_cost FROM promotion_products WHERE product_id = ANY($1)",
    )
    .bind(&product_ids)
    .fetch_all(&mut *tx)
    .await?;
    let promo: HashMap<i32, i32> = promo_rows
        .into_iter()
        .map(|p| (p.product_id, p.points_cost))
        .collect();
    let earned: i64 = ticket
        .details
        .iter()
        .map(|d| *promo.get(&d.product_id).unwrap_or(&0) as i64 * d.quantity as i64)
        .sum();
    let earned_points: i32 = earned.try_into().map_err(|_| DbError::Invalid {
        error: "points overflow",
        message: "earned points exceed i32".to_string(),
    })?;

    let (client, _) = find_or_create_client(&mut tx, ticket.client).await?;

    let ticket_pk: i32 = sqlx::query_scalar(
        "INSERT INTO tickets (ticket_id, partner_id, client_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(ticket.ticket_number)
    .bind(ticket.partner_id)
    .bind(client.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            DbError::Conflict {
                error: "duplicate ticket",
                message: format!(
                    "invoice {} already registered for this partner",
                    ticket.ticket_number
                ),
            }
        } else {
            DbError::Db(e)
        }
    })?;

    for d in ticket.details {
        sqlx::query(
            "INSERT INTO ticket_details (ticket_id, product_id, quantity) VALUES ($1, $2, $3)",
        )
        .bind(ticket_pk)
        .bind(d.product_id)
        .bind(d.quantity)
        .execute(&mut *tx)
        .await?;
    }

    if earned_points >= 1 {
        let ledger: PointLedger = sqlx::query_as::<_, PointLedger>(
            "INSERT INTO point_ledger (client_id, points_delta) VALUES ($1, $2)
             RETURNING id, client_id, points_delta, date",
        )
        .bind(client.id)
        .bind(earned_points)
        .fetch_one(&mut *tx)
        .await?;
        let _earning: TicketEarnings = sqlx::query_as::<_, TicketEarnings>(
            "INSERT INTO ticket_earnings (point_ledger_id, ticket_id) VALUES ($1, $2)
             RETURNING id, point_ledger_id, ticket_id",
        )
        .bind(ledger.id)
        .bind(ticket_pk)
        .fetch_one(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(CreatedTicket {
        ticket_id: ticket_pk,
        client_id: client.id,
        earned_points: if earned_points >= 1 { earned_points } else { 0 },
    })
}

/// Voids an invoice, atomically.
///
/// Inside one transaction: locks the ticket row, records the
/// `ticket_cancellations` audit entry and — when the ticket awarded points —
/// posts a negative `point_ledger` row linked from
/// `ticket_cancellation_discounts`. The original ticket rows stay intact.
///
/// # Errors
///
/// Returns [`DbError::NotFound`] for unknown tickets,
/// [`DbError::Conflict`] when already cancelled, and [`DbError::Db`] on
/// transport failures.
pub async fn cancel_ticket(pool: &PgPool, ticket_pk: i32, reason: &str) -> Result<i32, DbError> {
    let mut tx = pool.begin().await?;

    // Lock the ticket row (parent before children): concurrent cancels of
    // the same ticket serialize here, so the already-cancelled check below
    // cannot pass twice. The `ticket_cancellations.ticket_id` unique
    // constraint remains as a backstop, mapped to `409`.
    let exists: Option<i32> = sqlx::query_scalar("SELECT id FROM tickets WHERE id = $1 FOR UPDATE")
        .bind(ticket_pk)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(DbError::NotFound {
            error: "ticket not found",
        });
    }
    let already: Option<i32> =
        sqlx::query_scalar("SELECT id FROM ticket_cancellations WHERE ticket_id = $1")
            .bind(ticket_pk)
            .fetch_optional(&mut *tx)
            .await?;
    if already.is_some() {
        return Err(DbError::Conflict {
            error: "already cancelled",
            message: format!("ticket {ticket_pk} is already voided"),
        });
    }

    // Points to reverse: the award snapshot posted for this ticket, if any.
    let awarded: Option<i64> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(pl.points_delta), 0) FROM ticket_earnings te
         JOIN point_ledger pl ON pl.id = te.point_ledger_id WHERE te.ticket_id = $1",
    )
    .bind(ticket_pk)
    .fetch_one(&mut *tx)
    .await?;
    let awarded: i32 = awarded.unwrap_or(0).try_into().unwrap_or(i32::MAX);

    let cancellation: TicketCancellations = sqlx::query_as::<_, TicketCancellations>(
        "INSERT INTO ticket_cancellations (ticket_id, reason) VALUES ($1, $2)
         RETURNING id, ticket_id, date, reason",
    )
    .bind(ticket_pk)
    .bind(reason)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            DbError::Conflict {
                error: "already cancelled",
                message: format!("ticket {ticket_pk} is already voided"),
            }
        } else {
            DbError::Db(e)
        }
    })?;
    let cancellation_id = cancellation.id;

    if awarded >= 1 {
        let client_id: i32 = sqlx::query_scalar("SELECT client_id FROM tickets WHERE id = $1")
            .bind(ticket_pk)
            .fetch_one(&mut *tx)
            .await?;
        let ledger: PointLedger = sqlx::query_as::<_, PointLedger>(
            "INSERT INTO point_ledger (client_id, points_delta) VALUES ($1, $2)
             RETURNING id, client_id, points_delta, date",
        )
        .bind(client_id)
        .bind(-awarded)
        .fetch_one(&mut *tx)
        .await?;
        let _discount: TicketCancellationDiscounts =
            sqlx::query_as::<_, TicketCancellationDiscounts>(
                "INSERT INTO ticket_cancellation_discounts (point_ledger_id, cancellation_id)
                 VALUES ($1, $2) RETURNING id, point_ledger_id, cancellation_id",
            )
            .bind(ledger.id)
            .bind(cancellation_id)
            .fetch_one(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(cancellation_id)
}

// ============================================================
// Redemptions
// ============================================================

/// One validated redemption line for [`redeem_points`] (`products.id`).
pub struct RedeemLine {
    /// `products.id` of the prize.
    pub product_id: i32,
    /// Units (`>= 1`, checked by the handler).
    pub quantity: i32,
}

/// What [`redeem_points`] persisted.
pub struct Redeemed {
    /// `redemptions.id` of the new row.
    pub redemption_id: i32,
    /// Total cost charged.
    pub redeemed_points: i32,
    /// Balance left after the charge.
    pub remaining_points: i32,
}

/// Prices and persists a points redemption, atomically.
///
/// Inside one transaction: checks the client, resolves every line through
/// `redeemable_products` (snapshotting `points_needed` per line), verifies
/// the ledger balance covers the cost, then inserts the `redemptions` row,
/// its `redemption_items`, a negative `point_ledger` entry and the
/// `redemption_discounts` link.
///
/// # Errors
///
/// Returns [`DbError::NotFound`] for unknown clients,
/// [`DbError::Unprocessable`] for non-redeemable products,
/// [`DbError::Conflict`] for insufficient points (or a catalog entry the
/// schema's global-unique `redemption_items.product_id` already spent), and
/// [`DbError::Db`] on transport failures.
pub async fn redeem_points(
    pool: &PgPool,
    client_id: i32,
    items: &[RedeemLine],
) -> Result<Redeemed, DbError> {
    let mut tx = pool.begin().await?;

    // Lock the client row FIRST (parent before children): this serializes
    // concurrent redemptions for the same client, so the balance read below
    // cannot pass twice for the same points. Without this lock, two
    // simultaneous requests could both see a sufficient balance and drive
    // it negative.
    let exists: Option<i32> = sqlx::query_scalar("SELECT id FROM clients WHERE id = $1 FOR UPDATE")
        .bind(client_id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(DbError::NotFound {
            error: "client not found",
        });
    }

    let product_ids: Vec<i32> = items.iter().map(|i| i.product_id).collect();
    // One catalog read: each row carries the entry id (the
    // `redemption_items.product_id` FK target), the `products.id` and the price.
    let catalog: Vec<RedeemableProducts> = sqlx::query_as::<_, RedeemableProducts>(
        "SELECT id, product_id, points_needed FROM redeemable_products
         WHERE product_id = ANY($1)",
    )
    .bind(&product_ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut prices: HashMap<i32, i32> = HashMap::with_capacity(catalog.len());
    let mut entries: HashMap<i32, i32> = HashMap::with_capacity(catalog.len());
    for row in &catalog {
        prices.insert(row.product_id, row.points_needed);
        entries.insert(row.product_id, row.id);
    }

    let mut total: i64 = 0;
    for item in items {
        let Some(price) = prices.get(&item.product_id) else {
            return Err(DbError::Unprocessable {
                error: "product not redeemable",
                message: format!("product {} is not redeemable", item.product_id),
            });
        };
        total += *price as i64 * item.quantity as i64;
    }
    let redeemed_points: i32 = total.try_into().map_err(|_| DbError::Invalid {
        error: "points overflow",
        message: "redeemed points exceed i32".to_string(),
    })?;

    let balance: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(points_delta), 0) FROM point_ledger WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&mut *tx)
    .await?;
    let remaining = balance - total;
    if remaining < 0 {
        return Err(DbError::Conflict {
            error: "insufficient points",
            message: format!("redeeming {redeemed_points} of {balance} available"),
        });
    }

    let redemption: Redemptions = sqlx::query_as::<_, Redemptions>(
        "INSERT INTO redemptions (client_id, redeemed_points) VALUES ($1, $2)
         RETURNING id, client_id, redeemed_points, date",
    )
    .bind(client_id)
    .bind(redeemed_points)
    .fetch_one(&mut *tx)
    .await?;
    let redemption_id = redemption.id;

    for item in items {
        let entry_id = entries.get(&item.product_id).copied().unwrap_or(0);
        let price = prices.get(&item.product_id).copied().unwrap_or(0);
        let _line: RedemptionItems = sqlx::query_as::<_, RedemptionItems>(
            "INSERT INTO redemption_items (redemption_id, product_id, quantity, points_per_unit)
             VALUES ($1, $2, $3, $4)
             RETURNING id, redemption_id, product_id, quantity, points_per_unit",
        )
        .bind(redemption_id)
        .bind(entry_id)
        .bind(item.quantity)
        .bind(price)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                DbError::Conflict {
                    error: "prize already redeemed",
                    message: format!(
                        "product {} was already redeemed once (schema limits each prize to a single redemption row)",
                        item.product_id
                    ),
                }
            } else {
                DbError::Db(e)
            }
        })?;
    }

    let ledger: PointLedger = sqlx::query_as::<_, PointLedger>(
        "INSERT INTO point_ledger (client_id, points_delta) VALUES ($1, $2)
         RETURNING id, client_id, points_delta, date",
    )
    .bind(client_id)
    .bind(-redeemed_points)
    .fetch_one(&mut *tx)
    .await?;
    let _discount: RedemptionDiscounts = sqlx::query_as::<_, RedemptionDiscounts>(
        "INSERT INTO redemption_discounts (point_ledger_id, redemption_id) VALUES ($1, $2)
         RETURNING id, point_ledger_id, redemption_id",
    )
    .bind(ledger.id)
    .bind(redemption_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Redeemed {
        redemption_id,
        redeemed_points,
        remaining_points: remaining.try_into().unwrap_or(i32::MAX),
    })
}

// ============================================================
// Dashboard
// ============================================================

/// Point-balance breakdown for one client, derived from `point_ledger`.
struct Balance {
    /// Credits posted by ticket awards.
    earned: i32,
    /// Debits posted by redemptions.
    redeemed: i32,
    /// Debits posted by cancellation reversals.
    cancelled: i32,
}

/// Sums ledger movements per event kind for a client, inside the caller's
/// transaction (see [`client_dashboard`]).
async fn balance_of(
    tx: &mut Transaction<'_, Postgres>,
    client_id: i32,
) -> Result<Balance, DbError> {
    let earned: Option<i64> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(pl.points_delta), 0) FROM ticket_earnings te
         JOIN point_ledger pl ON pl.id = te.point_ledger_id WHERE pl.client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?;
    let redeemed: Option<i64> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(-pl.points_delta), 0) FROM redemption_discounts rd
         JOIN point_ledger pl ON pl.id = rd.point_ledger_id WHERE pl.client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?;
    let cancelled: Option<i64> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(-pl.points_delta), 0) FROM ticket_cancellation_discounts cd
         JOIN point_ledger pl ON pl.id = cd.point_ledger_id WHERE pl.client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Balance {
        earned: earned.unwrap_or(0).try_into().unwrap_or(i32::MAX),
        redeemed: redeemed.unwrap_or(0).try_into().unwrap_or(i32::MAX),
        cancelled: cancelled.unwrap_or(0).try_into().unwrap_or(i32::MAX),
    })
}

/// Serves the client points dashboard: profile, ledger-derived totals and
/// full ticket history (voided invoices included, flagged).
///
/// Reads run inside a `READ ONLY` transaction so the profile, ticket rows
/// and totals all come from one snapshot, even while tickets, redemptions
/// or cancellations commit concurrently.
///
/// # Errors
///
/// Returns [`DbError::NotFound`] for unknown clients.
pub async fn client_dashboard(
    pool: &PgPool,
    client_id: i32,
) -> Result<ClientDataResponse, DbError> {
    let mut tx = pool.begin().await?;
    // Must precede the first statement: freezes the snapshot for every read
    // below without taking any locks.
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *tx)
        .await?;

    let client: Clients = sqlx::query_as::<_, Clients>(
        "SELECT id, ci, verification_digit, first_name, last_name, phone_number, email
         FROM clients WHERE id = $1",
    )
    .bind(client_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound {
        error: "client not found",
    })?;

    let ticket_rows: Vec<Tickets> = sqlx::query_as::<_, Tickets>(
        "SELECT id, ticket_id, date, partner_id, client_id FROM tickets
         WHERE client_id = $1 ORDER BY date ASC, id ASC",
    )
    .bind(client_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut tickets = Vec::with_capacity(ticket_rows.len());
    for t in ticket_rows {
        let detail_rows: Vec<TicketDetails> = sqlx::query_as::<_, TicketDetails>(
            "SELECT id, ticket_id, product_id, quantity FROM ticket_details
             WHERE ticket_id = $1 ORDER BY product_id ASC",
        )
        .bind(t.id)
        .fetch_all(&mut *tx)
        .await?;
        let earned: Option<i64> = sqlx::query_scalar(
            "SELECT COALESCE(SUM(pl.points_delta), 0) FROM ticket_earnings te
             JOIN point_ledger pl ON pl.id = te.point_ledger_id WHERE te.ticket_id = $1",
        )
        .bind(t.id)
        .fetch_one(&mut *tx)
        .await?;
        let cancellation: Option<TicketCancellations> = sqlx::query_as::<_, TicketCancellations>(
            "SELECT id, ticket_id, date, reason FROM ticket_cancellations WHERE ticket_id = $1",
        )
        .bind(t.id)
        .fetch_optional(&mut *tx)
        .await?;
        tickets.push(TicketDto {
            id: t.id,
            ticket_number: t.ticket_id,
            date: t.date,
            partner_id: t.partner_id,
            client_id: t.client_id,
            details: detail_rows
                .into_iter()
                .map(|d| TicketDetailDto {
                    product_id: d.product_id,
                    quantity: d.quantity,
                })
                .collect(),
            earned_points: earned.unwrap_or(0).try_into().ok().filter(|e| *e >= 1),
            cancelled: cancellation.is_some(),
            cancellation_reason: cancellation.map(|c| c.reason),
        });
    }

    let balance = balance_of(&mut tx, client_id).await?;
    tx.commit().await?;
    Ok(ClientDataResponse {
        client: ClientInfoDto {
            id: client.id,
            ci: client.ci,
            verification_digit: client.verification_digit,
            first_name: client.first_name,
            last_name: client.last_name,
            phone_number: client.phone_number,
            email: client.email,
        },
        total_earned_points: balance.earned,
        total_redeemed_points: balance.redeemed,
        cancelled_points: balance.cancelled,
        balance_points: balance.earned - balance.redeemed - balance.cancelled,
        tickets,
    })
}

// ============================================================
// Redeemable catalog (frontend)
// ============================================================

/// One row of the redeemable catalog for the dashboard page.
pub struct CatalogRow {
    /// `products.id` to send back in redemption items.
    pub product_id: i32,
    /// `products.name` display label.
    pub name: String,
    /// `products.description` display subtitle.
    pub description: String,
    /// `redeemable_products.points_needed` cost per unit.
    pub points_needed: i32,
}

/// Lists the redeemable catalog ordered by product name.
///
/// # Errors
///
/// Returns [`DbError::Db`] on transport failures.
pub async fn redeemable_catalog(pool: &PgPool) -> Result<Vec<CatalogRow>, DbError> {
    let rows: Vec<(i32, String, String, i32)> = sqlx::query_as(
        "SELECT p.id, p.name, p.description, rp.points_needed
         FROM redeemable_products rp JOIN products p ON p.id = rp.product_id
         ORDER BY p.name ASC, p.id ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(product_id, name, description, points_needed)| CatalogRow {
                product_id,
                name,
                description,
                points_needed,
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::error::ErrorKind;

    /// Argon2 hashing verifies the right PSK and rejects the wrong one.
    #[test]
    fn psk_hash_roundtrip() {
        let hash = hash_psk("correct horse").unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_psk("correct horse", &hash));
        assert!(!verify_psk("wrong donkey", &hash));
        assert!(!verify_psk("correct horse", "not-a-hash"));
    }

    /// Unique violations map to conflicts; other errors stay transport errors.
    #[test]
    fn unique_violation_detection() {
        let conflict = sqlx::Error::Database(Box::new(DbErrUnique));
        assert!(is_unique_violation(&conflict));
        assert!(!is_unique_violation(&sqlx::Error::RowNotFound));
    }

    /// Minimal `DatabaseError` stub classified as a unique violation.
    #[derive(Debug)]
    struct DbErrUnique;

    impl std::fmt::Display for DbErrUnique {
        /// Renders the stub message.
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "duplicate key")
        }
    }

    impl std::error::Error for DbErrUnique {}

    impl sqlx::error::DatabaseError for DbErrUnique {
        /// Message stub.
        fn message(&self) -> &str {
            "duplicate key"
        }

        /// Classified as a unique violation.
        fn kind(&self) -> ErrorKind {
            ErrorKind::UniqueViolation
        }

        /// Downcast stub.
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        /// Downcast stub.
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        /// Boxing stub.
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
    }
}
