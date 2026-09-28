//! post-ticket — post tickets (facturas) to the lab API from the terminal.
//!
//! Two modes:
//! * One-shot CLI (parity with `lab/scripts/post-ticket.sh`):
//!   `post-ticket --partner 1 --ci 1234567 --first-name María --last-name González --item 1:2`
//! * Interactive TUI (default when no action flags are given, or `--tui`).
//!
//! HTTP is done with [`reqwest`], the interface with [`ratatui`].

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

#[derive(Debug, Clone, Serialize)]
struct TicketDetail {
    product_id: i32,
    quantity: i32,
}

#[derive(Debug, Clone, Serialize)]
struct TicketClient {
    ci: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification_digit: Option<i32>,
    first_name: String,
    last_name: String,
}

#[derive(Debug, Clone, Serialize)]
struct CreateTicketRequest {
    partner_id: i32,
    client: TicketClient,
    details: Vec<TicketDetail>,
}

#[derive(Debug, Clone, Serialize)]
struct DeleteTicketRequest {
    ticket_id: i32,
}

/// Mirrors `routes::api::dto::PartnerLoginRequest`:
/// partners authenticate with their RUC (natural key); `secret` is an
/// optional pre-shared credential.
#[derive(Debug, Clone, Serialize)]
struct PartnerLoginRequest {
    ruc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LoginReply {
    token: String,
}

// ============================================================
// CLI args (manual parsing, no extra deps)
// ============================================================

#[derive(Debug, Default)]
struct Args {
    tui: bool,
    partner: Option<String>,
    ci: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    verification_digit: Option<String>,
    items: Vec<String>,
    base_url: Option<String>,
    token: Option<String>,
    login_partner: Option<String>,
    login_secret: Option<String>,
    delete: Option<String>,
    dry_run: bool,
    help: bool,
}

const HELP: &str = r#"post-ticket — post tickets to the lab API from the terminal.

Usage (one-shot):
  post-ticket --partner ID --ci CI --first-name NAME --last-name NAME --item PID:QTY [...] [options]
  post-ticket --login-partner RUC [--login-secret SECRET]   # partner login only
  post-ticket --login-partner RUC --partner ID --ci CI ...  # login, then act as partner
  post-ticket --delete TICKET_ID [options]

Usage (interactive TUI):
  post-ticket [--tui] [--base-url URL] [--token TOKEN]

Options:
  --partner ID     Partner (farmacia) id. Required unless --delete/--login-partner alone.
  --ci CI          Buyer CI from the factura (client is registered if new).
                   Required unless --delete/--login-partner alone.
  --first-name N   Buyer first name as printed on the factura. Required unless --delete/--login-partner alone.
  --last-name N    Buyer last name as printed on the factura. Required unless --delete/--login-partner alone.
  --verification-digit D
                   Optional <CI>-<D> RUC suffix digit (0-9).
  --login-partner RUC
                   Log the partner in (POST /api/v1/partners/login) and print
                   the JWT. Combined with --partner/--delete it authenticates
                   that same invocation instead of --token.
  --login-secret S Optional pre-shared partner credential for the login.
  --item PID:QTY   One ticket line; repeatable. At least one required.
                   Example: --item 1:2 --item 3:1
  --base-url URL   API base URL. Default: $LAB_BASE_URL or http://127.0.0.1:8080
  --token TOKEN    Bearer JWT. Default: $LAB_JWT_TOKEN or empty (API is open).
  --delete ID      Void ticket ID instead of posting (DELETE /partners/tickets).
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
  Partners: 1 Farmacia Central (RUC 80012345-1), 2 Farmacia del Sur (RUC 80067890-2)
  Clients:  1 María González (CI 1234567), 2 Juan Pérez (CI 2345678), 3 Ana López
  Products: 1 Paracetamol (10 pts), 2 Ibuprofeno (8 pts), 3 Vitamina C (5 pts),
            4 Crema (3 pts), 5 Termo (canje 100 pts), 6 Mochila (canje 250 pts)
"#;

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

fn base_url(args: &Args) -> String {
    args.base_url
        .clone()
        .or_else(|| std::env::var("LAB_BASE_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn token(args: &Args) -> String {
    args.token
        .clone()
        .or_else(|| std::env::var("LAB_JWT_TOKEN").ok())
        .unwrap_or_default()
}

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

fn build_client(
    ci: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
    verification_digit: Option<&str>,
) -> Result<TicketClient> {
    Ok(TicketClient {
        ci: parse_ci(ci.context("--ci is required")?)?,
        verification_digit: verification_digit.map(parse_vdigit).transpose()?,
        first_name: non_empty(first_name, "--first-name")?,
        last_name: non_empty(last_name, "--last-name")?,
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

fn pretty_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string()))
        .unwrap_or_else(|_| raw.to_string())
}

// ============================================================
// HTTP via reqwest
// ============================================================

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .build()
        .context("failed to build HTTP client")
}

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

async fn delete_ticket_reqwest(
    base: &str,
    token: &str,
    ticket_id: i32,
) -> Result<(reqwest::StatusCode, String)> {
    let mut call = http_client()?
        .delete(format!("{base}/api/v1/partners/tickets"))
        .json(&DeleteTicketRequest { ticket_id });
    if !token.is_empty() {
        call = call.bearer_auth(token);
    }
    let res = call.send().await.context("request failed")?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    Ok((status, pretty_json(&body)))
}

async fn partner_login_reqwest(
    base: &str,
    ruc: &str,
    secret: Option<&str>,
) -> Result<(reqwest::StatusCode, String)> {
    let req = PartnerLoginRequest {
        ruc: ruc.to_string(),
        secret: secret
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
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

async fn run_oneshot(args: &Args) -> Result<()> {
    let base = base_url(args);
    let mut tok = token(args);

    // Optional partner login first: `--login-partner RUC` alone prints the
    // JWT; combined with --partner/--delete it authenticates this same
    // invocation (overriding --token).
    if let Some(ruc) = &args.login_partner {
        if args.dry_run {
            println!(
                "{}",
                serde_json::to_string_pretty(&PartnerLoginRequest {
                    ruc: ruc.clone(),
                    secret: args.login_secret.clone(),
                })?
            );
            return Ok(());
        }
        let (status, resp) =
            partner_login_reqwest(&base, ruc, args.login_secret.as_deref()).await?;
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
        let body = serde_json::to_string_pretty(&DeleteTicketRequest { ticket_id })?;
        if args.dry_run {
            println!("{body}");
            return Ok(());
        }
        let (status, resp) = delete_ticket_reqwest(&base, &tok, ticket_id).await?;
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
    let client = build_client(
        args.ci.as_deref(),
        args.first_name.as_deref(),
        args.last_name.as_deref(),
        args.verification_digit.as_deref(),
    )?;
    let details = parse_items(&args.items)?;
    let req = CreateTicketRequest {
        partner_id,
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
const FIELD_BASE: usize = 0;
const FIELD_TOKEN: usize = 1;
const FIELD_LOGIN_RUC: usize = 2;
const FIELD_LOGIN_SECRET: usize = 3;
const FIELD_PARTNER: usize = 4;
const FIELD_CI: usize = 5;
const FIELD_FIRST: usize = 6;
const FIELD_LAST: usize = 7;
const FIELD_VDIGIT: usize = 8;
const FIELD_DELETE: usize = 9;
const TEXT_FIELDS: usize = 10;

// Collapsible sections.
const SECTION_COUNT: usize = 5;
const SEC_CONN: usize = 0;
const SEC_LOGIN: usize = 1;
const SEC_TICKET: usize = 2;
const SEC_ITEMS: usize = 3;
const SEC_VOID: usize = 4;

const SECTION_NAMES: [&str; SECTION_COUNT] = [
    "Conexión",
    "Login partner",
    "Factura (partner + cliente)",
    "Productos (tantos como quieras)",
    "Anular ticket",
];

const BTN_LOGIN: usize = 0;
const BTN_POST: usize = 1;
const BTN_DELETE: usize = 2;

const ROW_H: u16 = 3;

/// Anything the cursor can stop on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Section(usize),
    Field(usize),
    Item(usize),
    Button(usize),
}

/// One rendered row of the scrollable form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowKind {
    Title,
    Section(usize),
    Field(usize),
    Item(usize),
    Buttons,
}

struct App {
    fields: [String; TEXT_FIELDS],
    /// Product lines, one `PID:QTY` entry per line (separators also split).
    items: Vec<String>,
    collapsed: [bool; SECTION_COUNT],
    /// Index into [`App::focusables`].
    focus: usize,
    /// First visible row of the scrollable viewport.
    scroll: usize,
    /// Last measured form height (rows), for page jumps.
    last_form_h: u16,
    message: String,
    busy: bool,
}

impl App {
    fn new(args: &Args) -> Self {
        let fields = [
            base_url(args),
            token(args),
            args.login_partner.clone().unwrap_or_default(),
            args.login_secret.clone().unwrap_or_default(),
            args.partner.clone().unwrap_or_default(),
            args.ci.clone().unwrap_or_default(),
            args.first_name.clone().unwrap_or_default(),
            args.last_name.clone().unwrap_or_default(),
            args.verification_digit.clone().unwrap_or_default(),
            args.delete.clone().unwrap_or_default(),
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
                        v.push(Focus::Field(FIELD_CI));
                        v.push(Focus::Field(FIELD_FIRST));
                        v.push(Focus::Field(FIELD_LAST));
                        v.push(Focus::Field(FIELD_VDIGIT));
                    }
                    SEC_ITEMS => {
                        for i in 0..self.items.len() {
                            v.push(Focus::Item(i));
                        }
                    }
                    SEC_VOID => v.push(Focus::Field(FIELD_DELETE)),
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
                        r.push(RowKind::Field(FIELD_CI));
                        r.push(RowKind::Field(FIELD_FIRST));
                        r.push(RowKind::Field(FIELD_LAST));
                        r.push(RowKind::Field(FIELD_VDIGIT));
                    }
                    SEC_ITEMS => {
                        for i in 0..self.items.len() {
                            r.push(RowKind::Item(i));
                        }
                    }
                    SEC_VOID => r.push(RowKind::Field(FIELD_DELETE)),
                    _ => unreachable!("section index out of range"),
                }
            }
        }
        r.push(RowKind::Buttons);
        r
    }

    fn focus_item(&self) -> Focus {
        let items = self.focusables();
        if items.is_empty() {
            return Focus::Button(BTN_POST);
        }
        items[self.focus.min(items.len() - 1)]
    }

    fn next(&mut self) {
        let n = self.focusables().len();
        if n > 0 {
            self.focus = (self.focus + 1) % n;
        }
    }

    fn prev(&mut self) {
        let n = self.focusables().len();
        if n > 0 {
            self.focus = (self.focus + n - 1) % n;
        }
    }

    fn page(&mut self, dir: i32) {
        let step = ((self.last_form_h / ROW_H) as usize).max(1);
        let n = self.focusables().len();
        if n == 0 {
            return;
        }
        let f = self.focus as i32 + dir * step as i32;
        self.focus = f.clamp(0, n as i32 - 1) as usize;
    }

    fn is_section_focus(&self) -> bool {
        matches!(self.focus_item(), Focus::Section(_))
    }

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

    fn focused_item(&self) -> Option<usize> {
        match self.focus_item() {
            Focus::Item(i) => Some(i),
            _ => None,
        }
    }

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

    async fn do_post(&mut self) {
        match self.try_post().await {
            Ok(msg) => self.message = msg,
            Err(e) => self.message = format!("POST failed: {e:#}"),
        }
    }

    async fn do_login(&mut self) {
        match self.try_login().await {
            Ok(msg) => self.message = msg,
            Err(e) => self.message = format!("LOGIN failed: {e:#}"),
        }
    }

    async fn try_login(&mut self) -> Result<String> {
        let base = self.fields[FIELD_BASE]
            .trim()
            .trim_end_matches('/')
            .to_string();
        let ruc = self.fields[FIELD_LOGIN_RUC].trim().to_string();
        if ruc.is_empty() {
            bail!("login RUC is required");
        }
        let secret = self.fields[FIELD_LOGIN_SECRET].trim().to_string();
        self.busy = true;
        let out = partner_login_reqwest(
            &base,
            &ruc,
            if secret.is_empty() {
                None
            } else {
                Some(secret.as_str())
            },
        )
        .await;
        self.busy = false;
        let (status, body) = out?;
        if !status.is_success() {
            return Ok(format!("LOGIN → HTTP {status}\n{body}"));
        }
        // Store the JWT for subsequent posts; never echo the token itself.
        self.fields[FIELD_TOKEN] = extract_token(&body)?;
        Ok(format!("LOGIN → HTTP {status} (token stored)"))
    }

    async fn try_post(&mut self) -> Result<String> {
        let base = self.fields[FIELD_BASE]
            .trim()
            .trim_end_matches('/')
            .to_string();
        let tok = self.fields[FIELD_TOKEN].trim().to_string();
        let req = CreateTicketRequest {
            partner_id: parse_id(self.fields[FIELD_PARTNER].trim(), "partner id")?,
            client: build_client(
                Some(&self.fields[FIELD_CI]),
                Some(&self.fields[FIELD_FIRST]),
                Some(&self.fields[FIELD_LAST]),
                if self.fields[FIELD_VDIGIT].trim().is_empty() {
                    None
                } else {
                    Some(self.fields[FIELD_VDIGIT].as_str())
                },
            )?,
            details: parse_items(&self.items)?,
        };
        self.busy = true;
        let out = post_ticket_reqwest(&base, &tok, &req).await;
        self.busy = false;
        let (status, body) = out?;
        Ok(format!("POST → HTTP {status}\n{body}"))
    }

    async fn do_delete(&mut self) {
        match self.try_delete().await {
            Ok(msg) => self.message = msg,
            Err(e) => self.message = format!("DELETE failed: {e:#}"),
        }
    }

    async fn try_delete(&mut self) -> Result<String> {
        let base = self.fields[FIELD_BASE]
            .trim()
            .trim_end_matches('/')
            .to_string();
        let tok = self.fields[FIELD_TOKEN].trim().to_string();
        let ticket_id = parse_id(self.fields[FIELD_DELETE].trim(), "ticket id")?;
        self.busy = true;
        let out = delete_ticket_reqwest(&base, &tok, ticket_id).await;
        self.busy = false;
        let (status, body) = out?;
        Ok(format!("DELETE → HTTP {status}\n{body}"))
    }
}

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

const FIELD_TITLES: [&str; TEXT_FIELDS] = [
    "Base URL",
    "JWT token (optional, filled by login)",
    "Login RUC (partner)",
    "Login secret (optional)",
    "Partner ID",
    "Client CI (de la factura)",
    "Client first name",
    "Client last name",
    "Verif. digit (optional)",
    "Ticket ID (para anular)",
];

fn is_secret_field(i: usize) -> bool {
    i == FIELD_TOKEN || i == FIELD_LOGIN_SECRET
}

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
    if let Some(frow) = focus_row {
        if frow >= app.scroll && frow < app.scroll + capacity.max(1) {
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
}

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
        match ev {
            Event::Key(key) => match (key.code, key.modifiers) {
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
            },
            _ => {}
        }
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

        // Collapse the ticket section: its 5 fields vanish from focus/rows.
        app.focus = app
            .focusables()
            .iter()
            .position(|f| *f == Focus::Section(SEC_TICKET))
            .unwrap();
        app.toggle_section();
        assert!(app.collapsed[SEC_TICKET]);
        assert_eq!(app.focusables().len(), full_focus - 5);
        assert_eq!(app.rows().len(), full_rows - 5);
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
