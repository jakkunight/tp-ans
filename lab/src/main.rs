pub(crate) mod jwt;
pub(crate) mod routes;

use std::{env, sync::Arc};

use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tracing::info;

use crate::routes::create_app;

struct AppState {
    db: PgPool,
}

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
    let state = Arc::new(AppState { db: pool });
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    let assets_path = env::current_dir()?;
    let assets_path = assets_path.join("assets");
    let app = create_app(assets_path, state)?;
    info!("Running at localhost:8080!");
    axum::serve(listener, app).await?;
    Ok(())
}
