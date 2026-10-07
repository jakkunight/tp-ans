//! post-ticket — post tickets (facturas) to the lab API from the terminal.
//!
//! Two modes:
//! * One-shot CLI (parity with `lab/scripts/post-ticket.sh`):
//!   `post-ticket --partner 1 --ci 1234567 --first-name María --last-name González --item 1:2`
//! * Interactive TUI (default when no action flags are given, or `--tui`):
//!   a scrollable, collapsible Ratatui form with unlimited product lines.
//!
//! HTTP is done with [`reqwest`], the interface with [`ratatui`]. Request
//! payloads mirror `lab/src/routes/api/dto.rs`; see [`Args`] for the flags
//! and [`App`] for the TUI state.

use anyhow::{Context, Result, bail};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};
use std::io::Stdout;

// ============================================================
// Payloads (mirror `routes::api::dto`)
// ============================================================

/// One `ticket_details` line: `product_id` purchased in `quantity` units.
#[derive(Debug, Clone, Serialize)]
struct TicketDetail {
    /// `products.id` of the purchased product.
    product_id: i32,
    /// Units purchased (`>= 1`).
    quantity: i32,
}

/// Buyer identity from the factura; the server registers him when unknown.
/// New clients need at least one contact (the schema requires an OTP channel).
#[derive(Debug, Clone, Serialize)]
struct TicketClient {
    /// `clients.ci` (unique) lookup key.
    ci: i32,
    /// Optional `<ci>-<digit>` RUC check digit (mod-11 must match `ci`).
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_digit: Option<i32>,
    /// Buyer first name as printed on the factura.
    first_name: String,
    /// Buyer last name as printed on the factura.
    last_name: String,
    /// Company name for juridical receptors.
    #[serde(skip_serializing_if = "Option::is_none")]
    razon_social: Option<String>,
    /// Receptor address printed on the factura.
    #[serde(skip_serializing_if = "Option::is_none")]
    domicilio: Option<String>,
    /// SMS channel for OTP codes; omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    phone_number: Option<String>,
    /// Email channel for OTP codes; omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
}

/// Body for `POST /api/v1/partners/tickets`.
///
/// Fiscal identity: paper sends only `ticket_id` (`EEE-PPP-NNNNNNN`);
/// timbrado paper adds `timbrado` (8 digits); electronic adds `timbrado` +
/// `cdc` (44 digits).
#[derive(Debug, Clone, Serialize)]
struct CreateTicketRequest {
    /// `partners.id` of the issuing pharmacy.
    partner_id: i32,
    /// Printed invoice number `EEE-PPP-NNNNNNN` (unique per partner).
    ticket_id: String,
    /// 8-digit DNIT timbrado (timbrado paper + electronic only).
    #[serde(skip_serializing_if = "Option::is_none")]
    timbrado: Option<String>,
    /// 44-digit SIFEN CDC (electronic only; requires `timbrado`).
    #[serde(skip_serializing_if = "Option::is_none")]
    cdc: Option<String>,
    /// Buyer data; registered on the fly when the CI is unknown.
    client: TicketClient,
    /// At least one product line.
    details: Vec<TicketDetail>,
}

/// Body for `DELETE /api/v1/partners/tickets`.
///
/// Cancellation is an audit entry: the rows stay intact, a cancellation is
/// recorded with the `reason`, and awarded points are reversed.
#[derive(Debug, Clone, Serialize)]
struct DeleteTicketRequest {
    /// `tickets.id` of the invoice to void.
    ticket_id: i32,
    /// Why the invoice is voided (`varchar(256)`).
    reason: String,
}

/// Mirrors `routes::api::dto::PartnerLoginRequest`:
/// partners authenticate with their RUC (natural key) plus the pre-shared
/// key stored as `partners.psk_hash`.
#[derive(Debug, Clone, Serialize)]
struct PartnerLoginRequest {
    /// `partners.ruc` of the pharmacy logging in.
    ruc: String,
    /// Pre-shared key; verified against `psk_hash`, never stored.
    psk: String,
}

/// Successful login reply (`LoginResponse`): the JWT to reuse as a bearer token.
#[derive(Debug, Clone, Deserialize)]
struct LoginReply {
    /// Compact JWT for `Authorization: Bearer <token>`.
    token: String,
}

// ============================================================
// CLI args (manual parsing, no extra deps)
// ============================================================

/// Parsed command-line flags (manual parsing, no extra deps).
#[derive(Debug, Default)]
struct Args {
    /// Force the interactive TUI even with action flags present.
    tui: bool,
    /// `--partner`: issuing pharmacy id.
    partner: Option<String>,
    /// `--ci`: buyer CI from the factura.
    ci: Option<String>,
    /// `--first-name`: buyer first name.
    first_name: Option<String>,
    /// `--last-name`: buyer last name.
    last_name: Option<String>,
    /// `--verification-digit`: optional RUC suffix digit.
    verification_digit: Option<String>,
    /// `--ticket-id`: printed invoice number EEE-PPP-NNNNNNN.
    ticket_id: Option<String>,
    /// `--timbrado`: 8-digit DNIT authorization (timbrado + electronic).
    timbrado: Option<String>,
    /// `--cdc`: 44-digit SIFEN CDC (electronic only).
    cdc: Option<String>,
    /// `--razon-social`: company name for juridical receptors.
    razon_social: Option<String>,
    /// `--domicilio`: receptor address printed on the factura.
    domicilio: Option<String>,
    /// `--phone`: buyer SMS channel (for new clients).
    phone: Option<String>,
    /// `--email`: buyer email channel (for new clients).
    email: Option<String>,
    /// `--reason`: void reason for `--delete`.
    reason: Option<String>,
    /// `--item` values (`PID:QTY`, repeatable).
    items: Vec<String>,
    /// `--base-url`: API base URL override.
    base_url: Option<String>,
    /// `--token`: bearer JWT override.
    token: Option<String>,
    /// `--login-partner`: partner RUC to log in with.
    login_partner: Option<String>,
    /// `--login-secret`: optional login credential.
    login_secret: Option<String>,
    /// `--delete`: ticket id to void.
    delete: Option<String>,
    /// `--dry-run`: print the JSON body without sending it.
    dry_run: bool,
    /// `-h` / `--help`: print [`HELP`].
    help: bool,
}

/// One-shot usage text, also printed for `--help`.
const HELP: &str = r#"post-ticket — post tickets to the lab API from the terminal.

Usage (one-shot):
  post-ticket --partner ID --ticket-id NRO --ci CI --first-name NAME --last-name NAME --item PID:QTY [...] [options]
  post-ticket --login-partner RUC --login-secret PSK   # partner login only
  post-ticket --login-partner RUC --login-secret PSK --partner ID --ticket-id NRO --ci CI ...  # login, then post
  post-ticket --delete TICKET_ID --reason MOTIVO [options]

