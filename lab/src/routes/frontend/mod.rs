//! # Server-rendered client pages (Askama + HTMX).
//!
//! Askama + HTMX interface for end customers: OTP self-identification at
//! `GET /clients/login` and the points-redemption dashboard at
//! `GET /clients/{client_id}/redeem`.
//!
//! All browser interaction is plain HTMX form posts returning HTML fragments —
//! no `fetch`, no `localStorage`, no inline JWT handling:
//!
//! * `POST /clients/otp/request` (form `ci`) answers the verify-form fragment.
//! * `POST /clients/otp/verify` (form `ci`, `code`, optional `next`) sets the
//!   `lab_client_token` session cookie and answers `HX-Redirect` to the redeem
//!   page, so the cookie carries auth from then on.
//! * The redeem page renders identity + balances server-side (it sits behind
//!   [`client_page_auth`], which already scope-checks the cookie, so rendering
//!   PII here is safe) and `POST /clients/{client_id}/redeem` (form
//!   `qty_<product_id>` fields) re-renders the whole dashboard region with
//!   fresh balances, the result message and per-row affordability (rows one
//!   unit of which the new balance can't cover render disabled).
//!
//! The JSON REST API ([`crate::routes::api`]) is untouched and remains
//! available for programmatic clients.
//!
//! The redeem page itself sits behind
//! [`client_page_auth`]: unauthenticated `GET`
//! navigations never render and are bounced with `303 See Other` to
//! `/clients/login?next=<path>`.
use std::{collections::HashMap, sync::Arc};

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    http::{StatusCode, header},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use chrono::{Duration, Utc};
use tracing::error;

use crate::{
    AppState, db,
    jwt::{CLIENT_SESSION_COOKIE, ClientClaims, client_page_auth, create_token},
    otp::{self, OtpError},
};

