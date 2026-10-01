//! # lab — Sistema de Validación de Facturas.
//!
//! Backend binary for the laboratory loyalty-points system: pharmacies
//! ([`partners`](crate::models::Partners)) upload invoices (tickets) that
//! award points to clients, and clients redeem those points for products.
//!
//! The HTTP surface is split into a JSON REST API
//! ([`routes::api`], served under `/api/v1/...`) and server-rendered pages
//! ([`routes::frontend`], Askama + HTMX). Authentication uses JWT claims
//! defined in [`jwt`]. All persistence goes through the Postgres pool held
//! by [`AppState`]; the schema lives in `lab/database/schema.sql`.
//!
//! ## Required environment
//!
//! * `LAB_DB_URL` — Postgres connection string.
//! * `LAB_JWT_SECRET` — HMAC secret used to sign/verify JWTs.
//! * `LAB_OTP_SECRET` — secret used to derive client OTP codes; falls back to
//!   `LAB_JWT_SECRET` when unset.
//! * `LAB_OTP_SENDER` — OTP delivery backend: `"log"` (default; logs the
//!   masked destination and — dev only — the code itself) or `"smtp"`
//!   (sends a real email through Gmail via `lettre`).
//! * `EMAIL_FROM` — Gmail address the OTP emails are sent from (required for
//!   `"smtp"`; must be the account the app password belongs to).
//! * `EMAIL_APP_PASSWORD` — Gmail app password for `EMAIL_FROM` (required
//!   for `"smtp"`; from the `EMAIL_APP_PASSWORD` secret, never the account
//!   password).
//!
//! The server listens on `127.0.0.1:8080`.
#![deny(missing_docs)]
pub(crate) mod db;
pub(crate) mod fiscal;
pub(crate) mod jwt;
pub(crate) mod models;
pub(crate) mod otp;
pub(crate) mod routes;

use std::{env, sync::Arc};

use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tracing::info;

use crate::{otp::OtpGuard, routes::create_app};

/// Shared state injected into every handler via Axum's [`State`](axum::extract::State).
pub(crate) struct AppState {
    /// Postgres connection pool (see `lab/database/schema.sql`).
    pub(crate) db: PgPool,
    /// In-memory OTP resend throttling + lockout (see [`crate::otp::OtpGuard`]).
    otp_guard: OtpGuard,
}

impl AppState {
    /// Borrows the OTP rate-limit guard.
    pub(crate) fn otp_guard(&self) -> &OtpGuard {
        &self.otp_guard
    }

    /// Test-only constructor: shared state over an existing pool with a fresh
    /// OTP guard (used by handler tests that never touch the network).
    #[cfg(test)]
    pub(crate) fn for_tests(db: PgPool) -> Arc<Self> {
        Arc::new(Self {
            db,
            otp_guard: OtpGuard::new(),
        })
    }
}

/// Bootstraps tracing, connects the [`PgPool`], builds the app and serves it.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::fmt().compact().finish();
    tracing::subscriber::set_global_default(subscriber)?;
    let db_connection_str = std::env::var("LAB_DB_URL").unwrap();
    let pool = match PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_connection_str)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("{e:?}");
            anyhow::bail!(e);
        }
    };
    let state = Arc::new(AppState {
        db: pool,
        otp_guard: OtpGuard::new(),
    });
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    let assets_path = env::current_dir()?;
    let assets_path = assets_path.join("assets");
    let app = create_app(assets_path, state)?;
    info!("Running at localhost:8080!");
    axum::serve(listener, app).await?;
    Ok(())
}
