//! # Client one-time passwords (OTP).
//!
//! Second-factor login for end customers ([`clients`](crate::models::Clients)):
//! the client asks for a code, receives a 6-digit number over his SMS or
//! email channel, and exchanges it for a JWT.
//!
//! The codes are TOTP-style and **stateless**: `code = blake3(secret || ci ||
//! window) mod 1_000_000` with 5-minute windows, so no OTP table is needed
//! (the schema has none) and any instance sharing the secret verifies. The
//! current and previous windows are accepted to tolerate clock skew. Only
//! short-lived rate-limit state lives in memory ([`OtpGuard`]): resend
//! throttling and brute-force lockout.
//!
//! Delivery is pluggable via [`send_otp`]. The default `log` sender only
//! writes the masked destination (plus the code, so the dev loop works
//! without a provider); production must configure a real SMS/email sender.
//! Codes are hashed with blake3 before comparison so they never sit in
//! memory as plain strings longer than needed.
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};

use crate::models::Clients;

/// Seconds per OTP validity window (5 minutes).
pub const OTP_STEP_SECS: u64 = 300;
/// Seconds a freshly sent code stays usable (two windows: current + previous).
pub const OTP_TTL_SECS: u64 = 2 * OTP_STEP_SECS;
/// Max code sends per client per window (resend throttling).
pub const OTP_MAX_SENDS_PER_WINDOW: u32 = 3;
/// Failed verifications before the client is locked out.
pub const OTP_MAX_ATTEMPTS: u32 = 5;
/// Lockout duration after too many failed verifications (15 minutes).
pub const OTP_LOCKOUT_SECS: i64 = 900;

/// Delivery channel for an OTP code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtpChannel {
    /// SMS to the client's `phone_number`.
    Sms,
    /// Email to the client's `email`.
    Email,
}

impl OtpChannel {
    /// Parses the `channel` field of an OTP request (`"sms"` / `"email"`).
    ///
    /// # Errors
    ///
    /// Returns [`OtpError::UnknownChannel`] for anything else.
    pub fn parse(raw: &str) -> Result<Self, OtpError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "sms" => Ok(Self::Sms),
            "email" => Ok(Self::Email),
            _ => Err(OtpError::UnknownChannel),
        }
    }

    /// Name used in logs and API responses.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sms => "sms",
            Self::Email => "email",
        }
    }
}

/// Failures of the OTP flow, mapped to HTTP statuses by the API layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtpError {
    /// Requested channel string is not `"sms"`/`"email"`.
    UnknownChannel,
    /// The client has no destination for the requested channel.
    ChannelUnavailable {
        /// Which channel was requested.
        channel: OtpChannel,
    },
    /// Too many codes requested in the current window.
    TooManyRequests,
    /// Locked out after too many wrong codes; retry after the instant.
    Locked {
        /// When verification may be attempted again.
        retry_after: DateTime<Utc>,
    },
}

/// Picks the delivery channel and destination for a client.
///
/// An explicitly requested channel wins when the client has a destination
/// for it; otherwise email is preferred (cheaper and more reliable than
/// SMS), falling back to SMS. The schema guarantees at least one contact
/// exists.
///
/// # Errors
///
/// Returns [`OtpError::UnknownChannel`] for a bad `requested` value and
/// [`OtpError::ChannelUnavailable`] when the requested channel has no
/// destination on the client.
pub fn channel_for(
    client: &Clients,
    requested: Option<OtpChannel>,
) -> Result<(OtpChannel, String), OtpError> {
    if let Some(channel) = requested {
        let dest = match channel {
            OtpChannel::Sms => client.phone_number.clone(),
            OtpChannel::Email => client.email.clone(),
        };
        return dest
            .map(|d| (channel, d))
            .ok_or(OtpError::ChannelUnavailable { channel });
    }
    if let Some(email) = client.email.clone() {
        return Ok((OtpChannel::Email, email));
    }
    if let Some(phone) = client.phone_number.clone() {
        return Ok((OtpChannel::Sms, phone));
    }
    // Unreachable under the schema CHECK, but email-first keeps the error sane.
    Err(OtpError::ChannelUnavailable {
        channel: OtpChannel::Email,
    })
}

/// Server secret used to derive OTP codes.
///
/// `LAB_OTP_SECRET` wins; otherwise `LAB_JWT_SECRET` is reused so a dev
/// deploy works with a single secret.
///
/// # Errors
///
/// Fails when neither variable is set.
pub fn otp_secret() -> anyhow::Result<Vec<u8>> {
    if let Ok(s) = std::env::var("LAB_OTP_SECRET") {
        if !s.is_empty() {
            return Ok(s.into_bytes());
        }
    }
    match std::env::var("LAB_JWT_SECRET") {
        Ok(s) if !s.is_empty() => Ok(s.into_bytes()),
        _ => anyhow::bail!("LAB_OTP_SECRET (or LAB_JWT_SECRET) must be set"),
    }
}