/// Builds the frontend router: `/`, `/clients/login`,
/// `POST /clients/otp/*` (all public) and
/// `GET|POST /clients/{client_id}/redeem` (behind [`client_page_auth`], which
/// redirects unauthenticated `GET` navigations to the login page).
///
/// # Errors
///
/// Currently infallible; returns [`anyhow::Error`] to allow future fallible setup.
pub fn create_frontend() -> anyhow::Result<Router<Arc<AppState>>> {
    let protected = Router::new()
        .route(
            "/clients/{client_id}/redeem",
            get(client_redeem_page).post(redeem_fragment),
        )
        .route_layer(middleware::from_fn(client_page_auth));
    Ok(Router::new()
        .route("/", get(root))
        .route("/clients/login", get(client_login_page))
        .route("/clients/otp/request", post(otp_request_fragment))
        .route("/clients/otp/verify", post(otp_verify_fragment))
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

/// Minimal HTML escaper for values interpolated into fragment strings
/// (Askama templates autoescape; these raw `Html<String>` fragments do not).
fn esc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// User-facing error paragraph shared by the HTMX fragments (always `200 OK`
/// so HTMX swaps it into the target).
fn fragment_error(message: &str) -> Html<String> {
    Html(format!(
        "<p class=\"text-sm mt-3\" role=\"alert\">{}</p>",
        esc(message)
    ))
}

/// `?next=` passthrough: only same-site client pages are honored, everything
/// else falls back to `default`.
fn safe_next(raw: &str, default: &str) -> String {
    if raw.starts_with("/clients/") {
        raw.to_string()
    } else {
        default.to_string()
    }
}

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
// + HTMX fragments POST /clients/otp/request|verify
// ============================================================

#[derive(Template)]
#[template(path = "client_login.html")]
/// Self-identification form template (`templates/client_login.html`).
struct ClientLoginTemplate {
    /// Sanitized `?next=` target the verify step redirects back to.
    next: String,
}

/// Login page where a client identifies with his CI and proves ownership of
/// his SMS/email channel with a one-time code. Step 1 posts the CI to
/// `POST /clients/otp/request` (HTMX fragment), step 2 posts CI + code to
/// `POST /clients/otp/verify`, which sets the session cookie and answers
/// `HX-Redirect` to the redeem page.
pub async fn client_login_page(Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let next = params.get("next").map_or(String::new(), |raw| {
        if raw.starts_with("/clients/") {
            raw.clone()
        } else {
            String::new()
        }
    });
    render_or_error(ClientLoginTemplate { next })
}

/// `POST /clients/otp/request` — HTMX fragment: sends the login code.
///
/// Takes a form (`ci`, optional `next`), dispatches the 6-digit code over the
/// client's channel and answers `200 OK` with either an error paragraph or
/// the verify-form fragment (with `ci`/`next` echoed as hidden inputs, so no
/// browser-side state is needed).
pub async fn otp_request_fragment(
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let next: String = form.get("next").cloned().unwrap_or_default();
    let ci: i32 = match form.get("ci").map(|s| s.trim().parse::<i32>()) {
        Some(Ok(ci)) if ci >= 0 => ci,
        _ => {
            return fragment_error("No se pudo enviar el código. Verificá tu CI.").into_response();
        }
    };

    let client = match db::client_by_ci(&state.db, ci).await {
        Ok(client) => client,
        Err(db::DbError::NotFound { .. }) => {
            return fragment_error("Ese CI no está registrado. Pedí en tu farmacia que te registren.")
                .into_response();
        }
        Err(e) => {
            error!("otp_request_fragment: client lookup failed: {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo enviar el código. Verificá tu CI."),
            )
                .into_response();
        }
    };
    let (channel, destination) = match otp::channel_for(&client, None) {
        Ok(pair) => pair,
        Err(_) => {
            return fragment_error("No se pudo enviar el código. Verificá tu CI.").into_response();
        }
    };
    if let Err(e) = state.otp_guard().check_and_record_send(client.ci) {
        let message = match e {
            OtpError::Locked { .. } | OtpError::TooManyRequests => {
                "Demasiados intentos. Esperá unos minutos y probá de nuevo."
            }
            _ => "No se pudo enviar el código. Verificá tu CI.",
        };
        return fragment_error(message).into_response();
    }

    let secret = match otp::otp_secret() {
        Ok(secret) => secret,
        Err(e) => {
            error!("otp_request_fragment: secret error: {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo enviar el código. Verificá tu CI."),
            )
                .into_response();
        }
    };
    let code = otp::generate_code(&secret, client.ci, otp::current_window());
    if let Err(e) = otp::send_otp(channel, &destination, &code).await {
        error!("otp_request_fragment: dispatch failed: {e:?}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            fragment_error("No se pudo enviar el código. Verificá tu CI."),
        )
            .into_response();
    }

    let masked = match channel {
        otp::OtpChannel::Sms => otp::mask_phone(&destination),
        otp::OtpChannel::Email => otp::mask_email(&destination),
    };
    let next_input = if next.starts_with("/clients/") {
        format!(
            "<input type=\"hidden\" name=\"next\" value=\"{}\" />",
            esc(next.as_str())
        )
    } else {
        String::new()
    };
    Html(format!(
        "<p class=\"text-sm opacity-80\">Código enviado por {} a {}.</p>\
         <form hx-post=\"/clients/otp/verify\" hx-target=\"#verify-result\" hx-swap=\"innerHTML\" class=\"flex flex-col gap-3 mt-4\">\
           <input type=\"hidden\" name=\"ci\" value=\"{ci}\" />\
           {next_input}\
           <label class=\"flex flex-col gap-1\">\
             <span>Código de 6 dígitos</span>\
             <input class=\"border rounded px-3 py-2 font-mono\" type=\"text\" name=\"code\" inputmode=\"numeric\" pattern=\"[0-9]{{6}}\" maxlength=\"6\" required placeholder=\"Ej. 482913\" />\
           </label>\
           <button class=\"border rounded px-3 py-2 font-semibold\" type=\"submit\">Ingresar</button>\
         </form>\
         <div id=\"verify-result\"></div>",
        esc(channel.name()),
        esc(&masked),
    ))
    .into_response()
}

