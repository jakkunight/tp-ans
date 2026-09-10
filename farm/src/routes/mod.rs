pub(crate) mod api;
pub(crate) mod frontend;

use askama::Template;
use axum::{Router, response::Html};
use std::path::PathBuf;
use tower_http::services::ServeDir;

use crate::routes::{api::create_api, frontend::create_frontend};

#[derive(Template)]
#[template(path = "base.html")]
struct BaseTemplate {
    title: &'static str,
}

pub fn create_app(assets_path: PathBuf) -> anyhow::Result<Router> {
    Ok(Router::new()
        .nest_service("/assets", ServeDir::new(assets_path))
        .route(
            "/",
            axum::routing::get(|| async {
                let t = BaseTemplate {
                    title: "Hello World!",
                };
                Html(t.render().unwrap())
            }),
        )
        .merge(create_api()?)
        .merge(create_frontend()?))
}
