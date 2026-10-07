<#
.SYNOPSIS
  Prepara la base de datos y ejecuta el servidor `lab` en Windows.

.DESCRIPTION
  Pasos:
    1. Verifica dependencias (cargo, psql, bun/bunx para Tailwind en build.rs).
    2. Verifica que PostgreSQL responda en 127.0.0.1:5432 (intenta iniciar el servicio).
    3. Crea el rol/base de desarrollo si faltan (por defecto lab/lab).
    4. Aplica lab\database\schema.sql solo si la BD está vacía (no es
       idempotente) y siempre lab\database\seed.sql (idempotente).
    5. Ejecuta el servidor en lab\ con `secretspec run` si hay clave age +
       secretspec; si no, con variables de desarrollo por defecto.

  El servidor escucha en http://127.0.0.1:8080

  Variables opcionales: LAB_DB_USER, LAB_DB_PASS, LAB_DB_DATABASE,
  LAB_DB_HOST, LAB_DB_PORT, SOPS_AGE_KEY_FILE, PG_SUPER_PASSWORD (clave del
  superusuario postgres para crear el rol, si hace falta; si no, se pide).

  Ejecutar desde la raíz del repo:
    powershell -ExecutionPolicy Bypass -File scripts\run-windows.ps1

.PARAMETER Release
  Compila/ejecuta en modo release.
.PARAMETER SkipDb
  Omite la preparación de la base de datos.
.PARAMETER SkipSeed
  No aplica seed.sql (solo esquema si la BD está vacía).
.PARAMETER Profile
  Perfil de secretspec (por defecto: development).
#>
[CmdletBinding()]
param(
  [switch]$Release,
  [switch]$SkipDb,
  [switch]$SkipSeed,
  [string]$Profile = $(if ($env:SECRETSPEC_PROFILE) { $env:SECRETSPEC_PROFILE } else { 'development' })
)

$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$Msg) Write-Host "==> $Msg" -ForegroundColor Blue }
function Write-WarnMsg { param([string]$Msg) Write-Host "Advertencia: $Msg" -ForegroundColor Yellow }
function Write-ErrMsg { param([string]$Msg) Write-Host "error: $Msg" -ForegroundColor Red; exit 1 }