/// `POST /clients/otp/verify` — HTMX fragment: exchanges the code for a session.
///
/// Takes a form (`ci`, `code`, optional `next`). Failures answer `200 OK`
/// with an error paragraph swapped into the form target; success sets the
/// `lab_client_token` cookie and answers `HX-Redirect` to `next` (when it
/// points at our own client pages) or to the owner's redeem page.
pub async fn otp_verify_fragment(
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let ci: i32 = match form.get("ci").map(|s| s.trim().parse::<i32>()) {
        Some(Ok(ci)) if ci >= 0 => ci,
        _ => {
            return fragment_error("Código inválido o vencido. Pedí uno nuevo si hace falta.")
                .into_response();
        }
    };
    let code = form.get("code").map_or(String::new(), |s| s.trim().to_string());
    let next: String = form.get("next").cloned().unwrap_or_default();
    if code.is_empty() {
        return fragment_error("Código inválido o vencido. Pedí uno nuevo si hace falta.")
            .into_response();
    }
    if let Err(e) = state.otp_guard().check_not_locked(ci) {
        let message = match e {
            OtpError::Locked { .. } => "Cuenta bloqueada por intentos fallidos. Esperá 15 minutos.",
            _ => "Código inválido o vencido. Pedí uno nuevo si hace falta.",
        };
        return fragment_error(message).into_response();
    }

    let secret = match otp::otp_secret() {
        Ok(secret) => secret,
        Err(e) => {
            error!("otp_verify_fragment: secret error: {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo verificar el código. Probá de nuevo."),
            )
                .into_response();
        }
    };
    let ok = match otp::verify_code(&secret, ci, &code) {
        Ok(ok) => ok,
        Err(e) => {
            error!("otp_verify_fragment: verify error: {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo verificar el código. Probá de nuevo."),
            )
                .into_response();
        }
    };
    state.otp_guard().record_result(ci, ok);
    if !ok {
        // A fresh lockout may have just kicked in after this failure.
        if matches!(
            state.otp_guard().check_not_locked(ci),
            Err(OtpError::Locked { .. })
        ) {
            return fragment_error("Cuenta bloqueada por intentos fallidos. Esperá 15 minutos.")
                .into_response();
        }
        return fragment_error("Código inválido o vencido. Pedí uno nuevo si hace falta.")
            .into_response();
    }

    let client = match db::client_by_ci(&state.db, ci).await {
        Ok(client) => client,
        Err(e) => {
            error!("otp_verify_fragment: client lookup failed: {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo verificar el código. Probá de nuevo."),
            )
                .into_response();
        }
    };
    let token = match create_token(
        ClientClaims::new(client.id, client.ci, Utc::now() + Duration::hours(1)).into(),
    ) {
        Ok(token) => token,
        Err(e) => {
            error!("otp_verify_fragment: token error: {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo verificar el código. Probá de nuevo."),
            )
                .into_response();
        }
    };
    let target = safe_next(next.as_str(), &format!("/clients/{}/redeem", client.id));
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        format!("{CLIENT_SESSION_COOKIE}={token}; Path=/; SameSite=Lax; Max-Age=3600")
            .parse()
            .unwrap_or_else(|_| header::HeaderValue::from_static("")),
    );
    headers.insert(
        "HX-Redirect",
        target.parse().unwrap_or_else(|_| header::HeaderValue::from_static("/")),
    );
    (StatusCode::OK, headers, "").into_response()
}

// ============================================================
// Client point redemption: GET|POST /clients/{client_id}/redeem
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
    /// `false` when a single unit already costs more than the current
    /// balance: the row renders `disabled` so it can't be picked.
    affordable: bool,
}