/// Current 5-minute window number since the Unix epoch.
///
/// Public so the API layer can derive the code being dispatched; only
/// [`generate_code`] and [`verify_code`] interpret it.
pub fn current_window() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / OTP_STEP_SECS)
        .unwrap_or(0)
}

/// Derives the 6-digit code for a client in a given window.
///
/// The 32-byte blake3 digest of the secret becomes the hash key; the message
/// binds the client (`ci`) and the window, so codes differ per client and
/// rotate every [`OTP_STEP_SECS`] seconds.
pub fn generate_code(secret: &[u8], ci: i32, window: u64) -> String {
    let key = blake3::hash(secret);
    let mut hasher = blake3::Hasher::new_keyed(key.as_bytes());
    hasher.update(&ci.to_le_bytes());
    hasher.update(&window.to_le_bytes());
    let digest = hasher.finalize();
    let n = u32::from_le_bytes(digest.as_bytes()[..4].try_into().unwrap_or([0; 4])) % 1_000_000;
    format!("{n:06}")
}

/// Checks `code` against the current and previous windows.
///
/// Comparing blake3 hashes (not the plain codes) keeps the secret material
/// out of timing-attack-friendly string compares.
///
/// # Errors
///
/// Fails when the OTP secret cannot be resolved.
pub fn verify_code(secret: &[u8], ci: i32, code: &str) -> anyhow::Result<bool> {
    let probe = blake3::hash(code.trim().as_bytes());
    let window = current_window();
    for w in window.saturating_sub(1)..=window {
        let expected = generate_code(secret, ci, w);
        if blake3::hash(expected.as_bytes()) == probe {
            return Ok(true);
        }
    }
    Ok(false)
}

/// In-memory resend throttling + brute-force lockout per client CI.
///
/// Short-lived operational state only (the codes themselves are stateless);
/// a restart simply resets counters, which is safe because codes expire
/// within minutes anyway.
#[derive(Debug, Default)]
pub struct OtpGuard {
    /// Rate-limit state by client CI.
    inner: Mutex<HashMap<i32, GuardEntry>>,
}

/// Counters for one client CI.
#[derive(Debug, Clone)]
struct GuardEntry {
    /// Window the `sends` count belongs to.
    window: u64,
    /// Codes sent in `window`.
    sends: u32,
    /// Consecutive failed verifications.
    failures: u32,
    /// Locked until this instant (after too many failures).
    locked_until: Option<DateTime<Utc>>,
}

impl OtpGuard {
    /// Creates an empty guard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a code send; rejects throttled or locked-out clients.
    ///
    /// # Errors
    ///
    /// Returns [`OtpError::Locked`] while a lockout holds and
    /// [`OtpError::TooManyRequests`] past [`OTP_MAX_SENDS_PER_WINDOW`].
    pub fn check_and_record_send(&self, ci: i32) -> Result<(), OtpError> {
        let now = Utc::now();
        let window = current_window();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.entry(ci).or_insert(GuardEntry {
            window,
            sends: 0,
            failures: 0,
            locked_until: None,
        });
        if let Some(until) = entry.locked_until {
            if now < until {
                return Err(OtpError::Locked { retry_after: until });
            }
            entry.locked_until = None;
            entry.failures = 0;
        }
        if entry.window != window {
            entry.window = window;
            entry.sends = 0;
        }
        if entry.sends >= OTP_MAX_SENDS_PER_WINDOW {
            return Err(OtpError::TooManyRequests);
        }
        entry.sends += 1;
        Ok(())
    }

    /// Rejects verification attempts from locked-out clients.
    ///
    /// # Errors
    ///
    /// Returns [`OtpError::Locked`] while a lockout holds.
    pub fn check_not_locked(&self, ci: i32) -> Result<(), OtpError> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = guard.get_mut(&ci) {
            if let Some(until) = entry.locked_until {
                if Utc::now() < until {
                    return Err(OtpError::Locked { retry_after: until });
                }
                entry.locked_until = None;
                entry.failures = 0;
            }
        }
        Ok(())
    }

    /// Records a verification outcome: success clears failures, failure
    /// counts up to a [`OTP_LOCKOUT_SECS`] lockout.
    pub fn record_result(&self, ci: i32, ok: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if ok {
            guard.remove(&ci);
            return;
        }
        let entry = guard.entry(ci).or_insert(GuardEntry {
            window: current_window(),
            sends: 0,
            failures: 0,
            locked_until: None,
        });
        entry.failures += 1;
        if entry.failures >= OTP_MAX_ATTEMPTS {
            entry.locked_until = Some(Utc::now() + chrono::Duration::seconds(OTP_LOCKOUT_SECS));
        }
    }
}

