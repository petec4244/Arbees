# Crypto Shard Deployment & Testing Script (PowerShell)
# Deploys all crypto services and monitors for errors

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "Crypto Shard Deployment & Testing" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host ""

function Log-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message" -ForegroundColor Green
}

function Log-Warn {
    param([string]$Message)
    Write-Host "[WARN] $Message" -ForegroundColor Yellow
}

function Log-Error {
    param([string]$Message)
    Write-Host "[ERROR] $Message" -ForegroundColor Red
}

function Test-Port {
    param([int]$Port, [int]$Timeout = 1)
    try {
        $socket = New-Object System.Net.Sockets.TcpClient
        $result = $socket.BeginConnect("localhost", $Port, $null, $null)
        $result.AsyncWaitHandle.WaitOne($Timeout * 1000) | Out-Null
        if ($socket.Connected) {
            $socket.Close()
            return $true
        }
        return $false
    }
    catch {
        return $false
    }
}

function Wait-ForService {
    param([string]$ServiceName, [int]$Port, [int]$MaxAttempts = 5)

    Log-Info "Checking if $ServiceName is running on port $Port..."

    $attempt = 1
    while ($attempt -le $MaxAttempts) {
        if (Test-Port -Port $Port) {
            Log-Info "$ServiceName is running on port $Port"
            return $true
        }
        Write-Host "  Attempt $attempt/$MaxAttempts... waiting 5s"
        Start-Sleep -Seconds 5
        $attempt++
    }

    Log-Error "Failed to connect to $ServiceName on port $Port"
    return $false
}

# Step 1: Start infrastructure
Log-Info "Step 1: Starting infrastructure (TimescaleDB, Redis)..."
docker compose up -d timescaledb redis
Log-Info "Waiting for infrastructure to be healthy..."
Start-Sleep -Seconds 10

# Step 2: Start price monitors
Log-Info "Step 2: Starting price monitors (Kalshi, Polymarket, Spot)..."
docker compose up -d kalshi_monitor polymarket_monitor crypto-spot-monitor
Log-Info "Waiting for monitors to connect..."
Start-Sleep -Seconds 5

# Step 3: Start crypto_shard
Log-Info "Step 3: Starting crypto_shard_rust..."
docker compose up -d crypto_shard
Log-Info "Waiting for crypto_shard to initialize..."
Wait-ForService -ServiceName "crypto_shard" -Port 5559

# Step 4: Start execution service
Log-Info "Step 4: Starting execution_service_rust..."
docker compose up -d execution_service
Log-Info "Waiting for execution_service to initialize..."
Wait-ForService -ServiceName "execution_service" -Port 5560

Write-Host ""
Log-Info "========== DEPLOYMENT COMPLETE =========="
Write-Host ""

# Step 5: Display service status
Log-Info "Step 5: Checking service status..."
Write-Host ""

$services = @("crypto_shard", "execution_service", "kalshi_monitor", "polymarket_monitor", "crypto-spot-monitor")
foreach ($service in $services) {
    $status = docker compose ps $service | Select-String "Up"
    if ($status) {
        Log-Info "$service: Running"
    } else {
        Log-Warn "$service: Not running"
    }
}

Write-Host ""
Log-Info "========== DEPLOYMENT SUMMARY =========="
Write-Host ""

Write-Host "Service endpoints:"
Write-Host "  - crypto_shard: tcp://localhost:5559" -ForegroundColor Cyan
Write-Host "  - execution_service: tcp://localhost:5560" -ForegroundColor Cyan
Write-Host "  - kalshi_monitor: tcp://localhost:5555" -ForegroundColor Cyan
Write-Host "  - polymarket_monitor: tcp://localhost:5556" -ForegroundColor Cyan
Write-Host "  - crypto_spot_monitor: tcp://localhost:5560" -ForegroundColor Cyan
Write-Host ""

Log-Info "To view logs:"
Write-Host "  docker compose logs -f crypto_shard"
Write-Host "  docker compose logs -f execution_service"
Write-Host "  docker compose logs -f crypto-spot-monitor"
Write-Host ""

Log-Info "To check for arbitrage signals:"
Write-Host "  docker compose logs crypto_shard | Select-String 'arbitrage detected'"
Write-Host ""

Log-Info "To stop all services:"
Write-Host "  docker compose down"
Write-Host ""

Log-Info "Real-time monitoring:"
Write-Host "  $PSScriptRoot/follow_logs.ps1 -c crypto"
Write-Host ""