Usage (interactive TUI):
  post-ticket [--tui] [--base-url URL] [--token TOKEN]

Options:
  --partner ID     Partner (farmacia) id. Required unless --delete/--login-partner alone.
  --ticket-id NRO  Printed invoice number EEE-PPP-NNNNNNN (unique per partner).
                   Required to post. Example: 001-001-0000009
  --timbrado T     8-digit DNIT timbrado (timbrado paper + electronic only).
  --cdc CDC        44-digit SIFEN CDC (electronic only; requires --timbrado).
  --ci CI          Buyer CI from the factura (client is registered if new).
                   Required unless --delete/--login-partner alone.
  --first-name N   Buyer first name as printed on the factura. Required unless --delete/--login-partner alone.
  --last-name N    Buyer last name as printed on the factura. Required unless --delete/--login-partner alone.
  --verification-digit D
                   Optional <CI>-<D> RUC check digit (0-9, mod-11 must match CI).
  --razon-social R Company name for juridical receptors (optional).
  --domicilio DIR  Receptor address printed on the factura (optional).
  --phone NUM      Buyer phone (SMS channel for OTP). New clients need --phone and/or --email.
  --email ADDR     Buyer email (channel for OTP). New clients need --phone and/or --email.
  --login-partner RUC
                   Log the partner in (POST /api/v1/partners/login) and print
                   the JWT. Combined with --partner/--delete it authenticates
                   that same invocation instead of --token.
  --login-secret S Pre-shared partner key for the login (required with --login-partner).
  --item PID:QTY   One ticket line; repeatable. At least one required.
                   Example: --item 1:2 --item 3:1
  --base-url URL   API base URL. Default: $LAB_BASE_URL or http://127.0.0.1:8080
  --token TOKEN    Bearer JWT for the ticket routes (required). Default:
                   $LAB_JWT_TOKEN; use --login-partner to mint one.
  --delete ID      Void ticket ID instead of posting (DELETE /partners/tickets).
  --reason MOTIVO  Why the invoice is voided. Required with --delete.
  --dry-run        Print the JSON body without sending it.
  --tui            Force the interactive Ratatui interface.
  -h, --help       Show this help plus the seed-data IDs.

TUI keys:
  Tab / Down / Up    Moverse entre campos y secciones
  Espacio            Plegar/desplegar la sección enfocada
  + / - (Supr)       Añadir/quitar línea de producto (sobre una línea)
  RePág / AvPág      Desplazar el formulario una página
  Enter              Siguiente campo (Login/Post/Anular en botones)
  Ctrl+L             Partner login         Ctrl+P           Post ticket
  Ctrl+D             Void ticket           Esc / Ctrl+C     Quit

Seed data (lab/database/seed.sql):
  Partners: 1 Farmacia Central (RUC 80012345-0), 2 Farmacia del Sur (RUC 80067890-7)
  Clients:  12 clientes (1 María González CI 1234567, 2 Juan Pérez CI 2345678, ...)
  Products: 12 (1 Paracetamol 10 pts, 2 Ibuprofeno 8 pts, 3 Vitamina C 5 pts,
            4 Crema 3 pts, 7 Alcohol 6 pts, 8 Jabón 4 pts, 9 Protector 12 pts;
            canje (todos con precio): 1 Paracet 10, 2 Ibupr 8, 3 Vitam 50,
            4 Crema 3, 5 Termo 100, 6 Mochila 250, 7 Alcohol 6, 8 Jabón 4,
            9 Protector 12, 10 Termo Dep. 150, 11 Gorra 80, 12 Kit 300)
  Tickets:  12 demo (partner 1: 001-001-0000001..08, partner 2: 002-001-0000001..04)
"#;

/// Parses `std::env::args` into [`Args`]; unknown flags are an error.
fn parse_args() -> Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1).peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--tui" => args.tui = true,
            "--partner" => args.partner = Some(it.next().context("--partner needs a value")?),
            "--ci" => args.ci = Some(it.next().context("--ci needs a value")?),
            "--first-name" => {
                args.first_name = Some(it.next().context("--first-name needs a value")?)
            }
            "--last-name" => args.last_name = Some(it.next().context("--last-name needs a value")?),
            "--verification-digit" => {
                args.verification_digit =
                    Some(it.next().context("--verification-digit needs a value")?)
            }
            "--ticket-id" => args.ticket_id = Some(it.next().context("--ticket-id needs a value")?),
            "--timbrado" => args.timbrado = Some(it.next().context("--timbrado needs a value")?),
            "--cdc" => args.cdc = Some(it.next().context("--cdc needs a value")?),
            "--razon-social" => {
                args.razon_social = Some(it.next().context("--razon-social needs a value")?)
            }
            "--domicilio" => args.domicilio = Some(it.next().context("--domicilio needs a value")?),
            "--phone" => args.phone = Some(it.next().context("--phone needs a value")?),
            "--email" => args.email = Some(it.next().context("--email needs a value")?),
            "--reason" => args.reason = Some(it.next().context("--reason needs a value")?),
            "--item" => args.items.push(it.next().context("--item needs a value")?),
            "--base-url" => args.base_url = Some(it.next().context("--base-url needs a value")?),
            "--login-partner" => {
                args.login_partner = Some(it.next().context("--login-partner needs a value")?)
            }
            "--login-secret" => {
                args.login_secret = Some(it.next().context("--login-secret needs a value")?)
            }
            "--token" => args.token = Some(it.next().context("--token needs a value")?),
            "--delete" => args.delete = Some(it.next().context("--delete needs a value")?),
            "--dry-run" => args.dry_run = true,
            "-h" | "--help" => args.help = true,
            other => bail!("unknown argument: {other} (see --help)"),
        }
    }
    Ok(args)
}

