# Generate a development TLS cert that includes localhost and LAN IPs.
# Browsers only expose getUserMedia on HTTPS or localhost.

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $root) { $root = Get-Location }
$ssl = Join-Path $root "apps/airlock/ssl"
New-Item -ItemType Directory -Force -Path $ssl | Out-Null

$openssl = $null
foreach ($candidate in @(
        "openssl",
        "$env:ProgramFiles\Git\usr\bin\openssl.exe",
        "${env:ProgramFiles(x86)}\Git\usr\bin\openssl.exe"
    )) {
    if ($candidate -eq "openssl") {
        $cmd = Get-Command openssl -ErrorAction SilentlyContinue
        if ($cmd) { $openssl = $cmd.Source; break }
    }
    elseif (Test-Path $candidate) {
        $openssl = $candidate
        break
    }
}

if (-not $openssl) {
    throw "openssl not found. Install Git for Windows (includes openssl) or mkcert."
}

$ips = @(
    "127.0.0.1"
)
Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Where-Object { $_.IPAddress -and $_.IPAddress -notlike "127.*" } |
    ForEach-Object { $ips += $_.IPAddress }
$ips = $ips | Select-Object -Unique

$alt = @("DNS.1 = localhost", "DNS.2 = *.localhost", "IP.1 = ::1")
$i = 2
foreach ($ip in $ips) {
    $alt += "IP.$i = $ip"
    $i++
}

$cnf = @"
[req]
distinguished_name = req_distinguished_name
x509_extensions = v3_req
prompt = no

[req_distinguished_name]
CN = airlock-dev

[v3_req]
subjectAltName = @alt_names
basicConstraints = CA:FALSE
keyUsage = digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth

[alt_names]
$($alt -join "`n")
"@

$cnfPath = Join-Path $ssl "san.cnf"
Set-Content -Path $cnfPath -Value $cnf -Encoding ascii

$key = Join-Path $ssl "key.pem"
$cert = Join-Path $ssl "cert.pem"

& $openssl req -x509 -newkey rsa:2048 -sha256 -days 825 -nodes `
    -keyout $key -out $cert -config $cnfPath
if ($LASTEXITCODE -ne 0) {
    throw "openssl failed with exit code $LASTEXITCODE"
}

Write-Host "Wrote $cert"
Write-Host "Names: localhost, $($ips -join ', ')"
Write-Host "Open https://<lan-ip>:<port> and accept the certificate warning."
