#!/usr/bin/env bash
# post-ticket.sh — post tickets (facturas) to the lab API from the terminal.
#
# Requirements: curl, python3 (for JSON building/pretty-printing).
# No jq needed.
#
# Usage:
#   ./post-ticket.sh --partner 1 --ticket-id 001-001-0000009 --ci 1234567 --first-name María --last-name González --item 1:2
#   ./post-ticket.sh --partner 1 --ticket-id 001-001-0000010 --timbrado 12345678 --ci 9990001 --first-name Ana --last-name López --phone +595983000001 --item 2:3
#   ./post-ticket.sh --partner 1 --ticket-id 001-001-0000001 --timbrado 12345678 --cdc 01800123450001001000000122026010110000000012 --ci 1234567 --first-name María --last-name González --item 1:1 --token "$JWT" --dry-run
#   ./post-ticket.sh --delete 5 --reason "factura duplicada"  # void ticket 5 (DELETE /partners/tickets)
#   ./post-ticket.sh --help                # show IDs from lab/database/seed.sql
#
# Fiscal identity (factura PY): paper sends only --ticket-id (EEE-PPP-NNNNNNN);
# timbrado paper adds --timbrado (8 digits); electronic adds --timbrado + --cdc
# (44 digits, SIFEN). --verification-digit is the RUC DV (mod-11 must match CI).
#
# The buyer is taken from the factura data and registered on the fly:
# clients are looked up by CI (unique) and created when unknown. New clients
# need --phone and/or --email (the schema requires an OTP channel).
#
# Ticket routes require a partner JWT: log in first (RUC + pre-shared key)
# and pass --token (or $LAB_JWT_TOKEN). The Rust binary can do both in one call
# (see lab/src/bin/post-ticket.rs --login-partner --login-secret).
#
# Endpoint: POST /api/v1/partners/tickets
#   Body: {"partner_id":1,"ticket_id":"001-001-0000009","timbrado":"12345678","client":{"ci":1234567,"first_name":"María","last_name":"González"},"details":[{"product_id":1,"quantity":2}]}
#   Reply: {"ticket_id":N,"client_id":M,"earned_points":K}
set -u

BASE_URL="${LAB_BASE_URL:-http://127.0.0.1:8080}"
TOKEN="${LAB_JWT_TOKEN:-}"
PARTNER=""
TICKET_NO=""
TIMBRADO=""
CDC=""
CI=""
FIRST=""
LAST=""
VDIGIT=""
RAZON=""
DOMICILIO=""
PHONE=""
EMAIL=""
REASON=""
DELETE_ID=""
DRY_RUN=0
ITEMS=()

