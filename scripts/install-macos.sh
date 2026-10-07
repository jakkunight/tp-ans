#!/usr/bin/env bash
# install-macos.sh — instala todas las dependencias del proyecto en macOS
# y deja el entorno listo para `scripts/run.sh`.
#
# Requiere macOS 13+ (Apple Silicon o Intel).
#
# Instala (vía Homebrew, instalándolo si falta):
#   - Xcode Command Line Tools (compilador C, requerido por aws-lc-rs)
#   - git, postgresql@16, sops, age, bun
#   - Rust estable vía rustup
#   - sqlx-cli (cargo)
#   - secretspec (instalador oficial, con respaldo vía cargo)
#   - Inicia el servicio PostgreSQL con `brew services`
#
# Uso:
#   ./scripts/install-macos.sh [--no-postgres] [--no-rust]
#                              [--no-bun] [--no-secrets]
set -euo pipefail

WITH_POSTGRES=1
WITH_RUST=1
WITH_BUN=1
WITH_SECRETS=1

usage() {
  cat <<'EOF'
Uso: install-macos.sh [opciones]

Opciones:
  --no-postgres    Omitir PostgreSQL.
  --no-rust        Omitir Rust (rustup) y herramientas cargo (sqlx-cli...).
  --no-bun         Omitir Bun.
  --no-secrets     Omitir sops, age y secretspec.
  -h, --help       Mostrar esta ayuda.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --no-postgres) WITH_POSTGRES=0; shift ;;
    --no-rust) WITH_RUST=0; shift ;;
    --no-bun) WITH_BUN=0; shift ;;
    --no-secrets) WITH_SECRETS=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: opción desconocida: $1" >&2; usage >&2; exit 1 ;;
  esac
done

have() { command -v "$1" >/dev/null 2>&1; }
log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mAdvertencia:\033[0m %s\n' "$*" >&2; }

if [ "$(uname)" != "Darwin" ]; then
  echo "error: este script es solo para macOS." >&2
  exit 1
fi

# --- Xcode CLT -----------------------------------------------------------------
if xcode-select -p >/dev/null 2>&1; then
  log "Xcode Command Line Tools ya instaladas."
else
  log "Instalando Xcode Command Line Tools (puede tardar)..."
  xcode-select --install || true
  echo "Completá la instalación en el diálogo de Apple y volvé a correr este script."
  exit 1
fi

# --- Homebrew ------------------------------------------------------------------
if have brew; then
  log "Homebrew ya instalado."
else
  log "Instalando Homebrew..."
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
fi
if [ -x /opt/homebrew/bin/brew ]; then
  eval "$(/opt/homebrew/bin/brew shellenv)"
elif [ -x /usr/local/bin/brew ]; then
  eval "$(/usr/local/bin/brew shellenv)"
fi

# --- Paquetes brew --------------------------------------------------------------
log "Instalando paquetes base con Homebrew..."
brew_packages=(git)
[ "$WITH_POSTGRES" -eq 1 ] && brew_packages+=(postgresql@16)
[ "$WITH_BUN" -eq 1 ] && brew_packages+=(bun)
[ "$WITH_SECRETS" -eq 1 ] && brew_packages+=(sops age)
brew install "${brew_packages[@]}"

# postgresql@16 es keg-only: agregarlo al PATH de esta sesión y futuras.
PG_PREFIX="$(brew --prefix postgresql@16 2>/dev/null || true)"
if [ -n "$PG_PREFIX" ] && [ -x "$PG_PREFIX/bin/psql" ]; then
  export PATH="$PG_PREFIX/bin:$PATH"
  for rc in "$HOME/.zprofile" "$HOME/.zshrc" "$HOME/.bash_profile"; do
    if [ -f "$rc" ] && ! grep -q "postgresql@16/bin" "$rc" 2>/dev/null; then
      printf '\nexport PATH="%s/bin:$PATH"\n' "$PG_PREFIX" >> "$rc"
    fi
  done
fi

if [ "$WITH_POSTGRES" -eq 1 ]; then
  if pg_isready -h 127.0.0.1 -p 5432 >/dev/null 2>&1; then
    log "PostgreSQL ya responde en 127.0.0.1:5432."
  else
    log "Iniciando PostgreSQL con brew services..."
    brew services start postgresql@16
    sleep 3
    pg_isready -h 127.0.0.1 -p 5432 \
      || warn "PostgreSQL aún no responde; esperá unos segundos y verificá con: pg_isready -h 127.0.0.1"
  fi
fi

# --- Rust -----------------------------------------------------------------------
if [ "$WITH_RUST" -eq 1 ]; then
  if have cargo && have rustc; then
    log "Rust ya instalado: $(rustc --version)"
  else
    log "Instalando Rust (rustup, toolchain estable)..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain stable --profile minimal
  fi
  # shellcheck disable=SC1091
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  export PATH="$HOME/.cargo/bin:$PATH"

  if have sqlx; then
    log "sqlx-cli ya instalado."
  else
    log "Instalando sqlx-cli (puede tardar varios minutos)..."
    cargo install sqlx-cli --no-default-features --features postgres,rustls
  fi
fi

# --- secretspec ------------------------------------------------------------------
if [ "$WITH_SECRETS" -eq 1 ]; then
  if have secretspec; then
    log "secretspec ya instalado."
  else
    log "Instalando secretspec (instalador oficial)..."
    if curl -sSL https://install.secretspec.dev | sh; then
      export PATH="$HOME/.local/bin:$PATH"
    else
      warn "falló el instalador oficial; intentando con cargo..."
      cargo install secretspec-cli || cargo install secretspec
    fi
  fi
fi

# --- Resumen ----------------------------------------------------------------------
echo
log "Verificación final:"
for cmd in git psql cargo rustc bun sqlx sops age-keygen secretspec; do
  if have "$cmd"; then
    printf '  [OK] %-12s %s\n' "$cmd" "$("$cmd" --version 2>/dev/null | head -n1)"
  else
    printf '  [--] %-12s no encontrado (opcional u omitido)\n' "$cmd"
  fi
done
echo
log "Listo. Siguiente paso: ./scripts/run.sh"
