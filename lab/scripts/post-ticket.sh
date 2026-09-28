#!/usr/bin/env bash
# post-ticket.sh — post tickets (facturas) to the lab API from the terminal.
#
# Requirements: curl, python3 (for JSON building/pretty-printing).
# No jq needed.
#
# Usage:
#   ./post-ticket.sh --partner 1 --ci 1234567 --first-name María --last-name González --item 1:2
#   ./post-ticket.sh --partner 1 --ci 9990001 --first-name Ana --last-name López --item 2:3 --base-url http://127.0.0.1:8080
#   ./post-ticket.sh --partner 1 --ci 1234567 --first-name María --last-name González --item 1:1 --token "$JWT" --dry-run
#   ./post-ticket.sh --delete 5            # void ticket 5 (DELETE /partners/tickets)
#   ./post-ticket.sh --help                # show IDs from lab/database/seed.sql
#
# The buyer is taken from the factura data and registered on the fly:
# clients are looked up by CI (unique) and created when unknown.
#
# Endpoint: POST /api/v1/partners/tickets
#   Body: {"partner_id":1,"client":{"ci":1234567,"first_name":"María","last_name":"González"},"details":[{"product_id":1,"quantity":2}]}
#   Reply: {"ticket_id":N,"client_id":M,"earned_points":K}
set -u

BASE_URL="${LAB_BASE_URL:-http://127.0.0.1:8080}"
TOKEN="${LAB_JWT_TOKEN:-}"
PARTNER=""
CI=""
FIRST=""
LAST=""
VDIGIT=""
DELETE_ID=""
DRY_RUN=0
ITEMS=()

usage() {
  cat <<'EOF'
post-ticket.sh — post tickets to the lab API from the terminal.

Usage:
  post-ticket.sh --partner ID --ci CI --first-name NAME --last-name NAME --item PID:QTY [...] [options]
  post-ticket.sh --delete TICKET_ID [options]

Options:
  --partner ID     Partner (farmacia) id. Required unless --delete.
  --ci CI          Buyer CI from the factura (client is registered if new).
                   Required unless --delete.
  --first-name N   Buyer first name as printed on the factura. Required unless --delete.
  --last-name N    Buyer last name as printed on the factura. Required unless --delete.
  --verification-digit D
                   Optional <CI>-<D> RUC suffix digit (0-9).
  --item PID:QTY   One ticket line; repeatable. At least one required.
                   Example: --item 1:2 --item 3:1
  --base-url URL   API base URL. Default: $LAB_BASE_URL or http://127.0.0.1:8080
  --token TOKEN    Bearer JWT. Default: $LAB_JWT_TOKEN or empty (API is open).
  --delete ID      Void ticket ID instead of posting (DELETE /partners/tickets).
  --dry-run        Print the JSON body without sending it.
  -h, --help       Show this help plus the seed-data IDs.

Seed data (lab/database/seed.sql):
  Partners: 1 Farmacia Central (activa), 2 Farmacia del Sur (activa), 3 Inactiva
  Clients:  1 María González (CI 1234567), 2 Juan Pérez (CI 2345678), 3 Ana López
  Products: 1 Paracetamol (10 pts), 2 Ibuprofeno (8 pts), 3 Vitamina C (5 pts),
            4 Crema (3 pts), 5 Termo (canje 100 pts), 6 Mochila (canje 250 pts)
EOF
}

die() { echo "error: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --partner) PARTNER="${2:-}"; shift 2 ;;
    --ci) CI="${2:-}"; shift 2 ;;
    --first-name) FIRST="${2:-}"; shift 2 ;;
    --last-name) LAST="${2:-}"; shift 2 ;;
    --verification-digit) VDIGIT="${2:-}"; shift 2 ;;
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

# ---- DELETE mode: void a ticket ----
if [ -n "$DELETE_ID" ]; then
  [[ "$DELETE_ID" =~ ^[0-9]+$ ]] || die "--delete expects a numeric ticket id"
  BODY="$(PARTNER="$DELETE_ID" python3 -c 'import json,os; print(json.dumps({"ticket_id": int(os.environ["PARTNER"])}))')"
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
BODY="$(PARTNER="$PARTNER" CI="$CI" FIRST="$FIRST" LAST="$LAST" VDIGIT="$VDIGIT" python3 - "$PARTNER" "${ITEMS[@]}" <<'PY'
import json, os, sys
partner_id, *items = sys.argv[1:]
client = {
    "ci": int(os.environ["CI"]),
    "first_name": os.environ["FIRST"],
    "last_name": os.environ["LAST"],
}
if os.environ["VDIGIT"] != "":
    client["verification_digit"] = int(os.environ["VDIGIT"])
details = []
for it in items:
    pid, qty = it.split(":")
    pid, qty = int(pid), int(qty)
    assert pid >= 1 and qty >= 1, f"bad item {it}"
    details.append({"product_id": pid, "quantity": qty})
print(json.dumps({"partner_id": int(partner_id), "client": client, "details": details}))
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