usage() {
  cat <<'EOF'
post-ticket.sh — post tickets to the lab API from the terminal.

Usage:
  post-ticket.sh --partner ID --ticket-id NRO --ci CI --first-name NAME --last-name NAME --item PID:QTY [...] [options]
  post-ticket.sh --delete TICKET_ID --reason MOTIVO [options]

Options:
  --partner ID     Partner (farmacia) id. Required unless --delete.
  --ticket-id NRO  Printed invoice number EEE-PPP-NNNNNNN (unique per partner). Required to post.
  --timbrado T     8-digit DNIT timbrado (timbrado paper + electronic only).
  --cdc CDC        44-digit SIFEN CDC (electronic only; requires --timbrado).
  --ci CI          Buyer CI from the factura (client is registered if new).
                   Required unless --delete.
  --first-name N   Buyer first name as printed on the factura. Required unless --delete.
  --last-name N    Buyer last name as printed on the factura. Required unless --delete.
  --verification-digit D
                   Optional <CI>-<D> RUC check digit (0-9, mod-11 must match CI).
  --razon-social R Company name for juridical receptors (optional).
  --domicilio DIR  Receptor address printed on the factura (optional).
  --phone NUM      Buyer phone (SMS channel for OTP). New clients need --phone and/or --email.
  --email ADDR     Buyer email (channel for OTP). New clients need --phone and/or --email.
  --item PID:QTY   One ticket line; repeatable. At least one required.
                   Example: --item 1:2 --item 3:1
  --base-url URL   API base URL. Default: $LAB_BASE_URL or http://127.0.0.1:8080
  --token TOKEN    Bearer JWT for the ticket routes (required).
                   Default: $LAB_JWT_TOKEN.
  --delete ID      Void ticket ID instead of posting (DELETE /partners/tickets).
  --reason MOTIVO  Why the invoice is voided. Required with --delete.
  --dry-run        Print the JSON body without sending it.
  -h, --help       Show this help plus the seed-data IDs.

Seed data (lab/database/seed.sql):
  Partners: 1 Farmacia Central (RUC 80012345-0), 2 Farmacia del Sur (RUC 80067890-7), 3 Inactiva
  Clients:  12 clientes (1 María González CI 1234567, 2 Juan Pérez CI 2345678, ...)
  Products: 12 (1 Paracetamol 10 pts, 2 Ibuprofeno 8 pts, 3 Vitamina C 5 pts,
            4 Crema 3 pts, 7 Alcohol 6 pts, 8 Jabón 4 pts, 9 Protector 12 pts;
            canje (todos con precio): 1 Paracet 10, 2 Ibupr 8, 3 Vitam 50,
            4 Crema 3, 5 Termo 100, 6 Mochila 250, 7 Alcohol 6, 8 Jabón 4,
            9 Protector 12, 10 Termo Dep. 150, 11 Gorra 80, 12 Kit 300)
EOF
}

die() { echo "error: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --partner) PARTNER="${2:-}"; shift 2 ;;
    --ticket-id) TICKET_NO="${2:-}"; shift 2 ;;
    --timbrado) TIMBRADO="${2:-}"; shift 2 ;;
    --cdc) CDC="${2:-}"; shift 2 ;;
    --ci) CI="${2:-}"; shift 2 ;;
    --first-name) FIRST="${2:-}"; shift 2 ;;
    --last-name) LAST="${2:-}"; shift 2 ;;
    --verification-digit) VDIGIT="${2:-}"; shift 2 ;;
    --razon-social) RAZON="${2:-}"; shift 2 ;;
    --domicilio) DOMICILIO="${2:-}"; shift 2 ;;
    --phone) PHONE="${2:-}"; shift 2 ;;
    --email) EMAIL="${2:-}"; shift 2 ;;
    --reason) REASON="${2:-}"; shift 2 ;;
    --item) ITEMS+=("${2:-}"); shift 2 ;;
    --base-url) BASE_URL="$2"; shift 2 ;;
    --token) TOKEN="$2"; shift 2 ;;
    --delete) DELETE_ID="${2:-}"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
done

command -v curl >/dev/null || die "curl is required"
command -v python3 >/dev/null || die "python3 is required"

BASE_URL="${BASE_URL%/}"

# ---- DELETE mode: void a ticket (audit entry; rows stay intact) ----
if [ -n "$DELETE_ID" ]; then
  [[ "$DELETE_ID" =~ ^[0-9]+$ ]] || die "--delete expects a numeric ticket id"
  [ -n "$REASON" ] || die "--reason is required with --delete"
  BODY="$(TICKET="$DELETE_ID" REASON="$REASON" python3 -c 'import json,os; print(json.dumps({"ticket_id": int(os.environ["TICKET"]), "reason": os.environ["REASON"]}))')"
  [ "$DRY_RUN" -eq 1 ] && { echo "$BODY"; exit 0; }
  AUTH=(); [ -n "$TOKEN" ] && AUTH=(-H "Authorization: Bearer $TOKEN")
  TMP="$(mktemp)"; CODE="$(curl -sS -o "$TMP" -w '%{http_code}' -X DELETE \
    "$BASE_URL/api/v1/partners/tickets" \
    -H 'Content-Type: application/json' "${AUTH[@]}" --data "$BODY")"
  echo "HTTP $CODE"
  python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])), indent=2, ensure_ascii=False))' "$TMP" 2>/dev/null || cat "$TMP"
  echo; rm -f "$TMP"
  [ "$CODE" -ge 200 ] && [ "$CODE" -lt 300 ] && exit 0 || exit 1
