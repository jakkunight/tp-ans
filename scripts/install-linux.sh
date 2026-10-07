#!/usr/bin/env bash
# install-linux.sh — instala todas las dependencias del proyecto en Linux
# y deja el entorno listo para `scripts/run.sh`.
#
# Soporta: Debian/Ubuntu (apt), Fedora/RHEL/Rocky/Alma (dnf),
#           Arch/Manjaro (pacman).
#
# Instala:
#   - Herramientas base: git, curl, ca-certificates, unzip, pkg-config,
#     compilador C, cmake, perl (requeridos por aws-lc-rs / jsonwebtoken)
#   - PostgreSQL 16 (servidor + cliente) y lo habilita al arranque
#   - Rust estable vía rustup
#   - Bun (requerido por build.rs: `bunx @tailwindcss/cli`)
#   - sqlx-cli (cargo)
#   - sops + age (secretos)
#   - secretspec (instalador oficial, con respaldo vía cargo)
#
# Uso:
#   ./scripts/install-linux.sh [--yes] [--no-postgres] [--no-rust]
#                             [--no-bun] [--no-secrets]
set -euo pipefail

YES=0
WITH_POSTGRES=1
WITH_RUST=1
WITH_BUN=1
WITH_SECRETS=1
# Versión de respaldo si el gestor de paquetes no trae sops/age (solo .deb).
SOPS_VERSION="${SOPS_VERSION:-3.10.2}"
AGE_VERSION="${AGE_VERSION:-1.2.1}"

usage() {
  cat <<'EOF'
Uso: install-linux.sh [opciones]

Opciones:
  --yes            No preguntar confirmación antes de instalar con sudo.
  --no-postgres    Omitir PostgreSQL.
  --no-rust        Omitir Rust (rustup) y herramientas cargo (sqlx-cli...).
  --no-bun         Omitir Bun.
  --no-secrets     Omitir sops, age y secretspec.
  -h, --help       Mostrar esta ayuda.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --yes) YES=1; shift ;;
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

# --- Detección de distro ------------------------------------------------------
if [ -f /etc/os-release ]; then
  # shellcheck disable=SC1091
  . /etc/os-release
else
  echo "error: no se pudo detectar la distribución (/etc/os-release ausente)." >&2
  exit 1
fi
DISTRO="${ID:-unknown} ${ID_LIKE:-}"
PKG=""
case "$DISTRO" in
  *debian*|*ubuntu*) PKG="apt" ;;
  *fedora*|*rhel*|*centos*|*rocky*|*alma*) PKG="dnf" ;;
  *arch*|*manjaro*) PKG="pacman" ;;
  *) echo "error: distro no soportada: ${ID:-?} (usa apt, dnf o pacman)." >&2; exit 1 ;;
esac
log "Distro detectada: ${ID:-desconocida} (gestor: $PKG)"

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
  if ! have sudo; then
    echo "error: se necesita root o sudo para instalar paquetes." >&2
    exit 1
  fi
fi

if [ "$YES" -ne 1 ]; then
  printf 'Se instalarán paquetes del sistema con %s %s. ¿Continuar? [S/n] ' "$SUDO" "$PKG"
  read -r ans || ans="S"
  case "$ans" in
    [nN]*) echo "Cancelado."; exit 0 ;;
  esac
fi

# --- Paquetes base ------------------------------------------------------------
log "Instalando herramientas base..."
case "$PKG" in
  apt)
    $SUDO apt-get update
    # shellcheck disable=SC2086
    $SUDO apt-get install -y git curl ca-certificates unzip \
      build-essential pkg-config cmake perl
    ;;
  dnf)
    $SUDO dnf install -y git curl ca-certificates unzip \
      gcc gcc-c++ make pkg-config cmake perl
    ;;
  pacman)
    $SUDO pacman -Sy --needed --noconfirm git curl ca-certificates unzip \
      base-devel cmake perl pkgconf
    ;;
esac

