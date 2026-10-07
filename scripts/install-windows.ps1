<#
.SYNOPSIS
  Instala todas las dependencias del proyecto en Windows y deja el entorno
  listo para scripts\run-windows.ps1.

.DESCRIPTION
  Instala vía winget (con respaldos manuales cuando el paquete no existe):
    - Git, Rust (rustup, toolchain estable), Bun
    - PostgreSQL 16 (servidor + cliente) e inicia el servicio
    - Herramientas de compilación MSVC (requeridas por aws-lc-rs/jsonwebtoken)
    - sqlx-cli (cargo)
    - age y SOPS (secretos)
    - secretspec (cargo, con instrucciones de respaldo)

  Ejecutar en PowerShell (normal o como Administrador; winget puede pedir
  elevación solo):
    powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1

.PARAMETER NoPostgres
  Omite PostgreSQL.
.PARAMETER NoRust
  Omite Rust (rustup) y herramientas cargo (sqlx-cli, secretspec).
.PARAMETER NoBun
  Omite Bun.
.PARAMETER NoSecrets
  Omite age, SOPS y secretspec.
.PARAMETER NoBuildTools
  Omite las Build Tools de Visual Studio (solo si ya tenés MSVC instalado).
#>
[CmdletBinding()]
param(
  [switch]$NoPostgres,
  [switch]$NoRust,
  [switch]$NoBun,
  [switch]$NoSecrets,
  [switch]$NoBuildTools
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Msg) Write-Host "==> $Msg" -ForegroundColor Blue }
function Write-WarnMsg { param([string]$Msg) Write-Host "Advertencia: $Msg" -ForegroundColor Yellow }

