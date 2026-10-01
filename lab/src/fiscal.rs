//! # Paraguayan fiscal value types (`factura`).
//!
//! Validation for the invoice format in
//! <https://4invoices.net/py/modelo-factura> (DNIT / SIFEN).
//!
//! The loyalty-points domain deliberately stores **no prices, IVA breakdowns,
//! sale conditions or totals** — they are stripped from the model as out of
//! scope. What *is* validated here is the fiscal identity of every ticket:
//!
//! * [`InvoiceNumber`]: printed number `EEE-PPP-NNNNNNN`
//!   (`001-001-0000123`), 3 + 3 + 7 digits.
//! * [`Timbrado`]: 8-digit DNIT authorization (`Nº de Timbrado`).
//! * [`Cdc`]: 44-digit SIFEN control code with full structure + mod-11 DV.
//! * [`Ruc`]: `base-DV` (`80012345-6`), DV via DNIT mod-11 (base 11).
//!
//! ## The three supported cases
//!
//! The system accepts all Paraguayan invoice realities:
//!
//! | Case | `ticket_id` | `timbrado` | `cdc` |
//! |---|---|---|---|
//! | Paper (pre-printed) | mandatory | absent | absent |
//! | Timbrado paper | mandatory | mandatory | absent |
//! | Electronic (SIFEN) | mandatory | mandatory | mandatory |
//!
//! Invariant (also enforced in SQL): `cdc` requires `timbrado`.
//! When `cdc` is present its embedded establishment/point/number must equal
//! `ticket_id` and its embedded emitter RUC must equal the partner RUC
//! (checked in [`crate::db::create_ticket`]).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ============================================================
// Mod-11 (DNIT, base 11)
// ============================================================

/// DNIT mod-11 check digit over an all-digit string.
///
/// Weights 2..=11 cycling right-to-left; `resto > 1 ? 11 - resto : 0`.
/// Used for both RUC DVs and the CDC trailing DV.
pub fn mod11_dv(digits: &str) -> Option<u8> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut total: u32 = 0;
    let mut k: u32 = 2;
    for b in digits.bytes().rev() {
        total += u32::from(b - b'0') * k;
        k = if k >= 11 { 2 } else { k + 1 };
    }
    let resto = total % 11;
    Some(if resto > 1 {
        (11 - resto) as u8
    } else {
        0
    })
}

// ============================================================
// InvoiceNumber: EEE-PPP-NNNNNNN
// ============================================================

/// Printed invoice number `EEE-PPP-NNNNNNN` (e.g. `001-001-0000123`).
///
/// * `EEE`: establecimiento `001`-`999` (never `000`).
/// * `PPP`: punto de expedición `001`-`999`.
/// * `NNNNNNN`: sequential `0000001`-`9999999` (never all zeros).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InvoiceNumber(String);

impl InvoiceNumber {
    /// Validates and builds an invoice number from its printed form.
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let s = raw.trim();
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 3 {
            return Err("invalid invoice number (expected 000-000-0000000)");
        }
        let (eee, ppp, nnn) = (parts[0], parts[1], parts[2]);
        if eee.len() != 3 || ppp.len() != 3 || nnn.len() != 7 {
            return Err("invalid invoice number (expected 000-000-0000000)");
        }
        if !eee.bytes().all(|b| b.is_ascii_digit())
            || !ppp.bytes().all(|b| b.is_ascii_digit())
            || !nnn.bytes().all(|b| b.is_ascii_digit())
        {
            return Err("invalid invoice number (digits and dashes only)");
        }
        if eee == "000" || ppp == "000" || nnn == "0000000" {
            return Err("invalid invoice number (blocks must be >= 1)");
        }
        Ok(Self(s.to_string()))
    }

    /// Raw printed form (`001-001-0000123`).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `(establecimiento, punto, numero)` blocks.
    pub fn parts(&self) -> (&str, &str, &str) {
        (&self.0[0..3], &self.0[4..7], &self.0[8..15])
    }

    /// Establishment block (`EEE`).
    pub fn establishment(&self) -> &str {
        self.parts().0
    }

    /// Point-of-sale block (`PPP`).
    pub fn point(&self) -> &str {
        self.parts().1
    }

    /// Sequential block (`NNNNNNN`).
    pub fn number(&self) -> &str {
        self.parts().2
    }
}