# --- PostgreSQL ---------------------------------------------------------------
if [ "$WITH_POSTGRES" -eq 1 ]; then
  if have pg_isready && pg_isready -h 127.0.0.1 -p 5432 >/dev/null 2>&1; then
    log "PostgreSQL ya responde en 127.0.0.1:5432, se omite la instalación."
  else
    log "Instalando PostgreSQL..."
    case "$PKG" in
      apt)
        $SUDO apt-get install -y postgresql postgresql-client
        $SUDO systemctl enable --now postgresql || $SUDO service postgresql start
        ;;
      dnf)
        $SUDO dnf install -y postgresql-server postgresql
        if [ ! -f /var/lib/pgsql/data/postgresql.conf ]; then
          $SUDO postgresql-setup --initdb
        fi
        $SUDO systemctl enable --now postgresql
        ;;
      pacman)
        $SUDO pacman -S --needed --noconfirm postgresql
        if [ ! -f /var/lib/postgres/data/postgresql.conf ]; then
          $SUDO -u postgres initdb -D /var/lib/postgres/data
        fi
        $SUDO systemctl enable --now postgresql
        ;;
    esac
  fi
fi

# --- Rust ---------------------------------------------------------------------
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

# --- Bun (build.rs usa `bunx @tailwindcss/cli`) --------------------------------
if [ "$WITH_BUN" -eq 1 ]; then
  if have bun || have bunx; then
    log "Bun ya instalado."
  else
    log "Instalando Bun..."
    curl -fsSL https://bun.sh/install | bash
    export BUN_INSTALL="${BUN_INSTALL:-$HOME/.bun}"
    export PATH="$BUN_INSTALL/bin:$PATH"
  fi
fi

# --- Secretos: sops + age + secretspec ----------------------------------------
install_sops_age_deb_fallback() {
  # Respaldo solo Debian/Ubuntu cuando apt no trae los paquetes.
  arch="$(uname -m)"
  case "$arch" in
    x86_64) deb_arch="amd64" ;;
    aarch64) deb_arch="arm64" ;;
    *) warn "arquitectura $arch sin respaldo .deb automático."; return 1 ;;
  esac
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN
  if ! have sops; then
    log "Descargando sops v${SOPS_VERSION} (.deb)..."
    curl -fsSL -o "$tmp/sops.deb" \
      "https://github.com/getsops/sops/releases/download/v${SOPS_VERSION}/sops_${SOPS_VERSION}_${deb_arch}.deb"
    $SUDO dpkg -i "$tmp/sops.deb" || $SUDO apt-get install -f -y
  fi
  if ! have age && ! have age-keygen; then
    log "Descargando age v${AGE_VERSION} (.deb)..."
    curl -fsSL -o "$tmp/age.deb" \
      "https://github.com/FiloSottile/age/releases/download/v${AGE_VERSION}/age-v${AGE_VERSION}-linux-${deb_arch}.deb" \
      || warn "no se pudo descargar age; instalalo manualmente desde https://github.com/FiloSottile/age/releases"
    [ -f "$tmp/age.deb" ] && $SUDO dpkg -i "$tmp/age.deb"
  fi
}

if [ "$WITH_SECRETS" -eq 1 ]; then
  log "Instalando sops y age..."
  case "$PKG" in
    apt)
      if $SUDO apt-get install -y sops age 2>/dev/null; then
        true
      else
        warn "apt no trae sops/age; usando respaldo .deb."
        install_sops_age_deb_fallback || true
      fi
      ;;
    dnf)
      $SUDO dnf install -y sops age || warn "dnf no trajo sops/age; instalalos manualmente."
      ;;
    pacman)
      $SUDO pacman -S --needed --noconfirm sops age
      ;;
  esac

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

# --- Resumen ------------------------------------------------------------------
echo
log "Verificación final:"
for cmd in git curl psql cargo rustc bun sqlx sops age-keygen secretspec; do
  if have "$cmd"; then
    printf '  [OK] %-12s %s\n' "$cmd" "$("$cmd" --version 2>/dev/null | head -n1)"
  else
    printf '  [--] %-12s no encontrado (opcional u omitido)\n' "$cmd"
  fi
done
echo
log "Listo. Siguiente paso: ./scripts/run.sh"
echo "  (Si instalaste Rust/Bun recién, abrí una terminal nueva o ejecutá:"
echo "   source \"\$HOME/.cargo/env\" y agregá \"\$HOME/.bun/bin\" al PATH.)"