function Test-Command {
  param([string]$Name)
  return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Install-WingetPackage {
  param(
    [string[]]$Ids,
    [string]$DisplayName
  )
  foreach ($id in $Ids) {
    Write-Step "Instalando $DisplayName (winget: $id)..."
    try {
      winget install -e --id $id --accept-source-agreements `
        --accept-package-agreements --silent
      if ($LASTEXITCODE -eq 0) { return $true }
      Write-WarnMsg "winget devolvió código $LASTEXITCODE para $id."
    } catch {
      Write-WarnMsg "falló winget para $id : $($_.Exception.Message)"
    }
  }
  return $false
}

function Refresh-Path {
  # Recarga el PATH de la sesión desde máquina + usuario (sin reiniciar).
  $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  $user = [Environment]::GetEnvironmentVariable('Path', 'User')
  $env:Path = "$machine;$user"
}

# --- Requisitos previos ---------------------------------------------------------
if (-not (Test-Command 'winget')) {
  Write-Error 'winget no encontrado. Instalá "App Installer" desde Microsoft Store y reintentá.'
}

$principal = New-Object Security.Principal.WindowsPrincipal(
  [Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  Write-WarnMsg 'No sos Administrador: algunos instaladores pedirán elevación (UAC).'
}

# --- Git ------------------------------------------------------------------------
if (Test-Command 'git') {
  Write-Step "Git ya instalado: $(git --version)"
} else {
  Install-WingetPackage -Ids @('Git.Git') -DisplayName 'Git' | Out-Null
}
Refresh-Path

# --- Build Tools de Visual Studio (linker MSVC para Rust) -------------------------
if ($NoBuildTools) {
  Write-Step 'Se omiten las Build Tools (--NoBuildTools).'
} elseif (Test-Path "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe") {
  Write-Step 'Visual Studio / Build Tools ya instalados.'
} else {
  Write-Step 'Instalando Build Tools de Visual Studio 2022 (carga VCTools, tarda varios minutos)...'
  winget install -e --id Microsoft.VisualStudio.2022.BuildTools `
    --accept-source-agreements --accept-package-agreements --silent `
    --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  if ($LASTEXITCODE -ne 0) {
    Write-WarnMsg 'falló la instalación automática.'
    Write-WarnMsg 'Instalalas manualmente desde https://visualstudio.microsoft.com/downloads/ (carga "Desarrollo para el escritorio con C++").'
  }
}

# --- Rust -------------------------------------------------------------------------
if ($NoRust) {
  Write-Step 'Se omite Rust (--NoRust).'
} else {
  if (Test-Command 'cargo') {
    Write-Step "Rust ya instalado: $(rustc --version)"
  } else {
    Install-WingetPackage -Ids @('Rustlang.Rustup') -DisplayName 'Rust (rustup)' | Out-Null
    Refresh-Path
    if (Test-Command 'rustup') {
      Write-Step 'Instalando toolchain estable de Rust...'
      rustup toolchain install stable --profile minimal
      rustup default stable
    }
  }
  Refresh-Path
  if (Test-Command 'cargo') {
    if (Test-Command 'sqlx') {
      Write-Step 'sqlx-cli ya instalado.'
    } else {
      Write-Step 'Instalando sqlx-cli (puede tardar varios minutos)...'
      cargo install sqlx-cli --no-default-features --features postgres,rustls
      if ($LASTEXITCODE -ne 0) {
        Write-WarnMsg 'falló sqlx-cli; reintentá manualmente: cargo install sqlx-cli --no-default-features --features postgres,rustls'
      }
    }
  } else {
    Write-WarnMsg 'cargo no quedó en el PATH; cerrá y abrí la terminal, luego: cargo install sqlx-cli --no-default-features --features postgres,rustls'
  }
}

# --- Bun (build.rs usa `bunx @tailwindcss/cli`) ------------------------------------
if ($NoBun) {
  Write-Step 'Se omite Bun (--NoBun).'
} elseif ((Test-Command 'bun') -or (Test-Command 'bunx')) {
  Write-Step 'Bun ya instalado.'
} else {
  Install-WingetPackage -Ids @('Oven.Bun') -DisplayName 'Bun' | Out-Null
  Refresh-Path
}

# --- PostgreSQL 16 ------------------------------------------------------------------
if ($NoPostgres) {
  Write-Step 'Se omite PostgreSQL (--NoPostgres).'
} else {
  $pgReady = $false
  if (Test-Command 'pg_isready') {
    $pgReady = (& pg_isready -h 127.0.0.1 -p 5432 2>$null; $LASTEXITCODE -eq 0)
  }
  if ($pgReady) {
    Write-Step 'PostgreSQL ya responde en 127.0.0.1:5432.'
  } else {
    Install-WingetPackage -Ids @('PostgreSQL.PostgreSQL.16', 'PostgreSQL.PostgreSQL') `
      -DisplayName 'PostgreSQL 16' | Out-Null
    Refresh-Path
    # El instalador EDB crea el servicio; intentamos iniciarlo.
    $svc = Get-Service -Name 'postgresql*' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($svc) {
      Write-Step "Iniciando servicio PostgreSQL ($($svc.Name))..."
      try { Start-Service $svc -ErrorAction Stop } catch { Write-WarnMsg "$($_.Exception.Message)" }
    } else {
      Write-WarnMsg 'No se encontró el servicio postgresql. Abrí "Services" y arrancalo manualmente.'
    }
    Write-Host ''
    Write-Host 'NOTA: el instalador de PostgreSQL pide una contraseña para el superusuario' -ForegroundColor Cyan
    Write-Host '"postgres". Recordala: la vas a necesitar en scripts\run-windows.ps1' -ForegroundColor Cyan
    Write-Host '(variable de entorno PGPASSWORD) para crear el rol/base de desarrollo.' -ForegroundColor Cyan
  }
}

# --- Secretos: age + SOPS + secretspec -----------------------------------------------
if ($NoSecrets) {
  Write-Step 'Se omiten los secretos (--NoSecrets).'
} else {
  if (Test-Command 'age-keygen') {
    Write-Step 'age ya instalado.'
  } else {
    $ok = Install-WingetPackage -Ids @('FiloSottile.Age') -DisplayName 'age'
    if (-not $ok) {
      Write-WarnMsg 'Instalá age manualmente desde https://github.com/FiloSottile/age/releases'
    }
  }
  if (Test-Command 'sops') {
    Write-Step 'SOPS ya instalado.'
  } else {
    $ok = Install-WingetPackage -Ids @('Mozilla.SOPS', 'SOPS.SOPS') -DisplayName 'SOPS'
    if (-not $ok) {
      Write-WarnMsg 'Instalá SOPS manualmente desde https://github.com/getsops/sops/releases'
    }
  }
  Refresh-Path
  if (Test-Command 'secretspec') {
    Write-Step 'secretspec ya instalado.'
  } elseif (Test-Command 'cargo') {
    Write-Step 'Instalando secretspec con cargo...'
    cargo install secretspec-cli
    if ($LASTEXITCODE -ne 0) { cargo install secretspec }
    if ($LASTEXITCODE -ne 0) {
      Write-WarnMsg 'Instalá secretspec manualmente desde https://github.com/cachix/secretspec/releases'
    }
  } else {
    Write-WarnMsg 'Instalá secretspec manualmente desde https://github.com/cachix/secretspec/releases'
  }
  Refresh-Path
}

# --- Resumen ---------------------------------------------------------------------------
Write-Host ''
Write-Step 'Verificación final:'
foreach ($cmd in @('git', 'cargo', 'rustc', 'bun', 'psql', 'sqlx', 'sops', 'age-keygen', 'secretspec')) {
  if (Test-Command $cmd) {
    try { $ver = (& $cmd --version 2>$null | Select-Object -First 1) } catch { $ver = '' }
    Write-Host "  [OK] $cmd $ver"
  } else {
    Write-Host "  [--] $cmd no encontrado (opcional u omitido)"
  }
}
Write-Host ''
Write-Step 'Listo. Cerrá y abrí la terminal (para actualizar el PATH) y seguí con: .\scripts\run-windows.ps1'