/// Resolves the API base URL: `--base-url`, then `LAB_BASE_URL`, then the
/// `127.0.0.1:8080` default (trailing slashes stripped).
fn base_url(args: &Args) -> String {
    args.base_url
        .clone()
        .or_else(|| std::env::var("LAB_BASE_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Resolves the bearer JWT: `--token`, then `LAB_JWT_TOKEN`, else empty
/// (the ticket routes reject unauthenticated calls with 401).
fn token(args: &Args) -> String {
    args.token
        .clone()
        .or_else(|| std::env::var("LAB_JWT_TOKEN").ok())
        .unwrap_or_default()
}

/// Parses a `>= 1` id (partner, product, quantity, ticket).
fn parse_id(raw: &str, what: &str) -> Result<i32> {
    raw.trim()
        .parse::<i32>()
        .context(format!("{what} must be a number, got {raw:?}"))
        .and_then(|v| {
            if v >= 1 {
                Ok(v)
            } else {
                bail!("{what} must be >= 1, got {v}")
            }
        })
}

/// Parses a buyer CI (`>= 0`, per the `ci >= 0` column check).
fn parse_ci(raw: &str) -> Result<i32> {
    raw.trim()
        .parse::<i32>()
        .context(format!("ci must be a number, got {raw:?}"))
        .and_then(|v| {
            if v >= 0 {
                Ok(v)
            } else {
                bail!("ci must be >= 0, got {v}")
            }
        })
}

/// Parses a `<ci>-<digit>` RUC suffix digit (`0-9`).
fn parse_vdigit(raw: &str) -> Result<i32> {
    raw.trim()
        .parse::<i32>()
        .context(format!("verification digit must be a number, got {raw:?}"))
        .and_then(|v| {
            if (0..=9).contains(&v) {
                Ok(v)
            } else {
                bail!("verification digit must be 0-9, got {v}")
            }
        })
}

/// Requires a non-blank flag value (buyer names), trimmed.
fn non_empty(raw: Option<&str>, flag: &str) -> Result<String> {
    let v = raw
        .context(format!("{flag} is required"))?
        .trim()
        .to_string();
    if v.is_empty() {
        bail!("{flag} must not be empty");
    }
    Ok(v)
}

/// Requires a non-blank form value, trimmed.
fn non_empty_str(raw: &str, what: &str) -> Result<String> {
    let v = raw.trim().to_string();
    if v.is_empty() {
        bail!("{what} is required");
    }
    Ok(v)
}

/// Inputs for building a ticket buyer from CLI flags.
#[derive(Debug, Clone)]
struct BuyerFlags<'a> {
    ci: Option<&'a str>,
    first_name: Option<&'a str>,
    last_name: Option<&'a str>,
    verification_digit: Option<&'a str>,
    razon_social: Option<&'a str>,
    domicilio: Option<&'a str>,
    phone: Option<&'a str>,
    email: Option<&'a str>,
}

/// Builds the ticket buyer from raw flag/field values, validating CI, names
/// and the optional verification digit. Blank optionals become `None`.
fn build_client(flags: BuyerFlags<'_>) -> Result<TicketClient> {
    let contact = |raw: Option<&str>| {
        raw.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Ok(TicketClient {
        ci: parse_ci(flags.ci.context("--ci is required")?)?,
        verification_digit: flags.verification_digit.map(parse_vdigit).transpose()?,
        first_name: non_empty(flags.first_name, "--first-name")?,
        last_name: non_empty(flags.last_name, "--last-name")?,
        razon_social: contact(flags.razon_social),
        domicilio: contact(flags.domicilio),
        phone_number: contact(flags.phone),
        email: contact(flags.email),
    })
}

/// Accept items as `PID:QTY`, separated by commas and/or whitespace:
/// `"1:2, 3:1"` or `"1:2 3:1"`.
fn parse_items(raw_items: &[String]) -> Result<Vec<TicketDetail>> {
    let joined = raw_items.join(" ");
    let mut details = Vec::new();
    for part in joined
        .split([',', ' ', '\t', '\n'])
        .filter(|s| !s.is_empty())
    {
        let (pid, qty) = part
            .split_once(':')
            .context(format!("bad item {part:?}, expected PID:QTY (e.g. 1:2)"))?;
        details.push(TicketDetail {
            product_id: parse_id(pid, "product id")?,
            quantity: parse_id(qty, "quantity")?,
        });
    }
    if details.is_empty() {
        bail!("at least one --item PID:QTY is required");
    }
    Ok(details)
}

/// Split `--item` args into one `PID:QTY` string per product line for the
/// TUI (separators `,` / whitespace also split, like [`parse_items`]).
/// Always returns at least one (blank) line.
fn expand_item_lines(raw_items: &[String]) -> Vec<String> {
    let joined = raw_items.join(" ");
    let parts: Vec<String> = joined
        .split([',', ' ', '\t', '\n'])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        vec![String::new()]
    } else {
        parts
    }
}

/// Pretty-prints a JSON response body, passing non-JSON through untouched.
fn pretty_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string()))
        .unwrap_or_else(|_| raw.to_string())
}

// ============================================================
// HTTP via reqwest
// ============================================================

/// Builds a [`reqwest`] client with defaults.
fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .build()
        .context("failed to build HTTP client")
}

/// `POST`s a ticket; returns the HTTP status plus the pretty reply body.
/// Sends `Authorization: Bearer` only when `token` is non-empty.
async fn post_ticket_reqwest(
    base: &str,
    token: &str,
    req: &CreateTicketRequest,
) -> Result<(reqwest::StatusCode, String)> {
    let mut call = http_client()?
        .post(format!("{base}/api/v1/partners/tickets"))
        .json(req);
    if !token.is_empty() {
        call = call.bearer_auth(token);
    }
    let res = call.send().await.context("request failed")?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    Ok((status, pretty_json(&body)))
}

/// `DELETE`s a ticket by id with a void reason; returns the HTTP status plus
/// the pretty reply body.
async fn delete_ticket_reqwest(
    base: &str,
    token: &str,
    ticket_id: i32,
    reason: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let mut call = http_client()?
        .delete(format!("{base}/api/v1/partners/tickets"))
        .json(&DeleteTicketRequest {
            ticket_id,
            reason: reason.to_string(),
        });
    if !token.is_empty() {
        call = call.bearer_auth(token);
    }
    let res = call.send().await.context("request failed")?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    Ok((status, pretty_json(&body)))
}

/// Logs a partner in by RUC + pre-shared key; returns the HTTP status plus
/// the pretty reply body.
async fn partner_login_reqwest(
    base: &str,
    ruc: &str,
    psk: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let req = PartnerLoginRequest {
        ruc: ruc.to_string(),
        psk: psk.to_string(),
    };
    let res = http_client()?
        .post(format!("{base}/api/v1/partners/login"))
        .json(&req)
        .send()
        .await
        .context("request failed")?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    Ok((status, pretty_json(&body)))
}

/// Pull the JWT out of a pretty-printed login reply body.
fn extract_token(pretty_body: &str) -> Result<String> {
    serde_json::from_str::<LoginReply>(pretty_body)
        .map(|r| r.token)
        .context("login reply carried no token")
}

// ============================================================
// One-shot CLI mode
// ============================================================

