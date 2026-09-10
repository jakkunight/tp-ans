pub(crate) mod routes;

use std::env;

use tokio::net::TcpListener;

use crate::routes::create_app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::fmt().compact().finish();
    tracing::subscriber::set_global_default(subscriber)?;
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    let app = create_app(env::current_dir()?)?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
