//! # Server-rendered client pages.
//!
//! Askama + HTMX interface for end customers: OTP self-identification at
//! `GET /clients/login` and the points-redemption dashboard at
//! `GET /clients/{client_id}/redeem`.
//!
//! Privacy rule: the redeem page is a PII-free shell (redeemable catalog
//! only). Identity, CI and point totals are fetched in the browser from the
//! JSON REST API ([`crate::routes::api`]) with the client JWT from
//! `localStorage`, so no credential ever renders server-side without the
//! [`client_auth`](crate::jwt::client_auth) layer having checked it.
//!
//! The redeem page itself sits behind
//! [`client_page_auth`]: unauthenticated `GET`
//! navigations never render and are bounced with `303 See Other` to
//! `/clients/login?next=<path>`.
use std::sync::Arc;

use askama::Template;
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{Html, IntoResponse},
    routing::get,
};
use tracing::error;

use crate::{AppState, db, jwt::client_page_auth};

/// Builds the frontend router: `/`, `/clients/login` (both public) and
/// `/clients/{client_id}/redeem` (behind [`client_page_auth`], which redirects
/// unauthenticated `GET` navigations to the login page).
///
/// # Errors
///
/// Currently infallible; returns [`anyhow::Error`] to allow future fallible setup.
pub fn create_frontend() -> anyhow::Result<Router<Arc<AppState>>> {
    let protected = Router::new()
        .route("/clients/{client_id}/redeem", get(client_redeem_page))
        .route_layer(middleware::from_fn(client_page_auth));
    Ok(Router::new()
        .route("/", get(root))
        .route("/clients/login", get(client_login_page))
        .merge(protected))
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

/// Login page where a client identifies with his CI and proves ownership of
/// his SMS/email channel with a one-time code. Step 1 calls
/// `POST /api/v1/clients/otp/request`, step 2 calls
/// `POST /api/v1/clients/otp/verify`, which stores the JWT (1hr expiry) in
/// `localStorage` and redirects to the redeem page.
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
/// Points dashboard shell template (`templates/client_redeem.html`).
///
/// Carries no PII: only the dashboard owner id (for the JS fetch URL) and
/// the public redeemable catalog. Names, CI and balances load client-side
/// through the authenticated API.
struct ClientRedeemTemplate {
    /// `clients.id` of the dashboard owner (also the redeem route id).
    client_id: i32,
    /// Redeemable catalog, ordered by product name.
    products: Vec<RedeemableProductView>,
}

/// Dashboard shell where an identified client sees their point balance and
/// redeems points for `redeemable_products`. Only the public catalog renders
/// server-side; the client profile and ledger totals load in the browser via
/// `GET /api/v1/clients/{client_id}` with the stored JWT (which the
/// [`client_auth`](crate::jwt::client_auth) layer scopes to the owner), so
/// this page leaks no PII to unauthenticated viewers.
///
/// # Errors
///
/// Returns [`StatusCode::NOT_FOUND`] for an unknown client id and
/// [`StatusCode::INTERNAL_SERVER_ERROR`] on DB failures.
pub async fn client_redeem_page(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<i32>,
) -> Result<impl IntoResponse, StatusCode> {
    // Existence check only: nothing about the client renders here.
    db::client_by_id(&state.db, client_id)
        .await
        .map_err(|e| match e {
            db::DbError::NotFound { .. } => StatusCode::NOT_FOUND,
            _ => {
                error!("client_redeem_page: client lookup failed: {e:?}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;
    let catalog = db::redeemable_catalog(&state.db).await.map_err(|e| {
        error!("client_redeem_page: catalog failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let template = ClientRedeemTemplate {
        client_id,
        products: catalog
            .into_iter()
            .map(|row| RedeemableProductView {
                product_id: row.product_id,
                name: row.name,
                description: row.description,
                points_needed: row.points_needed,
            })
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