/// Runs the one-shot CLI: optional login, then at most one delete/post action.
/// Prints `HTTP <status>` plus the pretty body; non-2xx is an error.
async fn run_oneshot(args: &Args) -> Result<()> {
    let base = base_url(args);
    let mut tok = token(args);

    // Optional partner login first: `--login-partner RUC` alone prints the
    // JWT; combined with --partner/--delete it authenticates this same
    // invocation (overriding --token).
    if let Some(ruc) = &args.login_partner {
        let psk = args
            .login_secret
            .as_deref()
            .context("--login-secret PSK is required with --login-partner")?;
        if args.dry_run {
            println!(
                "{}",
                serde_json::to_string_pretty(&PartnerLoginRequest {
                    ruc: ruc.clone(),
                    psk: psk.to_string(),
                })?
            );
            return Ok(());
        }
        let (status, resp) = partner_login_reqwest(&base, ruc, psk).await?;
        println!("LOGIN HTTP {status}\n{resp}");
        if !status.is_success() {
            bail!("login replied with {status}");
        }
        tok = extract_token(&resp)?;
        let login_only = args.delete.is_none()
            && args.partner.is_none()
            && args.ci.is_none()
            && args.first_name.is_none()
            && args.last_name.is_none()
            && args.items.is_empty();
        if login_only {
            println!("hint: export LAB_JWT_TOKEN='<token above>' to reuse it");
            return Ok(());
        }
    }

    if let Some(raw) = &args.delete {
        let ticket_id = parse_id(raw, "ticket id")?;
        let reason = non_empty(args.reason.as_deref(), "--reason")?;
        let body = serde_json::to_string_pretty(&DeleteTicketRequest {
            ticket_id,
            reason: reason.clone(),
        })?;
        if args.dry_run {
            println!("{body}");
            return Ok(());
        }
        let (status, resp) = delete_ticket_reqwest(&base, &tok, ticket_id, &reason).await?;
        println!("HTTP {status}\n{resp}");
        if !status.is_success() {
            bail!("server replied with {status}");
        }
        return Ok(());
    }

    let partner_id = parse_id(
        args.partner
            .as_deref()
            .context("--partner ID is required")?,
        "partner id",
    )?;
    let client = build_client(BuyerFlags {
        ci: args.ci.as_deref(),
        first_name: args.first_name.as_deref(),
        last_name: args.last_name.as_deref(),
        verification_digit: args.verification_digit.as_deref(),
        razon_social: args.razon_social.as_deref(),
        domicilio: args.domicilio.as_deref(),
        phone: args.phone.as_deref(),
        email: args.email.as_deref(),
    })?;
    let details = parse_items(&args.items)?;
    let blank = |raw: &Option<String>| {
        raw.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let req = CreateTicketRequest {
        partner_id,
        ticket_id: non_empty(args.ticket_id.as_deref(), "--ticket-id")?,
        timbrado: blank(&args.timbrado),
        cdc: blank(&args.cdc),
        client,
        details,
    };
    if args.dry_run {
        println!("{}", serde_json::to_string_pretty(&req)?);
        return Ok(());
    }
    let (status, resp) = post_ticket_reqwest(&base, &tok, &req).await?;
    println!("HTTP {status}\n{resp}");
    if !status.is_success() {
        bail!("server replied with {status}");
    }
    Ok(())
}

// ============================================================
// Ratatui TUI mode
//
// The form is a scrollable list of rows grouped into collapsible
// sections. Products are dynamic lines: add as many as needed.
// ============================================================

// Indices into App::fields (fixed order).
/// Base URL text field.
const FIELD_BASE: usize = 0;
/// JWT token text field (masked on screen).
const FIELD_TOKEN: usize = 1;
/// Partner login RUC text field.
const FIELD_LOGIN_RUC: usize = 2;
/// Partner login PSK text field (masked on screen).
const FIELD_LOGIN_SECRET: usize = 3;
/// Ticket partner id text field.
const FIELD_PARTNER: usize = 4;
/// Invoice number (`tickets.ticket_id`) text field.
const FIELD_TICKET_NO: usize = 5;
/// Buyer CI text field.
const FIELD_CI: usize = 6;
/// Buyer first-name text field.
const FIELD_FIRST: usize = 7;
/// Buyer last-name text field.
const FIELD_LAST: usize = 8;
/// Buyer verification-digit text field.
const FIELD_VDIGIT: usize = 9;
/// Buyer phone (SMS OTP channel) text field.
const FIELD_PHONE: usize = 10;
/// Buyer email (OTP channel) text field.
const FIELD_EMAIL: usize = 11;
/// Buyer razon social (juridical receptor) text field.
const FIELD_RAZON: usize = 12;
/// Buyer domicilio text field.
const FIELD_DOMICILIO: usize = 13;
/// Fiscal timbrado (8 digits) text field.
const FIELD_TIMBRADO: usize = 14;
/// Fiscal CDC (44 digits) text field.
const FIELD_CDC: usize = 15;
/// Ticket id to void text field.
const FIELD_DELETE: usize = 16;
/// Void reason text field.
const FIELD_REASON: usize = 17;
/// Number of fixed text fields.
const TEXT_FIELDS: usize = 18;

// Collapsible sections.
/// Connection section: base URL + token.
const SEC_CONN: usize = 0;
/// Partner-login section: RUC + PSK.
const SEC_LOGIN: usize = 1;
/// Invoice section: partner id + buyer data.
const SEC_TICKET: usize = 2;
/// Products section: dynamic `PID:QTY` lines.
const SEC_ITEMS: usize = 3;
/// Void section: ticket id + reason to delete.
const SEC_VOID: usize = 4;
/// Number of collapsible sections.
const SECTION_COUNT: usize = 5;

/// Display names of the collapsible TUI sections, in order.
const SECTION_NAMES: [&str; SECTION_COUNT] = [
    "Conexión",
    "Login partner",
    "Factura (partner + cliente)",
    "Productos (tantos como quieras)",
    "Anular ticket",
];

/// Login button id (see [`Focus::Button`]).
const BTN_LOGIN: usize = 0;
/// Post-ticket button id.
const BTN_POST: usize = 1;
/// Void-ticket button id.
const BTN_DELETE: usize = 2;

/// Height in terminal rows of every form row.
const ROW_H: u16 = 3;

/// Anything the cursor can stop on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    /// A collapsible section header (`SECTION_NAMES` index).
    Section(usize),
    /// A fixed text field (`FIELD_*` index into [`App::fields`]).
    Field(usize),
    /// A dynamic product line (`App::items` index).
    Item(usize),
    /// An action button (`BTN_*` id).
    Button(usize),
}

/// One rendered row of the scrollable form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowKind {
    /// Static title banner (never focusable).
    Title,
    /// Collapsible section header.
    Section(usize),
    /// Fixed text field.
    Field(usize),
    /// Dynamic product line.
    Item(usize),
    /// The login/post/void button row.
    Buttons,
}

/// Interactive TUI state: form contents, collapse flags, cursor and viewport.
struct App {
    /// Fixed text field contents, indexed by `FIELD_*`.
    fields: [String; TEXT_FIELDS],
    /// Product lines, one `PID:QTY` entry per line (separators also split).
    items: Vec<String>,
    /// Per-section fold state, indexed by `SEC_*`.
    collapsed: [bool; SECTION_COUNT],
    /// Index into [`App::focusables`].
    focus: usize,
    /// First visible row of the scrollable viewport.
    scroll: usize,
    /// Last measured form height (rows), for page jumps.
    last_form_h: u16,
    /// Last action result shown in the response pane.
    message: String,
    /// Whether an HTTP round-trip is in flight.
    busy: bool,
}

