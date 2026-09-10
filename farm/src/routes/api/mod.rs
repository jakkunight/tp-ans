use axum::{
    Router,
    response::{IntoResponse, Json},
    routing::get,
};

pub fn create_api() -> anyhow::Result<Router> {
    Ok(Router::new().route("/api/hello", get(hello_api)))
}

// Devuelve un JSON
pub async fn hello_api() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 200,
        "msg": "OK"
    }))
}