#[derive(Debug, Clone)]
/// Everything the redeem dashboard region needs: identity, ledger-derived
/// totals, the catalog with per-row affordability and the result message.
///
/// Rendered both by the page (`templates/client_redeem.html`, via
/// `{% include %}` of the shared partial) and by the redeem fragment, so a
/// redemption always swaps in fully fresh numbers — no out-of-band updates.
struct RedeemDashboardView {
    /// `clients.id` of the dashboard owner (also the redeem route id).
    client_id: i32,
    /// Owner first name.
    first_name: String,
    /// Owner last name.
    last_name: String,
    /// Owner CI.
    ci: i32,
    /// Spendable points (`earned − redeemed − cancelled`).
    balance_points: i32,
    /// Lifetime earned points.
    total_earned_points: i32,
    /// Lifetime redeemed points.
    total_redeemed_points: i32,
    /// Awarded points reversed by cancellations.
    cancelled_points: i32,
    /// Redeemable catalog, ordered by product name.
    products: Vec<RedeemableProductView>,
    /// `true` when the catalog is non-empty but nothing is affordable: the
    /// submit button renders `disabled` too.
    all_disabled: bool,
    /// Result message of the last redemption (`None` on page load).
    message: Option<String>,
    /// `true` renders the message as `role="alert"`, else `role="status"`.
    message_is_error: bool,
}

/// Builds the dashboard view from a ledger dashboard plus the catalog,
/// flagging rows whose single-unit price the `balance` can't cover.
///
/// `message` carries the result text and its severity (`(text, is_error)`).
fn dashboard_view(
    dashboard: crate::routes::api::dto::ClientDataResponse,
    catalog: Vec<db::CatalogRow>,
    message: Option<(String, bool)>,
) -> RedeemDashboardView {
    let balance = dashboard.balance_points;
    let products: Vec<RedeemableProductView> = catalog
        .into_iter()
        .map(|row| {
            let affordable = row.points_needed <= balance;
            RedeemableProductView {
                product_id: row.product_id,
                name: row.name,
                description: row.description,
                points_needed: row.points_needed,
                affordable,
            }
        })
        .collect();
    let all_disabled = !products.is_empty() && products.iter().all(|p| !p.affordable);
    let (message, message_is_error) = match message {
        Some((text, is_error)) => (Some(text), is_error),
        None => (None, false),
    };
    RedeemDashboardView {
        client_id: dashboard.client.id,
        first_name: dashboard.client.first_name,
        last_name: dashboard.client.last_name,
        ci: dashboard.client.ci,
        balance_points: dashboard.balance_points,
        total_earned_points: dashboard.total_earned_points,
        total_redeemed_points: dashboard.total_redeemed_points,
        cancelled_points: dashboard.cancelled_points,
        products,
        all_disabled,
        message,
        message_is_error,
    }
}

#[derive(Template)]
#[template(path = "client_redeem.html")]
/// Points dashboard page (`templates/client_redeem.html`).
///
/// Rendered behind [`client_page_auth`] (same-client cookie scope), so
/// identity and balances render server-side — no browser fetch needed.
struct ClientRedeemTemplate {
    /// Dashboard region (the shared `_dashboard.html` partial reads `dash`).
    dash: RedeemDashboardView,
}

#[derive(Template)]
#[template(path = "_dashboard.html")]
/// Redeem dashboard region fragment (`templates/_dashboard.html`).
///
/// Returned by [`redeem_fragment`] with a fresh balance and message; the
/// form swaps it over `#dashboard` via `outerHTML`.
struct RedeemDashboardFragment {
    /// Dashboard region (same shape as the page include).
    dash: RedeemDashboardView,
}

