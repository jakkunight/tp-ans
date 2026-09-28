pub(crate) mod api;
pub(crate) mod frontend;

use askama::Template;
use axum::{
    Router,
    http::{HeaderValue, Method, header},
};
use std::{path::PathBuf, sync::Arc};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    services::ServeDir,
};
use tracing::info;

use crate::{
    AppState,
    routes::{api::create_api, frontend::create_frontend},
};

#[derive(Template)]
#[template(path = "base.html")]
struct BaseTemplate {
    title: &'static str,
}

pub fn create_app(assets_path: PathBuf, state: Arc<AppState>) -> anyhow::Result<Router> {
    let p = assets_path
        .as_path()
        .as_os_str()
        .to_str()
        .unwrap_or_default();
    info!("Assets Path: {p}");
    Ok(Router::new()
        .nest_service("/assets", ServeDir::new(assets_path))
        .merge(create_api()?)
        .merge(create_frontend()?)
        .layer(loopback_cors())
        .with_state(state))
}

/// CORS layer that only allows origins served from 127.0.0.1 (any scheme or
/// port, e.g. `http://127.0.0.1:3000`). Everything else gets no
/// `access-control-allow-*` response headers, so browsers block the response.
fn loopback_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            is_loopback_origin(origin)
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

fn is_loopback_origin(origin: &HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let host = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or(origin);
    let host = host.split(':').next().unwrap_or(host);
    host == "127.0.0.1"
}
