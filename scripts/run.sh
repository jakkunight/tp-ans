#!/usr/bin/env bash
# run.sh — prepara la base de datos y ejecuta el servidor `lab` (Linux/macOS).
#
# Pasos:
#   1. Verifica dependencias (cargo, psql, bun/bunx para Tailwind en build.rs).
#   2. Verifica que PostgreSQL responda en 127.0.0.1:5432 (intenta iniciarlo).
#   3. Crea el rol/base de desarrollo si faltan (por defecto lab/lab).
#   4. Aplica lab/database/schema.sql solo si la BD está vacía (no es
#      idempotente) y siempre lab/database/seed.sql (idempotente).
#   5. Ejecuta el servidor en lab/ con `secretspec run` si hay clave age +
#      secretspec; si no, con variables de desarrollo por defecto.
#
# El servidor escucha en http://127.0.0.1:8080
#
# Uso:
#   ./scripts/run.sh [--release] [--skip-db] [--skip-seed] [--profile NOMBRE]
#   Variables opcionales: LAB_DB_USER, LAB_DB_PASS, LAB_DB_DATABASE,
#     LAB_DB_HOST, LAB_DB_PORT, SOPS_AGE_KEY_FILE, PGPASSWORD (clave del
#     superusuario postgres para crear el rol, si hace falta).
set -euo pipefail

RELEASE=0
WITH_DB=1
WITH_SEED=1
PROFILE="${SECRETSPEC_PROFILE:-development}"

usage() {
  cat <<'EOF'
Uso: run.sh [opciones] [-- args extra para cargo run]

Opciones:
  --release          Compila/ejecuta en modo release.
  --skip-db          Omitir la preparación de la base de datos.
  --skip-seed        No aplicar seed.sql (solo esquema si la BD está vacía).
  --profile NOMBRE   Perfil de secretspec (por defecto: development).
  -h, --help         Mostrar esta ayuda.
EOF
}

EXTRA_ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --release) RELEASE=1; shift ;;
    --skip-db) WITH_DB=0; shift ;;
    --skip-seed) WITH_SEED=0; shift ;;
    --profile) PROFILE="${2:?falta el nombre del perfil}"; shift 2 ;;
    --) shift; EXTRA_ARGS+=("$@"); break ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: opción desconocida: $1" >&2; usage >&2; exit 1 ;;
  esac
done

have() { command -v "$1" >/dev/null 2>&1; }
log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mAdvertencia:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Valores de desarrollo (coinciden con devenv.nix y secrets.enc.yaml).
DB_USER="${LAB_DB_USER:-lab}"
DB_PASS="${LAB_DB_PASS:-lab}"
DB_NAME="${LAB_DB_DATABASE:-lab}"
DB_HOST="${LAB_DB_HOST:-127.0.0.1}"
DB_PORT="${LAB_DB_PORT:-5432}"

# --- 1. Dependencias ------------------------------------------------------------
missing=0
for cmd in cargo psql; do
  have "$cmd" || { warn "falta '$cmd' (corré scripts/install-linux.sh o install-macos.sh)."; missing=1; }
done
if ! have bun && ! have bunx; then
  warn "falta 'bun'/'bunx' (build.rs compila Tailwind con bunx; instalalo o el build fallará)."
fi
[ "$missing" -eq 1 ] && die "faltan dependencias."
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

# --- 2. PostgreSQL en marcha ----------------------------------------------------
pg_ready() { pg_isready -h "$DB_HOST" -p "$DB_PORT" >/dev/null 2>&1; }

if [ "$WITH_DB" -eq 1 ]; then
  if ! pg_ready; then
    log "PostgreSQL no responde; intentando iniciarlo..."
    if [ "$(uname)" = "Darwin" ]; then
      brew services start postgresql@16 || true
    elif have systemctl; then
      sudo systemctl start postgresql || true
    elif have service; then
      sudo service postgresql start || true
    fi
    sleep 2
  fi
  pg_ready || die "PostgreSQL no responde en $DB_HOST:$DB_PORT. Inícialo manualmente."
  log "PostgreSQL responde en $DB_HOST:$DB_PORT."
fi

# --- 3-4. Rol, base, esquema y seed ---------------------------------------------
# psql_super ejecuta SQL como superusuario probando, en orden:
#   a) conexión peer local (socket, usuario postgres vía sudo),
#   b) TCP como postgres con $PGPASSWORD,
#   c) la conexión de desarrollo (por si el rol ya es superusuario).
psql_super() {
  local sql="$1"
  if have sudo && sudo -n true 2>/dev/null; then
    if sudo -u postgres psql -v ON_ERROR_STOP=1 -tA -c "$sql" postgres 2>/dev/null; then
      return 0
    fi
  fi
  if [ -n "${PGPASSWORD:-}" ]; then
    if PGPASSWORD="$PGPASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U postgres \
        -v ON_ERROR_STOP=1 -tA -c "$sql" postgres 2>/dev/null; then
      return 0
    fi
  fi
  return 1
}