impl App {
    /// Seeds the form from CLI flags/env, cursor on the partner id field.
    fn new(args: &Args) -> Self {
        let fields = [
            base_url(args),
            token(args),
            args.login_partner.clone().unwrap_or_default(),
            args.login_secret.clone().unwrap_or_default(),
            args.partner.clone().unwrap_or_default(),
            args.ticket_id.clone().unwrap_or_default(),
            args.ci.clone().unwrap_or_default(),
            args.first_name.clone().unwrap_or_default(),
            args.last_name.clone().unwrap_or_default(),
            args.verification_digit.clone().unwrap_or_default(),
            args.phone.clone().unwrap_or_default(),
            args.email.clone().unwrap_or_default(),
            args.razon_social.clone().unwrap_or_default(),
            args.domicilio.clone().unwrap_or_default(),
            args.timbrado.clone().unwrap_or_default(),
            args.cdc.clone().unwrap_or_default(),
            args.delete.clone().unwrap_or_default(),
            args.reason.clone().unwrap_or_default(),
        ];
        let mut app = Self {
            fields,
            items: expand_item_lines(&args.items),
            collapsed: [false; SECTION_COUNT],
            focus: 0,
            scroll: 0,
            last_form_h: 0,
            message: "Tab mueve • Espacio pliega • +/- productos • Ctrl+L login • Ctrl+P post • Ctrl+D anular • Esc salir"
                .to_string(),
            busy: false,
        };
        app.focus = app
            .focusables()
            .iter()
            .position(|f| *f == Focus::Field(FIELD_PARTNER))
            .unwrap_or(0);
        app
    }

    /// Flat focus order, honoring collapsed sections.
    fn focusables(&self) -> Vec<Focus> {
        let mut v = Vec::new();
        for s in 0..SECTION_COUNT {
            v.push(Focus::Section(s));
            if !self.collapsed[s] {
                match s {
                    SEC_CONN => {
                        v.push(Focus::Field(FIELD_BASE));
                        v.push(Focus::Field(FIELD_TOKEN));
                    }
                    SEC_LOGIN => {
                        v.push(Focus::Field(FIELD_LOGIN_RUC));
                        v.push(Focus::Field(FIELD_LOGIN_SECRET));
                    }
                    SEC_TICKET => {
                        v.push(Focus::Field(FIELD_PARTNER));
                        v.push(Focus::Field(FIELD_TICKET_NO));
                        v.push(Focus::Field(FIELD_TIMBRADO));
                        v.push(Focus::Field(FIELD_CDC));
                        v.push(Focus::Field(FIELD_CI));
                        v.push(Focus::Field(FIELD_FIRST));
                        v.push(Focus::Field(FIELD_LAST));
                        v.push(Focus::Field(FIELD_VDIGIT));
                        v.push(Focus::Field(FIELD_RAZON));
                        v.push(Focus::Field(FIELD_DOMICILIO));
                        v.push(Focus::Field(FIELD_PHONE));
                        v.push(Focus::Field(FIELD_EMAIL));
                    }
                    SEC_ITEMS => {
                        for i in 0..self.items.len() {
                            v.push(Focus::Item(i));
                        }
                    }
                    SEC_VOID => {
                        v.push(Focus::Field(FIELD_DELETE));
                        v.push(Focus::Field(FIELD_REASON));
                    }
                    _ => unreachable!("section index out of range"),
                }
            }
        }
        v.push(Focus::Button(BTN_LOGIN));
        v.push(Focus::Button(BTN_POST));
        v.push(Focus::Button(BTN_DELETE));
        v
    }

    /// Visible rows of the scrollable form, in render order.
    fn rows(&self) -> Vec<RowKind> {
        let mut r = vec![RowKind::Title];
        for s in 0..SECTION_COUNT {
            r.push(RowKind::Section(s));
            if !self.collapsed[s] {
                match s {
                    SEC_CONN => {
                        r.push(RowKind::Field(FIELD_BASE));
                        r.push(RowKind::Field(FIELD_TOKEN));
                    }
                    SEC_LOGIN => {
                        r.push(RowKind::Field(FIELD_LOGIN_RUC));
                        r.push(RowKind::Field(FIELD_LOGIN_SECRET));
                    }
                    SEC_TICKET => {
                        r.push(RowKind::Field(FIELD_PARTNER));
                        r.push(RowKind::Field(FIELD_TICKET_NO));
                        r.push(RowKind::Field(FIELD_TIMBRADO));
                        r.push(RowKind::Field(FIELD_CDC));
                        r.push(RowKind::Field(FIELD_CI));
                        r.push(RowKind::Field(FIELD_FIRST));
                        r.push(RowKind::Field(FIELD_LAST));
                        r.push(RowKind::Field(FIELD_VDIGIT));
                        r.push(RowKind::Field(FIELD_RAZON));
                        r.push(RowKind::Field(FIELD_DOMICILIO));
                        r.push(RowKind::Field(FIELD_PHONE));
                        r.push(RowKind::Field(FIELD_EMAIL));
                    }
                    SEC_ITEMS => {
                        for i in 0..self.items.len() {
                            r.push(RowKind::Item(i));
                        }
                    }
                    SEC_VOID => {
                        r.push(RowKind::Field(FIELD_DELETE));
                        r.push(RowKind::Field(FIELD_REASON));
                    }
                    _ => unreachable!("section index out of range"),
                }
            }
        }
        r.push(RowKind::Buttons);
        r
    }

    /// Currently focused stop, clamped into [`App::focusables`].
    fn focus_item(&self) -> Focus {
        let items = self.focusables();
        if items.is_empty() {
            return Focus::Button(BTN_POST);
        }
        items[self.focus.min(items.len() - 1)]
    }

    /// Moves the cursor to the next focus stop, wrapping around.
    fn next(&mut self) {
        let n = self.focusables().len();
        if n > 0 {
            self.focus = (self.focus + 1) % n;
        }
    }

    /// Moves the cursor to the previous focus stop, wrapping around.
    fn prev(&mut self) {
        let n = self.focusables().len();
        if n > 0 {
            self.focus = (self.focus + n - 1) % n;
        }
    }

    /// Jumps the cursor by roughly one viewport page (`dir`: `+1` down, `-1` up).
    fn page(&mut self, dir: i32) {
        let step = ((self.last_form_h / ROW_H) as usize).max(1);
        let n = self.focusables().len();
        if n == 0 {
            return;
        }
        let f = self.focus as i32 + dir * step as i32;
        self.focus = f.clamp(0, n as i32 - 1) as usize;
    }

