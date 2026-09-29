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
//! * `LAB_OTP_SENDER` — OTP delivery backend (`log` by default; logs the
//!   masked destination and — dev only — the code itself).
//!
//! The server listens on `127.0.0.1:8080`.
#![deny(missing_docs)]
pub(crate) mod db;
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