impl fmt::Display for InvoiceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for InvoiceNumber {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// ============================================================
// Timbrado: 8 digits
// ============================================================

/// DNIT authorization number (`Nº de Timbrado`), exactly 8 digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timbrado(String);

impl Timbrado {
    /// Validates an 8-digit timbrado.
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let s = raw.trim();
        if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid timbrado (expected 8 digits)");
        }
        Ok(Self(s.to_string()))
    }

    /// Raw form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Timbrado {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Timbrado {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// ============================================================
// Ruc: base-DV
// ============================================================

/// Paraguayan RUC `base-DV` (e.g. `80012345-6`).
///
/// * `base`: 1-8 digits (individuals: CI number; companies: DNIT-assigned,
///   usually starting with `80`).
/// * `DV`: single mod-11 digit over `base` (see [`mod11_dv`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ruc {
    /// Numeric base without DV.
    pub base: String,
    /// Check digit.
    pub dv: u8,
}

impl Ruc {
    /// Validates `base-DV`, verifying the DV with [`mod11_dv`].
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let s = raw.trim();
        let (base, dv) = s.split_once('-').ok_or("invalid RUC (expected base-DV)")?;
        if base.is_empty() || base.len() > 8 || !base.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid RUC (base must be 1-8 digits)");
        }
        if dv.len() != 1 || !dv.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid RUC (DV must be one digit)");
        }
        // Strip leading zeros for the mod-11 input? No: DNIT computes over
        // the base as written; leading zeros change nothing arithmetically.
        let expected = mod11_dv(base).ok_or("invalid RUC (base not numeric)")?;
        let got = dv.bytes().next().unwrap() - b'0';
        if got != expected {
            return Err("invalid RUC (check digit mismatch)");
        }
        Ok(Self {
            base: base.to_string(),
            dv: got,
        })
    }

    /// Computes the DV for a numeric base (no validation of length).
    pub fn dv_for(base: &str) -> Option<u8> {
        mod11_dv(base.trim())
    }

    /// Canonical `base-DV` form.
    pub fn as_string(&self) -> String {
        format!("{}-{}", self.base, self.dv)
    }

    /// Base zero-padded to 8 digits (CDC emitter field).
    pub fn base_padded8(&self) -> String {
        format!("{:0>8}", self.base)
    }
}

impl fmt::Display for Ruc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.base, self.dv)
    }
}

impl FromStr for Ruc {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// ============================================================
// Cdc: 44 digits
// ============================================================

/// Decoded CDC blocks (SIFEN, 11 fields, 44 digits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdcParts {
    /// doc type: `01` FE, `04` AFE, `05` NCE, `06` NDE, `07` NRE.
    pub doc_type: String,
    /// emitter RUC base, 8 digits zero-padded.
    pub ruc_base: String,
    /// emitter RUC DV.
    pub ruc_dv: u8,
    /// establecimiento `001`-`999`.
    pub establishment: String,
    /// punto de expedición `001`-`999`.
    pub point: String,
    /// sequential `0000001`-`9999999`.
    pub number: String,
    /// contributor type: `1` física, `2` jurídica.
    pub contrib_type: char,
    /// issue date `AAAAMMDD`.
    pub date_yyyymmdd: String,
    /// emission type: `1` normal, `2` contingency.
    pub emi_type: char,
    /// 9-digit security code.
    pub security_code: String,
    /// trailing mod-11 DV over the previous 43 digits.
    pub dv: u8,
}

/// SIFEN control code: 44 numeric digits, irrepeatable per document.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cdc(String);