    /// Whether the cursor sits on a section header (where `Espacio` folds).
    fn is_section_focus(&self) -> bool {
        matches!(self.focus_item(), Focus::Section(_))
    }

    /// Folds/unfolds the focused section, keeping the cursor on its header.
    fn toggle_section(&mut self) {
        if let Focus::Section(s) = self.focus_item() {
            self.collapsed[s] = !self.collapsed[s];
            // Keep the cursor on the toggled header.
            self.focus = self
                .focusables()
                .iter()
                .position(|f| *f == Focus::Section(s))
                .unwrap_or(0);
        }
    }

    /// Product-line index under the cursor, if any.
    fn focused_item(&self) -> Option<usize> {
        match self.focus_item() {
            Focus::Item(i) => Some(i),
            _ => None,
        }
    }

    /// Inserts a blank product line below the focused one and moves to it.
    fn add_item_line(&mut self) {
        if let Some(i) = self.focused_item() {
            self.items.insert(i + 1, String::new());
            self.focus = self
                .focusables()
                .iter()
                .position(|f| *f == Focus::Item(i + 1))
                .unwrap_or(self.focus);
        }
    }

    /// Removes the focused product line (clearing it when it is the last one,
    /// so posting stays possible).
    fn remove_item_line(&mut self) {
        if let Some(i) = self.focused_item() {
            if self.items.len() > 1 {
                self.items.remove(i);
                let j = i.min(self.items.len() - 1);
                self.focus = self
                    .focusables()
                    .iter()
                    .position(|f| *f == Focus::Item(j))
                    .unwrap_or(0);
            } else {
                self.items[0].clear();
            }
        }
    }

    /// One-line header summary per section (token presence, line count).
    /// Never includes secret values.
    fn section_summary(&self, s: usize) -> String {
        match s {
            SEC_CONN => {
                if self.fields[FIELD_TOKEN].is_empty() {
                    "sin token".to_string()
                } else {
                    "JWT guardado".to_string()
                }
            }
            SEC_ITEMS => {
                let n = self.items.len();
                if n == 1 {
                    "1 línea".to_string()
                } else {
                    format!("{n} líneas")
                }
            }
            _ => String::new(),
        }
    }

    /// Posts the form ticket, reporting the outcome in the response pane.
    async fn do_post(&mut self) {
        match self.try_post().await {
            Ok(msg) => self.message = msg,
            Err(e) => self.message = format!("POST failed: {e:#}"),
        }
    }

    /// Logs the partner in, reporting the outcome in the response pane.
    async fn do_login(&mut self) {
        match self.try_login().await {
            Ok(msg) => self.message = msg,
            Err(e) => self.message = format!("LOGIN failed: {e:#}"),
        }
    }

    /// Performs the login round-trip; on success stores the JWT in the token
    /// field and returns a confirmation that never echoes the token itself.
    async fn try_login(&mut self) -> Result<String> {
        let base = self.fields[FIELD_BASE]
            .trim()
            .trim_end_matches('/')
            .to_string();
        let ruc = self.fields[FIELD_LOGIN_RUC].trim().to_string();
        if ruc.is_empty() {
            bail!("login RUC is required");
        }
        let psk = self.fields[FIELD_LOGIN_SECRET].trim().to_string();
        if psk.is_empty() {
            bail!("login PSK is required");
        }
        self.busy = true;
        let out = partner_login_reqwest(&base, &ruc, &psk).await;
        self.busy = false;
        let (status, body) = out?;
        if !status.is_success() {
            return Ok(format!("LOGIN → HTTP {status}\n{body}"));
        }
        // Store the JWT for subsequent posts; never echo the token itself.
        self.fields[FIELD_TOKEN] = extract_token(&body)?;
        Ok(format!("LOGIN → HTTP {status} (token stored)"))
    }

    /// Validates the form and performs the ticket round-trip, returning the
    /// `POST → HTTP <status>` line plus the pretty body for the response pane.
    async fn try_post(&mut self) -> Result<String> {
        let base = self.fields[FIELD_BASE]
            .trim()
            .trim_end_matches('/')
            .to_string();
        let tok = self.fields[FIELD_TOKEN].trim().to_string();
        let contact = |i: usize| {
            let v = self.fields[i].trim().to_string();
            if v.is_empty() { None } else { Some(v) }
        };
        let req = CreateTicketRequest {
            partner_id: parse_id(self.fields[FIELD_PARTNER].trim(), "partner id")?,
            ticket_id: non_empty_str(self.fields[FIELD_TICKET_NO].trim(), "nro. factura")?,
            timbrado: contact(FIELD_TIMBRADO),
            cdc: contact(FIELD_CDC),
            client: build_client(BuyerFlags {
                ci: Some(&self.fields[FIELD_CI]),
                first_name: Some(&self.fields[FIELD_FIRST]),
                last_name: Some(&self.fields[FIELD_LAST]),
                verification_digit: if self.fields[FIELD_VDIGIT].trim().is_empty() {
                    None
                } else {
                    Some(self.fields[FIELD_VDIGIT].as_str())
                },
                razon_social: contact(FIELD_RAZON).as_deref(),
                domicilio: contact(FIELD_DOMICILIO).as_deref(),
                phone: contact(FIELD_PHONE).as_deref(),
                email: contact(FIELD_EMAIL).as_deref(),
            })?,
            details: parse_items(&self.items)?,
        };
        self.busy = true;
        let out = post_ticket_reqwest(&base, &tok, &req).await;
        self.busy = false;
        let (status, body) = out?;
        Ok(format!("POST → HTTP {status}\n{body}"))
    }

    /// Voids the ticket id in the form, reporting the outcome in the response pane.
    async fn do_delete(&mut self) {
        match self.try_delete().await {
            Ok(msg) => self.message = msg,
            Err(e) => self.message = format!("DELETE failed: {e:#}"),
        }
    }

    /// Performs the void round-trip, returning the `DELETE → HTTP <status>`
    /// line plus the pretty body for the response pane.
    async fn try_delete(&mut self) -> Result<String> {
        let base = self.fields[FIELD_BASE]
            .trim()
            .trim_end_matches('/')
            .to_string();
        let tok = self.fields[FIELD_TOKEN].trim().to_string();
        let ticket_id = parse_id(self.fields[FIELD_DELETE].trim(), "ticket id")?;
        let reason = non_empty_str(self.fields[FIELD_REASON].trim(), "motivo")?;
        self.busy = true;
        let out = delete_ticket_reqwest(&base, &tok, ticket_id, &reason).await;
        self.busy = false;
        let (status, body) = out?;
        Ok(format!("DELETE → HTTP {status}\n{body}"))
    }
}