/// Masks a phone number for logs/responses (`+59598112••••`).
pub fn mask_phone(phone: &str) -> String {
    let visible = phone.chars().count().min(8);
    let kept: String = phone.chars().take(visible).collect();
    format!("{kept}••••")
}

/// Masks an email for logs/responses (`m•••@example.com`).
pub fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((user, domain)) => {
            let first = user.chars().next().unwrap_or('•');
            format!("{first}•••@{domain}")
        }
        None => "•••".to_string(),
    }
}

/// Dispatches an OTP code over the client's channel.
///
/// `LAB_OTP_SENDER` selects the backend (default `"log"`):
///
/// * `"log"` — records the masked destination and — dev only — the code
///   itself, so the login loop works with no provider configured.
/// * `"smtp"` — sends a real email through Gmail's SMTP relay via
///   [`lettre`], authenticated with `EMAIL_FROM` (the Gmail address) plus
///   the `EMAIL_APP_PASSWORD` secret (a Gmail app password, never the
///   account password). The blocking SMTP round-trip runs on Tokio's
///   blocking pool. Production senders MUST NOT log codes.
///
/// SMS delivery has no provider wired yet: requesting the SMS channel with
/// `LAB_OTP_SENDER=smtp` fails with a clear error (use `"log"` while no SMS
/// provider is configured).
///
/// # Errors
///
/// Fails for an unknown `LAB_OTP_SENDER` value, for missing `EMAIL_FROM` /
/// `EMAIL_APP_PASSWORD` in SMTP mode, for unparseable addresses, and for
/// SMTP transport failures.
pub async fn send_otp(channel: OtpChannel, destination: &str, code: &str) -> anyhow::Result<()> {
    let mode = std::env::var("LAB_OTP_SENDER").unwrap_or_else(|_| "log".to_string());
    match mode.trim().to_ascii_lowercase().as_str() {
        "log" => {
            let masked = match channel {
                OtpChannel::Sms => mask_phone(destination),
                OtpChannel::Email => mask_email(destination),
            };
            // DEV ONLY: the code is logged so the OTP loop works without an
            // SMS/email provider. Production senders must not log codes.
            tracing::info!(
                channel = channel.name(),
                destination = masked.as_str(),
                code = code,
                "otp dispatched (log sender; dev only)"
            );
            Ok(())
        }
        "smtp" => {
            let masked = match channel {
                OtpChannel::Sms => mask_phone(destination),
                OtpChannel::Email => mask_email(destination),
            };
            // DEV ONLY: the code is logged so the OTP loop works without an
            // SMS/email provider. Production senders must not log codes.
            tracing::info!(
                channel = channel.name(),
                destination = masked.as_str(),
                code = code,
                "otp dispatched (log sender; dev only)"
            );
            if channel != OtpChannel::Email {
                anyhow::bail!("smtp sender only delivers email; {masked} needs an SMS provider");
            }
            let from: String = match std::env::var("EMAIL_FROM") {
                Ok(s) if !s.trim().is_empty() => s,
                _ => anyhow::bail!("EMAIL_FROM must be set for the smtp sender"),
            };
            let app_password: String = match std::env::var("EMAIL_APP_PASSWORD") {
                Ok(s) if !s.is_empty() => s,
                _ => anyhow::bail!("EMAIL_APP_PASSWORD must be set for the smtp sender"),
            };
            let to = destination.to_string();
            let code = code.to_string();
            tokio::task::spawn_blocking(move || {
                send_email_blocking(&from, &app_password, &to, &code)
            })
            .await
            .map_err(|e| anyhow::anyhow!("smtp worker failed: {e}"))??;
            tracing::info!(
                channel = channel.name(),
                destination = masked.as_str(),
                "otp dispatched (smtp)"
            );
            Ok(())
        }
        other => anyhow::bail!("unknown LAB_OTP_SENDER backend: {other}"),
    }
}

