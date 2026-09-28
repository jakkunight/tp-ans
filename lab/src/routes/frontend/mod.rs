//! # Server-rendered client pages.
//!
//! Askama + HTMX interface for end customers: self-identification at
//! `GET /clients/login` and the points-redemption dashboard at
//! `GET /clients/{client_id}/redeem`. The pages call the JSON REST API
//! ([`crate::routes::api`]) from the browser, carrying the client JWT from
//! `localStorage`.
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

/// Builds the frontend router: `/`, `/clients/login` and
/// `/clients/{client_id}/redeem`.
///
/// # Errors
///
/// Currently infallible; returns [`anyhow::Error`] to allow future fallible setup.
pub fn create_frontend() -> anyhow::Result<Router<Arc<AppState>>> {
    Ok(Router::new()
        .route("/", get(root))
        .route("/clients/login", get(client_login_page))
        .route("/clients/{client_id}/redeem", get(client_redeem_page)))
}

/// Static fallback page served when an Askama template fails to render.
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
/// Landing page template (`templates/root.html`).
struct RootTemplate {}

/// `GET /` — renders the landing page.
pub async fn root() -> impl IntoResponse {
    let t = RootTemplate {};
    render_or_error(t)
}

// ============================================================
// Client self-identification: GET /clients/login
// ============================================================

#[derive(Template)]
#[template(path = "client_login.html")]
/// Self-identification form template (`templates/client_login.html`).
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
/// One row of the redeemable-products table: a `products` entry joined with
/// its `redeemable_products` price.
struct RedeemableProductView {
    /// `products.id` to send back in redemption items.
    product_id: i32,
    /// `products.name` display label.
    name: String,
    /// `products.description` display subtitle.
    description: String,
    /// `redeemable_products.points_needed` cost per unit.
    points_needed: i32,
}

#[derive(Template)]
#[template(path = "client_redeem.html")]
/// Points dashboard template (`templates/client_redeem.html`).
struct ClientRedeemTemplate {
    /// `clients.id` of the dashboard owner (also the redeem route id).
    client_id: i32,
    /// `clients.ci` display value.
    ci: i32,
    /// `clients.first_name` display value.
    first_name: String,
    /// `clients.last_name` display value.
    last_name: String,
    /// Sum of `point_earnings` over his tickets.
    total_earned_points: i32,
    /// Spendable points (equals earned until redemptions persist).
    balance_points: i32,
    /// Redeemable catalog, ordered by product name.
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

/// Renders `template`, falling back to a static error page when Askama fails.
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