/// Builds a titled input box, highlighted yellow when focused.
fn input_block<'a>(title: &'a str, focused: bool) -> Block<'a> {
    let style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(style)
}

/// On-screen titles of the fixed text fields, indexed by `FIELD_*`.
const FIELD_TITLES: [&str; TEXT_FIELDS] = [
    "Base URL",
    "JWT token (required, filled by login)",
    "Login RUC (partner)",
    "Login PSK (required)",
    "Partner ID",
    "Nro. factura EEE-PPP-NNNNNNN",
    "Client CI (de la factura)",
    "Client first name",
    "Client last name",
    "Verif. digit (optional, mod-11)",
    "Phone (SMS OTP, optional)",
    "Email (OTP, optional)",
    "Razon social (optional)",
    "Domicilio (optional)",
    "Timbrado 8 digitos (optional)",
    "CDC 44 digitos (optional)",
    "Ticket ID (para anular)",
    "Motivo (para anular)",
];

/// Whether a fixed field holds a secret (masked with `•` on screen).
fn is_secret_field(i: usize) -> bool {
    i == FIELD_TOKEN || i == FIELD_LOGIN_SECRET
}

/// Renders one frame: the visible slice of the scrollable form (keeping the
/// focused row in view), the pinned response pane and the help footer.
/// Secrets on screen stay masked; the cursor tracks the focused text.
fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // scrollable form
            Constraint::Min(5),    // response
            Constraint::Length(2), // help
        ])
        .split(area);
    let form_area = outer[0];

    // Which row holds the focused field, if any.
    let focus_row = app
        .rows()
        .iter()
        .position(|r| match (*r, app.focus_item()) {
            (RowKind::Field(i), Focus::Field(j)) => i == j,
            (RowKind::Item(i), Focus::Item(j)) => i == j,
            (RowKind::Section(s), Focus::Section(t)) => s == t,
            (RowKind::Buttons, Focus::Button(_)) => true,
            _ => false,
        });

    // Keep the focused row inside the viewport.
    app.last_form_h = form_area.height;
    let capacity = (form_area.height / ROW_H) as usize;
    if capacity > 0 {
        if let Some(frow) = focus_row {
            if frow < app.scroll {
                app.scroll = frow;
            } else if frow >= app.scroll + capacity {
                app.scroll = frow + 1 - capacity;
            }
        }
        let rows_len = app.rows().len();
        app.scroll = app.scroll.min(rows_len.saturating_sub(capacity));
    }

    // Render the visible slice of the form.
    let rows = app.rows();
    let mut y = form_area.y;
    let end_y = form_area.y + form_area.height;
    for row in rows.iter().skip(app.scroll) {
        if y + ROW_H > end_y {
            break;
        }
        let rect = ratatui::layout::Rect {
            x: form_area.x,
            y,
            width: form_area.width,
            height: ROW_H,
        };
        let focused = match (*row, app.focus_item()) {
            (RowKind::Field(i), Focus::Field(j)) => i == j,
            (RowKind::Item(i), Focus::Item(j)) => i == j,
            (RowKind::Section(s), Focus::Section(t)) => s == t,
            (RowKind::Buttons, Focus::Button(_)) => true,
            _ => false,
        };
        match *row {
            RowKind::Title => {
                frame.render_widget(
                    Paragraph::new("post-ticket — POST /api/v1/partners/tickets (facturas)")
                        .block(Block::default().borders(Borders::ALL)),
                    rect,
                );
            }
            RowKind::Section(s) => {
                let marker = if app.collapsed[s] { "▸" } else { "▾" };
                let summary = app.section_summary(s);
                let head = if summary.is_empty() {
                    format!("{marker} {}  [Espacio]", SECTION_NAMES[s])
                } else {
                    format!("{marker} {} · {summary}  [Espacio]", SECTION_NAMES[s])
                };
                frame.render_widget(Paragraph::new(head).block(input_block("", focused)), rect);
            }
            RowKind::Field(i) => {
                let shown = if is_secret_field(i) && !app.fields[i].is_empty() {
                    "•".repeat(app.fields[i].len().min(48))
                } else {
                    app.fields[i].clone()
                };
                frame.render_widget(
                    Paragraph::new(shown).block(input_block(FIELD_TITLES[i], focused)),
                    rect,
                );
            }
            RowKind::Item(i) => {
                let title = if focused {
                    format!("Producto {} · PID:QTY  (+ añade · - quita)", i + 1)
                } else {
                    format!("Producto {} · PID:QTY", i + 1)
                };
                frame.render_widget(
                    Paragraph::new(app.items[i].clone()).block(input_block(&title, focused)),
                    rect,
                );
            }
            RowKind::Buttons => {
                let btns = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(34),
                        Constraint::Percentage(33),
                        Constraint::Percentage(33),
                    ])
                    .split(rect);
                let style_for = |b: usize| {
                    if matches!(app.focus_item(), Focus::Button(x) if x == b) {
                        Style::default().fg(Color::Black).bg(Color::Yellow)
                    } else {
                        Style::default()
                    }
                };
                frame.render_widget(
                    Paragraph::new("[ Login ]")
                        .style(style_for(BTN_LOGIN))
                        .block(Block::default().borders(Borders::ALL)),
                    btns[0],
                );
                frame.render_widget(
                    Paragraph::new("[ Post ticket ]")
                        .style(style_for(BTN_POST))
                        .block(Block::default().borders(Borders::ALL)),
                    btns[1],
                );
                frame.render_widget(
                    Paragraph::new("[ Void ticket ]")
                        .style(style_for(BTN_DELETE))
                        .block(Block::default().borders(Borders::ALL)),
                    btns[2],
                );
            }
        }
        y += ROW_H;
    }
    if capacity == 0 {
        app.scroll = 0;
    }

    // Response + help stay pinned below the viewport.
    let msg = if app.busy {
        "sending…".to_string()
    } else {
        app.message.clone()
    };
    frame.render_widget(
        Paragraph::new(msg)
            .block(Block::default().borders(Borders::ALL).title("Respuesta"))
            .wrap(Wrap { trim: false }),
        outer[1],
    );
    frame.render_widget(
        Paragraph::new(
            "Tab/↑↓ moverse · Espacio pliega · +/- productos · RePág/AvPág desplaza · Ctrl+L/P/D login/post/anular · Esc salir",
        )
        .wrap(Wrap { trim: true }),
        outer[2],
    );

    // Cursor on the focused, visible text field.
    if let Some(frow) = focus_row
        && frow >= app.scroll && frow < app.scroll + capacity.max(1) {
            let len = match app.focus_item() {
                Focus::Field(i) => app.fields[i].len(),
                Focus::Item(i) => app.items.get(i).map(String::len).unwrap_or(0),
                _ => return,
            };
            let row_y = form_area.y + ((frow - app.scroll) as u16) * ROW_H + 1;
            let x = form_area.x + 1 + len as u16;
            frame.set_cursor_position((
                x.min(form_area.x + form_area.width.saturating_sub(2)),
                row_y,
            ));
        }
}

