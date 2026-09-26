use std::sync::Arc;

use askama::Template;
use axum::{
    Router,
    response::{Html, IntoResponse},
    routing::get,
};
use tracing::error;

use crate::AppState;

pub fn create_frontend() -> anyhow::Result<Router<Arc<AppState>>> {
    Ok(Router::new().route("/", get(root)))
}

const ERROR_HTML: &str = r#"
<!doctype html>
<html lang="en">
    <head>
        <meta charset="utf-8">
        <title>ERROR</title>
    </head>
    <body>
        <h1>Render ERROR</h1>
        <p>
            Failed to render the current page. Please refresh the page.
        </p>
    </body>
</html>
"#;

#[derive(Template)]
#[template(path = "root.html")]
struct RootTemplate {}

pub async fn root() -> impl IntoResponse {
    let t = RootTemplate {};
    let html = match t.render() {
        Ok(h) => h,
        Err(e) => {
            error!("Failed to render the template!");
            error!("{e:?}");
            ERROR_HTML.to_string()
        }
    };
    Html(html)
}