function Test-Command {
  param([string]$Name)
  return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

# Valores de desarrollo (coinciden con devenv.nix y secrets.enc.yaml).
$DbUser = if ($env:LAB_DB_USER) { $env:LAB_DB_USER } else { 'lab' }
$DbPass = if ($env:LAB_DB_PASS) { $env:LAB_DB_PASS } else { 'lab' }
$DbName = if ($env:LAB_DB_DATABASE) { $env:LAB_DB_DATABASE } else { 'lab' }
$DbHost = if ($env:LAB_DB_HOST) { $env:LAB_DB_HOST } else { '127.0.0.1' }
$DbPort = if ($env:LAB_DB_PORT) { $env:LAB_DB_PORT } else { '5432' }

# --- 1. Dependencias ---------------------------------------------------------------
$missing = $false
foreach ($cmd in @('cargo', 'psql')) {
  if (-not (Test-Command $cmd)) {
    Write-WarnMsg "falta '$cmd' (corré scripts\install-windows.ps1)."
    $missing = $true
  }
}
if (-not (Test-Command 'bun') -and -not (Test-Command 'bunx')) {
  Write-WarnMsg "falta 'bun'/'bunx' (build.rs compila Tailwind con bunx; instalalo o el build fallará)."
}
if ($missing) { Write-ErrMsg 'faltan dependencias.' }

# --- 2. PostgreSQL en marcha ----------------------------------------------------------
function Test-PgReady {
  $p = Start-Process -FilePath 'pg_isready' -ArgumentList "-h $DbHost -p $DbPort" `
    -NoNewWindow -Wait -PassThru
  return ($p.ExitCode -eq 0)
}

if (-not $SkipDb) {
  if (-not (Test-PgReady)) {
    Write-Step 'PostgreSQL no responde; intentando iniciar el servicio...'
    $svc = Get-Service -Name 'postgresql*' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($svc) {
      try { Start-Service $svc -ErrorAction Stop } catch { Write-WarnMsg "$($_.Exception.Message)" }
      Start-Sleep -Seconds 3
    } else {
      Write-WarnMsg 'No se encontró el servicio postgresql.'
    }
  }
  if (-not (Test-PgReady)) {
    Write-ErrMsg "PostgreSQL no responde en ${DbHost}:${DbPort}. Inícialo manualmente."
  }
  Write-Step "PostgreSQL responde en ${DbHost}:${DbPort}."
}

# --- 3-4. Rol, base, esquema y seed -----------------------------------------------------
function Invoke-PsqlSuper {
  param([string]$Sql)
  # Intenta como superusuario postgres por TCP usando $env:PGPASSWORD.
  if (-not $env:PGPASSWORD) { return $null }
  $out = & psql -h $DbHost -p $DbPort -U postgres -v ON_ERROR_STOP=1 -tA -c $Sql postgres 2>$null
  if ($LASTEXITCODE -eq 0) { return $out } else { return $null }
}

if (-not $SkipDb) {
  $savedPgPass = $env:PGPASSWORD
  $env:PGPASSWORD = $DbPass
  $roleOk = (& psql -h $DbHost -p $DbPort -U $DbUser -d postgres -tA -c 'SELECT 1' 2>$null) -eq '1'

  if ($roleOk) {
    Write-Step "Rol '$DbUser' ya accesible."
  } else {
    Write-Step "Creando rol '$DbUser' (superusuario local, solo desarrollo)..."
    # La contraseña del superusuario "postgres" puede venir en PG_SUPER_PASSWORD
    # (no interactivo); si no, se pide una vez por consola.
    $superPass = if ($env:PG_SUPER_PASSWORD) { $env:PG_SUPER_PASSWORD } else {
      $sec = Read-Host 'Contraseña del superusuario "postgres" (para crear el rol lab)' -AsSecureString
      [Runtime.InteropServices.Marshal]::PtrToStringAuto(
        [Runtime.InteropServices.Marshal]::SecureStringToBSTR($sec))
    }
    $escPass = $DbPass -replace "'", "''"
    $env:PGPASSWORD = $superPass
    $exists = Invoke-PsqlSuper "SELECT 1 FROM pg_roles WHERE rolname = '$DbUser'"
    if ($exists -eq '1') {
      Invoke-PsqlSuper "ALTER ROLE `"$DbUser`" WITH LOGIN SUPERUSER PASSWORD '$escPass'" | Out-Null
    } else {
      $r = Invoke-PsqlSuper "CREATE ROLE `"$DbUser`" WITH LOGIN SUPERUSER PASSWORD '$escPass'"
      if ($null -eq $r) {
        Write-ErrMsg 'no se pudo crear el rol. Verificá la contraseña del superusuario postgres (variable PG_SUPER_PASSWORD).'
      }
    }
    $env:PGPASSWORD = $DbPass
  }

  $dbOk = (& psql -h $DbHost -p $DbPort -U $DbUser -d $DbName -tA -c 'SELECT 1' 2>$null) -eq '1'
  if ($dbOk) {
    Write-Step "Base '$DbName' ya existe."
  } else {
    Write-Step "Creando base '$DbName' (dueño: $DbUser)..."
    # createdb con la contraseña del rol ya exportada en PGPASSWORD.
    & createdb -h $DbHost -p $DbPort -U $DbUser -O $DbUser $DbName
    if ($LASTEXITCODE -ne 0) { Write-ErrMsg 'no se pudo crear la base.' }
  }

  # Esquema solo si la BD está vacía (schema.sql usa CREATE TABLE sin IF NOT EXISTS).
  $hasSchema = & psql -h $DbHost -p $DbPort -U $DbUser -d $DbName `
    -tA -c "SELECT to_regclass('public.partners')" 2>$null
  if ($hasSchema -eq 'partners') {
    Write-Step 'Esquema ya aplicado (tabla partners existe).'
  } else {
    Write-Step 'Aplicando lab\database\schema.sql...'
    & psql -h $DbHost -p $DbPort -U $DbUser -d $DbName `
      -v ON_ERROR_STOP=1 -f lab\database\schema.sql
    if ($LASTEXITCODE -ne 0) { Write-ErrMsg 'falló schema.sql.' }
  }

  if (-not $SkipSeed) {
    Write-Step 'Aplicando lab\database\seed.sql (idempotente)...'
    & psql -h $DbHost -p $DbPort -U $DbUser -d $DbName `
      -v ON_ERROR_STOP=1 -f lab\database\seed.sql
    if ($LASTEXITCODE -ne 0) { Write-ErrMsg 'falló seed.sql.' }
  }
  if ($null -eq $savedPgPass) { Remove-Item Env:\PGPASSWORD -ErrorAction SilentlyContinue }
  else { $env:PGPASSWORD = $savedPgPass }
}

# --- 5. Servidor --------------------------------------------------------------------------
Set-Location (Join-Path $Root 'lab')

$keyCandidates = @(
  $env:SOPS_AGE_KEY_FILE,
  (Join-Path $HOME '.config\sops\age\dev-key.txt')
) | Where-Object { $_ }
$keyFile = $keyCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1

if ((Test-Command 'secretspec') -and $keyFile) {
  $env:SOPS_AGE_KEY_FILE = $keyFile
  Write-Step "Ejecutando con secretos (perfil '$Profile', clave: $keyFile)..."
  if ($Release) {
    secretspec run --profile $Profile -- cargo run --release
  } else {
    secretspec run --profile $Profile -- cargo run
  }
  exit $LASTEXITCODE
} else {
  if (Test-Command 'secretspec') {
    Write-WarnMsg 'hay secretspec pero no se encontró la clave age (~\.config\sops\age\dev-key.txt).'
    Write-WarnMsg 'Pedí la clave al equipo; mientras tanto uso valores de desarrollo.'
  } else {
    Write-WarnMsg 'sin secretspec: uso variables de desarrollo por defecto (ver scripts\secrets-decode.ps1).'
  }
}

$env:LAB_DB_URL = "postgres://${DbUser}:${DbPass}@${DbHost}:${DbPort}/${DbName}"
if (-not $env:LAB_JWT_SECRET) { $env:LAB_JWT_SECRET = 'dev-only-insecure-change-me' }
if (-not $env:LAB_OTP_SECRET) { $env:LAB_OTP_SECRET = $env:LAB_JWT_SECRET }
if (-not $env:LAB_OTP_SENDER) { $env:LAB_OTP_SENDER = 'log' }
Write-Step "LAB_DB_URL=$($env:LAB_DB_URL) | OTP_SENDER=$($env:LAB_OTP_SENDER)"
Write-Step 'Servidor en http://127.0.0.1:8080 (Ctrl+C para detener)...'
if ($Release) { cargo run --release } else { cargo run }