/// Appends a typed character to the focused text field or product line.
fn on_char(app: &mut App, c: char) {
    match app.focus_item() {
        Focus::Field(i) => app.fields[i].push(c),
        Focus::Item(i) => {
            if let Some(line) = app.items.get_mut(i) {
                line.push(c);
            }
        }
        _ => {}
    }
}

/// Deletes the last character of the focused text field or product line.
fn on_backspace(app: &mut App) {
    match app.focus_item() {
        Focus::Field(i) => {
            app.fields[i].pop();
        }
        Focus::Item(i) => {
            if let Some(line) = app.items.get_mut(i) {
                line.pop();
            }
        }
        _ => {}
    }
}

/// Runs the fullscreen TUI event loop until `Esc`/`Ctrl+C`, then prints the
/// last response line to stdout for scripting.
async fn run_tui(args: &Args) -> Result<()> {
    crossterm::terminal::enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)
        .context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(backend).context("failed to init terminal")?;

    let mut app = App::new(args);
    let outcome: Result<()> = loop {
        terminal.draw(|f| draw(f, &mut app)).ok();
        // Blocking read goes on the blocking pool so we don't stall workers.
        let ev = tokio::task::spawn_blocking(crossterm::event::read)
            .await
            .context("event thread failed")?
            .map_err(|e| anyhow::anyhow!("event read failed: {e}"))?;
        if let Event::Key(key) = ev { match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => break Ok(()),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => break Ok(()),
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => app.do_login().await,
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => app.do_post().await,
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => app.do_delete().await,
            (KeyCode::Tab, _) => app.next(),
            (KeyCode::BackTab, _) => app.prev(),
            (KeyCode::Down, _) => app.next(),
            (KeyCode::Up, _) => app.prev(),
            (KeyCode::PageDown, _) => app.page(1),
            (KeyCode::PageUp, _) => app.page(-1),
            (KeyCode::Enter, _) => match app.focus_item() {
                Focus::Button(BTN_LOGIN) => app.do_login().await,
                Focus::Button(BTN_POST) => app.do_post().await,
                Focus::Button(BTN_DELETE) => app.do_delete().await,
                Focus::Section(_) => app.toggle_section(),
                _ => app.next(),
            },
            // Space on a section header folds/unfolds it, elsewhere it types.
            (KeyCode::Char(' '), _) if app.is_section_focus() => app.toggle_section(),
            // On a product line, +/- manage lines instead of typing.
            (KeyCode::Char('+'), m)
                if (m.is_empty() || m == KeyModifiers::SHIFT)
                    && app.focused_item().is_some() =>
            {
                app.add_item_line();
            }
            (KeyCode::Char('-'), m)
                if (m.is_empty() || m == KeyModifiers::SHIFT)
                    && app.focused_item().is_some() =>
            {
                app.remove_item_line();
            }
            (KeyCode::Delete, _) if app.focused_item().is_some() => {
                app.remove_item_line();
            }
            (KeyCode::Backspace, _) => {
                on_backspace(&mut app);
            }
            (KeyCode::Left, _) | (KeyCode::Right, _) => {}
            (KeyCode::Char(c), m) if m.is_empty() || m == KeyModifiers::SHIFT => {
                on_char(&mut app, c);
            }
            _ => {}
        } }
    };

    crossterm::terminal::disable_raw_mode().ok();
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )
    .ok();
    terminal.show_cursor().ok();
    outcome?;
    println!("{}", app.message);
    Ok(())
}

/// Entry point: `--help` prints [`HELP`]; action flags select the one-shot
/// CLI, otherwise (or with `--tui`) the interactive form runs.
#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    if args.help {
        print!("{HELP}");
        return Ok(());
    }
    let oneshot = args.delete.is_some()
        || args.partner.is_some()
        || args.ci.is_some()
        || args.first_name.is_some()
        || args.last_name.is_some()
        || args.login_partner.is_some()
        || !args.items.is_empty()
        || args.dry_run;
    if args.tui || !oneshot {
        run_tui(&args).await
    } else {
        run_oneshot(&args).await
    }
}

#[cfg(test)]
/// Unit tests for the TUI form model (focus order, folding, product lines).
mod tests {
    use super::*;

    fn empty_args() -> Args {
        Args::default()
    }

    #[test]
    fn item_lines_expand_and_always_exist() {
        assert_eq!(expand_item_lines(&[]), vec![String::new()]);
        assert_eq!(
            expand_item_lines(&["1:2, 3:1".to_string(), "2:5".to_string()]),
            vec!["1:2".to_string(), "3:1".to_string(), "2:5".to_string()]
        );
    }

    #[test]
    fn collapse_hides_section_fields_from_focus_and_rows() {
        let mut app = App::new(&empty_args());
        let full_focus = app.focusables().len();
        let full_rows = app.rows().len();
        assert!(full_focus > 5 && full_rows > 5);

        // Collapse the ticket section: its 12 fields vanish from focus/rows.
        app.focus = app
            .focusables()
            .iter()
            .position(|f| *f == Focus::Section(SEC_TICKET))
            .unwrap();
        app.toggle_section();
        assert!(app.collapsed[SEC_TICKET]);
        assert_eq!(app.focusables().len(), full_focus - 12);
        assert_eq!(app.rows().len(), full_rows - 12);
        assert_eq!(app.focus_item(), Focus::Section(SEC_TICKET));

        app.toggle_section();
        assert_eq!(app.focusables().len(), full_focus);
        assert_eq!(app.rows().len(), full_rows);
    }

    #[test]
    fn product_lines_grow_and_shrink_without_limit() {
        let mut app = App::new(&empty_args());
        for _ in 0..50 {
            app.focus = app
                .focusables()
                .iter()
                .position(|f| *f == Focus::Item(app.items.len() - 1))
                .unwrap();
            app.add_item_line();
        }
        assert_eq!(app.items.len(), 51);
        assert_eq!(
            app.focusables()
                .iter()
                .filter(|f| matches!(f, Focus::Item(_)))
                .count(),
            51
        );

        for _ in 0..50 {
            app.focus = app
                .focusables()
                .iter()
                .position(|f| matches!(f, Focus::Item(_)))
                .unwrap();
            app.remove_item_line();
        }
        // One blank line always remains so posting stays possible.
        assert_eq!(app.items, vec![String::new()]);
    }

    #[test]
    fn focus_wraps_around() {
        let mut app = App::new(&empty_args());
        let n = app.focusables().len();
        app.focus = n - 1;
        app.next();
        assert_eq!(app.focus, 0);
        app.prev();
        assert_eq!(app.focus, n - 1);
    }
}
