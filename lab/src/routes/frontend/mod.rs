use std::sync::Arc;

use askama::Template;
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
};
use tracing::error;

use crate::{AppState, models::Clients};

pub fn create_frontend() -> anyhow::Result<Router<Arc<AppState>>> {
    Ok(Router::new()
        .route("/", get(root))
        .route("/clients/login", get(client_login_page))
        .route("/clients/{client_id}/redeem", get(client_redeem_page)))
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
    render_or_error(t)
}

// ============================================================
// Client self-identification: GET /clients/login
// ============================================================

#[derive(Template)]
#[template(path = "client_login.html")]
struct ClientLoginTemplate {}

/// Login page where a client identifies with CI + first/last name.
/// The form calls `POST /api/v1/clients/login`, stores the JWT
/// (1hr expiry) in `localStorage`, then redirects to the redeem page.
pub async fn client_login_page() -> impl IntoResponse {
    render_or_error(ClientLoginTemplate {})
}

// ============================================================
// Client point redemption: GET /clients/{client_id}/redeem
// ============================================================

#[derive(Debug, Clone)]
struct RedeemableProductView {
    product_id: i32,
    name: String,
    description: String,
    points_needed: i32,
}

#[derive(Template)]
#[template(path = "client_redeem.html")]
struct ClientRedeemTemplate {
    client_id: i32,
    ci: i32,
    first_name: String,
    last_name: String,
    total_earned_points: i32,
    balance_points: i32,
    products: Vec<RedeemableProductView>,
}

/// Dashboard where an identified client sees their point balance and
/// redeems points for `redeemable_products`. The form calls
/// `POST /api/v1/clients/{client_id}/redeem_points` with the stored JWT.
pub async fn client_redeem_page(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<i32>,
) -> Result<impl IntoResponse, StatusCode> {
    let client: Clients = sqlx::query_as::<_, Clients>(
        "SELECT id, ci, verification_digit, first_name, last_name FROM clients WHERE id = $1",
    )
    .bind(client_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        error!("client_redeem_page: client lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    let total_earned_points: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(pe.earned_points), 0) FROM point_earnings pe
         JOIN tickets t ON t.id = pe.ticket_id WHERE t.client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        error!("client_redeem_page: points lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let total_earned_points: i32 = total_earned_points.try_into().unwrap_or(i32::MAX);

    let rows: Vec<(i32, String, String, i32)> = sqlx::query_as(
        "SELECT p.id, p.name, p.description, rp.points_needed
         FROM redeemable_products rp JOIN products p ON p.id = rp.product_id
         ORDER BY p.name ASC, p.id ASC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        error!("client_redeem_page: redeemable lookup failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let template = ClientRedeemTemplate {
        client_id: client.id,
        ci: client.ci,
        first_name: client.first_name,
        last_name: client.last_name,
        total_earned_points,
        // No `redemptions` table exists yet, so history is 0.
        balance_points: total_earned_points,
        products: rows
            .into_iter()
            .map(
                |(product_id, name, description, points_needed)| RedeemableProductView {
                    product_id,
                    name,
                    description,
                    points_needed,
                },
            )
            .collect(),
    };
    Ok(render_or_error(template))
}

fn render_or_error<T: Template>(template: T) -> Html<String> {
    match template.render() {
        Ok(h) => Html(h),
        Err(e) => {
            error!("Failed to render the template!");
            error!("{e:?}");
            Html(ERROR_HTML.to_string())
        }
    }
}