/// Sends one OTP email through Gmail's SMTP relay (blocking).
///
/// Called from [`send_otp`] on the blocking pool. The app password is used
/// only for SMTP auth and is never logged.
///
/// # Errors
///
/// Fails on invalid addresses or SMTP transport errors.
fn send_email_blocking(from: &str, app_password: &str, to: &str, code: &str) -> anyhow::Result<()> {
    use lettre::{
        Message, SmtpTransport, Transport, message::Mailbox,
        transport::smtp::authentication::Credentials,
    };

    let email: Message = Message::builder()
        .from(
            from.parse::<Mailbox>()
                .map_err(|e| anyhow::anyhow!("invalid EMAIL_FROM address: {e}"))?,
        )
        .to(to
            .parse::<Mailbox>()
            .map_err(|e| anyhow::anyhow!("invalid recipient address: {e}"))?)
        .subject("Tu código de acceso")
        .body(format!(
            "Tu código de acceso al sistema de puntos es: {code}\n\
             Vence en {} minutos. Si no lo pediste, ignorá este mensaje.",
            OTP_TTL_SECS / 60
        ))?;
    let mailer: SmtpTransport = SmtpTransport::relay("smtp.gmail.com")
        .map_err(|e| anyhow::anyhow!("smtp relay setup failed: {e}"))?
        .credentials(Credentials::new(from.to_string(), app_password.to_string()))
        .build();
    mailer
        .send(&email)
        .map_err(|e| anyhow::anyhow!("smtp send failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codes are 6 ASCII digits and rotate between windows.
    #[test]
    fn codes_are_six_digits_and_rotate() {
        let secret = b"test-secret";
        let a = generate_code(secret, 1234567, 42);
        let b = generate_code(secret, 1234567, 43);
        assert_eq!(a.len(), 6);
        assert!(a.bytes().all(|c| c.is_ascii_digit()));
        assert_ne!(a, b);
        // Different clients get different codes in the same window.
        assert_ne!(a, generate_code(secret, 7654321, 42));
    }

    /// The guard throttles resends per window.
    #[test]
    fn guard_throttles_resends() {
        let guard = OtpGuard::new();
        for _ in 0..OTP_MAX_SENDS_PER_WINDOW {
            guard.check_and_record_send(1).unwrap();
        }
        assert_eq!(
            guard.check_and_record_send(1),
            Err(OtpError::TooManyRequests)
        );
    }

    /// Failures accumulate into a lockout; success clears the counters.
    #[test]
    fn guard_locks_out_after_failures() {
        let guard = OtpGuard::new();
        guard.check_not_locked(7).unwrap();
        for _ in 0..OTP_MAX_ATTEMPTS {
            guard.record_result(7, false);
        }
        assert!(matches!(
            guard.check_not_locked(7),
            Err(OtpError::Locked { .. })
        ));
        assert!(matches!(
            guard.check_and_record_send(7),
            Err(OtpError::Locked { .. })
        ));
        guard.record_result(7, true);
        guard.check_not_locked(7).unwrap();
    }

    /// Masking never leaks the full destination.
    #[test]
    fn masking_hides_destinations() {
        assert!(!mask_phone("+595981123456").contains("123456"));
        assert!(!mask_email("maria@example.com").contains("maria"));
        assert!(mask_email("maria@example.com").contains("@example.com"));
    }

    /// Test client with both OTP channels.
    fn both_channels_client() -> Clients {
        Clients {
            id: 1,
            ci: 1234567,
            verification_digit: None,
            first_name: "Test".to_string(),
            last_name: "User".to_string(),
            razon_social: None,
            domicilio: None,
            phone_number: Some("+595981000000".to_string()),
            email: Some("test@example.com".to_string()),
        }
    }

    /// Email is the default channel; SMS is the fallback.
    #[test]
    fn channel_defaults_to_email_then_sms() {
        let both = both_channels_client();
        assert_eq!(
            channel_for(&both, None).unwrap(),
            (OtpChannel::Email, "test@example.com".to_string())
        );
        let mut sms_only = both.clone();
        sms_only.email = None;
        assert_eq!(
            channel_for(&sms_only, None).unwrap(),
            (OtpChannel::Sms, "+595981000000".to_string())
        );
        let mut email_only = both.clone();
        email_only.phone_number = None;
        assert_eq!(
            channel_for(&email_only, None).unwrap(),
            (OtpChannel::Email, "test@example.com".to_string())
        );
    }

    /// Forced channels are honored, or report the missing destination.
    #[test]
    fn channel_forced_or_unavailable() {
        let mut client = both_channels_client();
        client.phone_number = None;
        assert_eq!(
            channel_for(&client, Some(OtpChannel::Email)).unwrap(),
            (OtpChannel::Email, "test@example.com".to_string())
        );
        assert_eq!(
            channel_for(&client, Some(OtpChannel::Sms)),
            Err(OtpError::ChannelUnavailable {
                channel: OtpChannel::Sms
            })
        );
        assert_eq!(OtpChannel::parse("sms").unwrap(), OtpChannel::Sms);
        assert_eq!(OtpChannel::parse("bogus"), Err(OtpError::UnknownChannel));
    }
}