impl Cdc {
    /// Validates length/digits, structure ranges and the trailing mod-11 DV.
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let s = raw.trim();
        if s.len() != 44 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid CDC (expected 44 digits)");
        }
        let parts = Self::parts_of(s)?;
        let expected = mod11_dv(&s[..43]).ok_or("invalid CDC (DV input)")?;
        if expected != parts.dv {
            return Err("invalid CDC (check digit mismatch)");
        }
        Ok(Self(s.to_string()))
    }

    /// Decodes the 11 CDC blocks without checking the trailing DV.
    pub fn parts_of(s: &str) -> Result<CdcParts, &'static str> {
        if s.len() != 44 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid CDC (expected 44 digits)");
        }
        let doc_type = s[0..2].to_string();
        if !matches!(doc_type.as_str(), "01" | "04" | "05" | "06" | "07") {
            return Err("invalid CDC (unknown document type)");
        }
        let establishment = s[11..14].to_string();
        let point = s[14..17].to_string();
        let number = s[17..24].to_string();
        if establishment == "000" || point == "000" || number == "0000000" {
            return Err("invalid CDC (number blocks must be >= 1)");
        }
        let contrib_type = s.chars().nth(24).unwrap();
        if !matches!(contrib_type, '1' | '2') {
            return Err("invalid CDC (unknown contributor type)");
        }
        let date_yyyymmdd = s[25..33].to_string();
        if !valid_yyyymmdd(&date_yyyymmdd) {
            return Err("invalid CDC (bad issue date)");
        }
        let emi_type = s.chars().nth(33).unwrap();
        if !matches!(emi_type, '1' | '2') {
            return Err("invalid CDC (unknown emission type)");
        }
        Ok(CdcParts {
            doc_type,
            ruc_base: s[2..10].to_string(),
            ruc_dv: s.as_bytes()[10] - b'0',
            establishment,
            point,
            number,
            contrib_type,
            date_yyyymmdd,
            emi_type,
            security_code: s[34..43].to_string(),
            dv: s.as_bytes()[43] - b'0',
        })
    }

    /// Raw 44-digit form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Decoded blocks (DV already verified by [`Cdc::parse`]).
    pub fn parts(&self) -> CdcParts {
        Self::parts_of(&self.0).expect("CDC was validated at parse time")
    }

    /// Invoice number embedded in the CDC (`EEE-PPP-NNNNNNN`).
    pub fn invoice_number(&self) -> InvoiceNumber {
        let p = self.parts();
        InvoiceNumber::parse(&format!("{}-{}-{}", p.establishment, p.point, p.number))
            .expect("CDC blocks are valid invoice blocks")
    }

    /// Emitter RUC embedded in the CDC (`base-DV`, base unpadded).
    pub fn emitter_ruc(&self) -> Ruc {
        let p = self.parts();
        let base = p.ruc_base.trim_start_matches('0');
        let base = if base.is_empty() { "0" } else { base };
        Ruc {
            base: base.to_string(),
            dv: p.ruc_dv,
        }
    }
}

impl fmt::Display for Cdc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Cdc {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Strict `AAAAMMDD` calendar check (no external date dep).
fn valid_yyyymmdd(s: &str) -> bool {
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let y: u32 = s[0..4].parse().unwrap_or(0);
    let m: u32 = s[4..6].parse().unwrap_or(0);
    let d: u32 = s[6..8].parse().unwrap_or(0);
    if !(1990..=2100).contains(&y) || !(1..=12).contains(&m) || d < 1 {
        return false;
    }
    let dim = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    d <= dim
}

// ============================================================
// Document kind + cross-checks
// ============================================================

/// Which fiscal reality a ticket belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiscalDocumentKind {
    /// Pre-printed paper: number only.
    Paper,
    /// Paper with DNIT authorization: number + timbrado.
    WithTimbrado,
    /// SIFEN electronic: number + timbrado + CDC.
    Electronic,
}

impl FiscalDocumentKind {
    /// Classifies `(timbrado, cdc)` presence; `None` when CDC without timbrado.
    pub fn classify(timbrado: Option<&str>, cdc: Option<&str>) -> Option<Self> {
        match (
            timbrado.map(|s| !s.trim().is_empty()).unwrap_or(false),
            cdc.map(|s| !s.trim().is_empty()).unwrap_or(false),
        ) {
            (false, false) => Some(Self::Paper),
            (true, false) => Some(Self::WithTimbrado),
            (true, true) => Some(Self::Electronic),
            (false, true) => None,
        }
    }
}

