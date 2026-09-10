use std::path::PathBuf;

use askama::Template;
use axum::{Router, response::Html};
use tower_http::services::ServeDir;

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
        ))
}