/// Dashboard where an identified client sees their point balance and
/// redeems points for `redeemable_products`. The [`client_page_auth`]
/// layer guarantees the cookie client equals the path id before this
/// handler runs, so profile + ledger totals render server-side.
///
/// # Errors
///
/// Returns [`StatusCode::NOT_FOUND`] for an unknown client id and
/// [`StatusCode::INTERNAL_SERVER_ERROR`] on DB failures.
pub async fn client_redeem_page(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<i32>,
) -> Result<impl IntoResponse, StatusCode> {
    let dashboard = db::client_dashboard(&state.db, client_id)
        .await
        .map_err(|e| match e {
            db::DbError::NotFound { .. } => StatusCode::NOT_FOUND,
            _ => {
                error!("client_redeem_page: dashboard failed: {e:?}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;
    let catalog = db::redeemable_catalog(&state.db).await.map_err(|e| {
        error!("client_redeem_page: catalog failed: {e:?}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let template = ClientRedeemTemplate {
        dash: dashboard_view(dashboard, catalog, None),
    };
    Ok(render_or_error(template))
}

/// `POST /clients/{client_id}/redeem` — HTMX fragment: redeems points.
///
/// Takes a form with `qty_<product_id>` fields (only quantities `>= 1`
/// become redemption lines) and always answers `200 OK` with the freshly
/// rendered dashboard region: new balance/breakdown, the result message and
/// updated row affordability. The form swaps it over `#dashboard` via
/// `outerHTML`, so balances update immediately with no out-of-band plumbing.
/// Runs behind [`client_page_auth`], so the cookie client already matches
/// `client_id`.
pub async fn redeem_fragment(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<i32>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut items = Vec::new();
    for (key, value) in &form {
        let Some(product_id) = key
            .strip_prefix("qty_")
            .and_then(|rest| rest.parse::<i32>().ok())
        else {
            continue;
        };
        let qty = value.trim().parse::<i32>().unwrap_or(0);
        if qty >= 1 && product_id >= 1 {
            items.push(db::RedeemLine {
                product_id,
                quantity: qty,
            });
        }
    }
    if items.is_empty() {
        return render_dashboard(
            &state,
            client_id,
            Some((
                "Elegí al menos un producto con cantidad mayor a cero.".to_string(),
                true,
            )),
        )
        .await;
    }

    match db::redeem_points(&state.db, client_id, &items).await {
        Ok(redeemed) => {
            render_dashboard(
                &state,
                client_id,
                Some((
                    format!(
                        "Canje exitoso: {} puntos. Saldo restante: {}.",
                        redeemed.redeemed_points, redeemed.remaining_points,
                    ),
                    false,
                )),
            )
            .await
        }
        Err(e) => match &e {
            db::DbError::Conflict { .. } => {
                render_dashboard(
                    &state,
                    client_id,
                    Some((
                        "No tenés puntos suficientes para este canje.".to_string(),
                        true,
                    )),
                )
                .await
            }
            db::DbError::Unprocessable { .. }
            | db::DbError::Invalid { .. }
            | db::DbError::NotFound { .. } => {
                render_dashboard(
                    &state,
                    client_id,
                    Some((
                        "No se pudo completar el canje. Verificá los productos.".to_string(),
                        true,
                    )),
                )
                .await
            }
            _ => {
                error!("redeem_fragment: redemption failed: {e:?}");
                render_dashboard(
                    &state,
                    client_id,
                    Some((
                        "No se pudo completar el canje. Verificá los productos.".to_string(),
                        true,
                    )),
                )
                .await
            }
        },
    }
}

/// Reloads the dashboard + catalog and renders the `_dashboard.html` region
/// with `message` (`(text, is_error)`).
///
/// All outcomes answer `200 OK` with a fully fresh region so the HTMX swap
/// always shows current numbers; only when the dashboard itself can't load
/// (unknown client / DB down) does this fall back to a plain status +
/// error paragraph.
async fn render_dashboard(
    state: &AppState,
    client_id: i32,
    message: Option<(String, bool)>,
) -> Response {
    let dashboard = match db::client_dashboard(&state.db, client_id).await {
        Ok(dashboard) => dashboard,
        Err(db::DbError::NotFound { .. }) => {
            return (
                StatusCode::NOT_FOUND,
                fragment_error("Cliente no encontrado."),
            )
                .into_response();
        }
        Err(e) => {
            error!("render_dashboard: dashboard failed: {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo completar el canje. Probá de nuevo."),
            )
                .into_response();
        }
    };
    let catalog = match db::redeemable_catalog(&state.db).await {
        Ok(catalog) => catalog,
        Err(e) => {
            error!("render_dashboard: catalog failed: {e:?}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo completar el canje. Probá de nuevo."),
            )
                .into_response();
        }
    };
    let fragment = RedeemDashboardFragment {
        dash: dashboard_view(dashboard, catalog, message),
    };
    match fragment.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            error!("render_dashboard: render failed: {e:?}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                fragment_error("No se pudo completar el canje. Probá de nuevo."),
            )
                .into_response()
        }
    }
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