/// Cross-checks a CDC against its ticket number and emitter RUC.
///
/// * CDC number blocks must equal `invoice`.
/// * CDC emitter base/DV must equal `partner_ruc` (numeric comparison,
///   ignoring zero-padding).
/// * When the CDC doc type is `01` (factura electrónica) nothing else is
///   required; other DTE types are rejected for loyalty tickets.
pub fn check_cdc_consistency(
    cdc: &Cdc,
    invoice: &InvoiceNumber,
    partner_ruc: &Ruc,
) -> Result<(), &'static str> {
    let p = cdc.parts();
    if p.doc_type != "01" {
        return Err("CDC must be a factura electrónica (type 01)");
    }
    if cdc.invoice_number() != *invoice {
        return Err("CDC number does not match the invoice number");
    }
    let cdc_ruc = cdc.emitter_ruc();
    fn norm(b: &str) -> &str {
        let t = b.trim_start_matches('0');
        if t.is_empty() { "0" } else { t }
    }
    if norm(&cdc_ruc.base) != norm(&partner_ruc.base) || cdc_ruc.dv != partner_ruc.dv {
        return Err("CDC emitter RUC does not match the partner RUC");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoice_number_accepts_seed_format() {
        let n = InvoiceNumber::parse("001-001-0000001").unwrap();
        assert_eq!(n.parts(), ("001", "001", "0000001"));
    }

    #[test]
    fn invoice_number_rejects_free_text() {
        assert!(InvoiceNumber::parse("INV-2026-001234").is_err());
        assert!(InvoiceNumber::parse("001-001-123").is_err());
        assert!(InvoiceNumber::parse("000-001-0000001").is_err());
        assert!(InvoiceNumber::parse("001-001-0000000").is_err());
    }

    #[test]
    fn timbrado_is_8_digits() {
        assert!(Timbrado::parse("10151403").is_ok());
        assert!(Timbrado::parse("1234567").is_err());
        assert!(Timbrado::parse("123456789").is_err());
        assert!(Timbrado::parse("abcdefgh").is_err());
    }

    #[test]
    fn ruc_mod11_validates() {
        // `80012345` -> DV 0 under DNIT mod-11 (base 11). Note: the
        // 4invoices.net example prints `80012345-6`, but that page marks its
        // identifying data as illustrative, not DV-correct.
        let r = Ruc::parse("80012345-0").unwrap();
        assert_eq!(r.base, "80012345");
        assert_eq!(r.dv, 0);
    }

    #[test]
    fn ruc_rejects_bad_dv() {
        assert!(Ruc::parse("80012345-6").is_err());
    }

    #[test]
    fn cdc_structure_and_dv() {
        // Build a CDC deterministically, then parse it back:
        // 01 | 80012345 | 0 | 001 | 001 | 0000001 | 2 | 20260422 | 1 | 000000001
        let base43 = [
            "01", "80012345", "0", "001", "001", "0000001", "2", "20260422", "1",
            "000000001",
        ]
        .concat();
        assert_eq!(base43.len(), 43);
        let dv = mod11_dv(&base43).unwrap();
        let full = format!("{base43}{dv}");
        let cdc = Cdc::parse(&full).unwrap();
        let p = cdc.parts();
        assert_eq!(p.doc_type, "01");
        assert_eq!(p.establishment, "001");
        assert_eq!(p.point, "001");
        assert_eq!(p.number, "0000001");
        assert_eq!(cdc.invoice_number().as_str(), "001-001-0000001");
    }

    #[test]
    fn cdc_rejects_bad_dv() {
        let base43 = [
            "01", "80012345", "0", "001", "001", "0000001", "2", "20260422", "1",
            "000000001",
        ]
        .concat();
        let dv = mod11_dv(&base43).unwrap();
        let mut full = format!("{base43}{dv}");
        // Flip the last digit.
        let last = full.pop().unwrap();
        let flipped = if last == '0' { '1' } else { '0' };
        full.push(flipped);
        assert!(Cdc::parse(&full).is_err());
    }

    #[test]
    fn kind_classification() {
        assert_eq!(
            FiscalDocumentKind::classify(None, None),
            Some(FiscalDocumentKind::Paper)
        );
        assert_eq!(
            FiscalDocumentKind::classify(Some("10151403"), None),
            Some(FiscalDocumentKind::WithTimbrado)
        );
        assert_eq!(
            FiscalDocumentKind::classify(
                Some("12345678"),
                Some("01800123450001001000000122026010110000000012")
            ),
            Some(FiscalDocumentKind::Electronic)
        );
        assert_eq!(FiscalDocumentKind::classify(None, Some("0".repeat(44).as_str())), None);
    }
}