fi

# ---- POST mode: validate ----
[[ "$PARTNER" =~ ^[0-9]+$ ]] || die "--partner ID is required and must be numeric"
[ -n "$TICKET_NO" ] || die "--ticket-id NRO is required (invoice number, unique per partner)"
[[ "$CI" =~ ^[0-9]+$ ]] || die "--ci is required and must be numeric"
[ -n "$FIRST" ] || die "--first-name is required"
[ -n "$LAST" ] || die "--last-name is required"
if [ -n "$VDIGIT" ]; then
  [[ "$VDIGIT" =~ ^[0-9]$ ]] || die "--verification-digit must be a single digit 0-9"
fi
[ "${#ITEMS[@]}" -ge 1 ] || die "at least one --item PID:QTY is required"
for it in "${ITEMS[@]}"; do
  [[ "$it" =~ ^[0-9]+:[0-9]+$ ]] || die "bad --item '$it', expected PID:QTY (e.g. 1:2)"
done

# Build JSON body with python3 (no jq needed).
BODY="$(PARTNER="$PARTNER" TICKET_NO="$TICKET_NO" TIMBRADO="$TIMBRADO" CDC="$CDC" CI="$CI" FIRST="$FIRST" LAST="$LAST" VDIGIT="$VDIGIT" RAZON="$RAZON" DOMICILIO="$DOMICILIO" PHONE="$PHONE" EMAIL="$EMAIL" python3 - "$PARTNER" "${ITEMS[@]}" <<'PY'
import json, os, sys
partner_id, *items = sys.argv[1:]
client = {
    "ci": int(os.environ["CI"]),
    "first_name": os.environ["FIRST"],
    "last_name": os.environ["LAST"],
}
if os.environ["VDIGIT"] != "":
    client["verification_digit"] = int(os.environ["VDIGIT"])
if os.environ["RAZON"] != "":
    client["razon_social"] = os.environ["RAZON"]
if os.environ["DOMICILIO"] != "":
    client["domicilio"] = os.environ["DOMICILIO"]
if os.environ["PHONE"] != "":
    client["phone_number"] = os.environ["PHONE"]
if os.environ["EMAIL"] != "":
    client["email"] = os.environ["EMAIL"]
details = []
for it in items:
    pid, qty = it.split(":")
    pid, qty = int(pid), int(qty)
    assert pid >= 1 and qty >= 1, f"bad item {it}"
    details.append({"product_id": pid, "quantity": qty})
body = {"partner_id": int(partner_id), "ticket_id": os.environ["TICKET_NO"], "client": client, "details": details}
if os.environ["TIMBRADO"] != "":
    body["timbrado"] = os.environ["TIMBRADO"]
if os.environ["CDC"] != "":
    body["cdc"] = os.environ["CDC"]
print(json.dumps(body))
PY
)" || die "failed to build JSON body"

if [ "$DRY_RUN" -eq 1 ]; then
  echo "$BODY" | python3 -c 'import json,sys; print(json.dumps(json.loads(sys.stdin.read()), indent=2))'
  exit 0
fi

AUTH=(); [ -n "$TOKEN" ] && AUTH=(-H "Authorization: Bearer $TOKEN")
TMP="$(mktemp)"
CODE="$(curl -sS -o "$TMP" -w '%{http_code}' -X POST \
  "$BASE_URL/api/v1/partners/tickets" \
  -H 'Content-Type: application/json' "${AUTH[@]}" --data "$BODY")"
echo "HTTP $CODE"
python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])), indent=2, ensure_ascii=False))' "$TMP" 2>/dev/null || cat "$TMP"
echo
rm -f "$TMP"
[ "$CODE" -ge 200 ] && [ "$CODE" -lt 300 ] && exit 0 || exit 1