if [ "$WITH_DB" -eq 1 ]; then
  # Rol de desarrollo (superusuario local: solo entorno de desarrollo).
  # Se preserva PGPASSWORD del usuario y se restaura al final del bloque.
  _SAVED_PGPASSWORD="${PGPASSWORD:-__unset__}"
  if PGPASSWORD="$DB_PASS" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" \
      -d postgres -tA -c 'SELECT 1' >/dev/null 2>&1; then
    log "Rol '$DB_USER' ya accesible."
  else
    log "Creando rol '$DB_USER' (superusuario local, solo desarrollo)..."
    # Escapa comilla simple en la contraseña para el literal SQL.
    esc_pass="${DB_PASS//\'/\'\'}"
    if ! psql_super "SELECT 1 FROM pg_roles WHERE rolname = '$DB_USER'" | grep -q 1; then
      psql_super "CREATE ROLE \"$DB_USER\" WITH LOGIN SUPERUSER PASSWORD '$esc_pass'" >/dev/null \
        || die "no se pudo crear el rol. Exportá PGPASSWORD con la clave del superusuario postgres y reintentá."
    else
      psql_super "ALTER ROLE \"$DB_USER\" WITH LOGIN SUPERUSER PASSWORD '$esc_pass'" >/dev/null || true
    fi
  fi

  export PGPASSWORD="$DB_PASS"
  # Base de datos: primero como el propio rol (tiene CREATEDB si lo creó este
  # script), si no como superusuario.
  if psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" \
      -d "$DB_NAME" -tA -c 'SELECT 1' >/dev/null 2>&1; then
    log "Base '$DB_NAME' ya existe."
  else
    log "Creando base '$DB_NAME' (dueño: $DB_USER)..."
    if ! createdb -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -O "$DB_USER" "$DB_NAME" >/dev/null 2>&1; then
      psql_super "CREATE DATABASE \"$DB_NAME\" OWNER \"$DB_USER\"" >/dev/null \
        || die "no se pudo crear la base. Exportá PGPASSWORD con la clave del superusuario postgres y reintentá."
    fi
  fi
  # Esquema solo si la BD está vacía (schema.sql usa CREATE TABLE sin IF NOT EXISTS).
  if [ "$(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" \
      -tA -c "SELECT to_regclass('public.partners')")" = "partners" ]; then
    log "Esquema ya aplicado (tabla partners existe)."
  else
    log "Aplicando lab/database/schema.sql..."
    psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" \
      -v ON_ERROR_STOP=1 -f lab/database/schema.sql
  fi

  if [ "$WITH_SEED" -eq 1 ]; then
    log "Aplicando lab/database/seed.sql (idempotente)..."
    psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" \
      -v ON_ERROR_STOP=1 -f lab/database/seed.sql
  fi
  if [ "$_SAVED_PGPASSWORD" = "__unset__" ]; then
    unset PGPASSWORD
  else
    export PGPASSWORD="$_SAVED_PGPASSWORD"
  fi
fi

# --- 5. Servidor ------------------------------------------------------------------
# build.rs corre `bunx @tailwindcss/cli`; la primera compilación necesita red.
cd lab

if have secretspec; then
  KEY_FILE="${SOPS_AGE_KEY_FILE:-$HOME/.config/sops/age/dev-key.txt}"
  if [ -f "$KEY_FILE" ]; then
    export SOPS_AGE_KEY_FILE="$KEY_FILE"
    log "Ejecutando con secretos (perfil '$PROFILE', clave: $KEY_FILE)..."
    if [ "$RELEASE" -eq 1 ]; then
      exec secretspec run --profile "$PROFILE" -- cargo run --release "${EXTRA_ARGS[@]}"
    else
      exec secretspec run --profile "$PROFILE" -- cargo run "${EXTRA_ARGS[@]}"
    fi
  else
    warn "hay secretspec pero no existe la clave age ($KEY_FILE)."
    warn "Pedí la clave al equipo o generá la tuya; mientras tanto uso valores de desarrollo."
  fi
else
  warn "sin secretspec: uso variables de desarrollo por defecto (ver scripts/secrets-decode.sh)."
fi

export LAB_DB_URL="postgres://${DB_USER}:${DB_PASS}@${DB_HOST}:${DB_PORT}/${DB_NAME}"
export LAB_JWT_SECRET="${LAB_JWT_SECRET:-dev-only-insecure-change-me}"
export LAB_OTP_SECRET="${LAB_OTP_SECRET:-$LAB_JWT_SECRET}"
export LAB_OTP_SENDER="${LAB_OTP_SENDER:-log}"
log "LAB_DB_URL=$LAB_DB_URL | OTP_SENDER=$LAB_OTP_SENDER"
log "Servidor en http://127.0.0.1:8080 (Ctrl+C para detener)..."
if [ "$RELEASE" -eq 1 ]; then
  exec cargo run --release "${EXTRA_ARGS[@]}"
else
  exec cargo run "${EXTRA_ARGS[@]}"
fi
