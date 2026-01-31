# Monitor Crypto Signals in Real-Time (PowerShell)
# Watches for arbitrage detection, signal generation, and execution

function Show-Header {
    param([string]$Text)
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host $Text -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan
}

function Show-Success {
    param([string]$Text)
    Write-Host "✓ $Text" -ForegroundColor Green
}

function Show-Warning {
    param([string]$Text)
    Write-Host "⚠ $Text" -ForegroundColor Yellow
}

function Show-Error {
    param([string]$Text)
    Write-Host "✗ $Text" -ForegroundColor Red
}

function Show-Activity {
    Write-Host ""
    Show-Header "Recent Activity (Last 20 lines)"
    Write-Host ""

    $logs = docker compose logs --tail=20 crypto_shard execution_service crypto-spot-monitor 2>&1

    $filtered = $logs | Select-String "(arbitrage|signal|ExecutionRequest|price|connected|error)" -ErrorAction SilentlyContinue

    if ($filtered) {
        Write-Host $filtered
    } else {
        Write-Host "No matching activity yet..." -ForegroundColor Yellow
    }

    Write-Host ""
}

function Show-Metrics {
    Write-Host ""
    Show-Header "Service Metrics"
    Write-Host ""

    Write-Host "crypto_shard:" -ForegroundColor Cyan
    $logs = docker compose logs crypto_shard 2>&1
    $prices = ($logs | Select-String "price" -AllMatches | Measure-Object -Line).Lines
    $signals = ($logs | Select-String "arbitrage|signal" -AllMatches | Measure-Object -Line).Lines
    $errors = ($logs | Select-String "error|Error" -AllMatches | Measure-Object -Line).Lines

    Write-Host "  Prices processed: $prices"
    Write-Host "  Signals generated: $signals"
    Write-Host "  Errors: $errors"

    Write-Host ""
    Write-Host "execution_service:" -ForegroundColor Cyan
    $logs = docker compose logs execution_service 2>&1
    $exec_requests = ($logs | Select-String "ExecutionRequest|ZMQ signal" -AllMatches | Measure-Object -Line).Lines
    $executed = ($logs | Select-String "Executing|Executed" -AllMatches | Measure-Object -Line).Lines
    $exec_errors = ($logs | Select-String "error|Error|rejection" -AllMatches | Measure-Object -Line).Lines

    Write-Host "  Execution requests: $exec_requests"
    Write-Host "  Trades executed: $executed"
    Write-Host "  Errors: $exec_errors"

    Write-Host ""
    Write-Host "crypto-spot-monitor:" -ForegroundColor Cyan
    $logs = docker compose logs crypto-spot-monitor 2>&1
    $spot_prices = ($logs | Select-String "Published.*spot prices" -AllMatches | Measure-Object -Line).Lines
    $ws_errors = ($logs | Select-String "error|Error" -AllMatches | Measure-Object -Line).Lines

    Write-Host "  Spot prices published: $spot_prices"
    Write-Host "  WebSocket errors: $ws_errors"

    Write-Host ""
}

function Watch-Signals {
    Write-Host ""
    Show-Header "Watching for Arbitrage Signals"
    Write-Host "(Press Ctrl+C to stop)" -ForegroundColor Yellow
    Write-Host ""

    docker compose logs -f crypto_shard execution_service 2>&1 | Select-String "(arbitrage|ExecutionRequest|signal)"
}

function Check-Health {
    Write-Host ""
    Show-Header "Service Health Check"
    Write-Host ""

    $services = @("crypto_shard", "execution_service", "crypto-spot-monitor", "kalshi_monitor", "polymarket_monitor")

    foreach ($service in $services) {
        $status = docker compose ps $service 2>&1 | Select-String -Pattern "(Up|Exit)" | ForEach-Object { $_.Line.Split()[-1] }

        if ($status -like "*Up*") {
            Show-Success "$service is running"
        } else {
            Show-Error "$service is not running"
        }
    }

    Write-Host ""
    Write-Host "Network connectivity:" -ForegroundColor Cyan

    try {
        $test = docker compose exec -T crypto_shard nc -z localhost 5555 2>&1
        Show-Success "crypto_shard can reach kalshi_monitor:5555"
    } catch {
        Show-Warning "crypto_shard cannot reach kalshi_monitor:5555"
    }

    Write-Host ""
}

function Restart-Services {
    Write-Host ""
    Show-Header "Restarting Services"

    docker compose restart crypto_shard execution_service crypto-spot-monitor

    Write-Host ""
    Show-Success "Services restarted"
    Start-Sleep -Seconds 5
}

function Show-Menu {
    Write-Host ""
    Write-Host "Options:" -ForegroundColor Cyan
    Write-Host "  1) Show recent activity"
    Write-Host "  2) Show metrics"
    Write-Host "  3) Watch signals (live)"
    Write-Host "  4) Show all service logs (live)"
    Write-Host "  5) Check service health"
    Write-Host "  6) Restart services"
    Write-Host "  7) Export logs to file"
    Write-Host "  q) Quit"
    Write-Host ""
}

function Export-Logs {
    $filename = "crypto-shard-logs-$(Get-Date -Format 'yyyy-MM-dd_HHmmss').log"
    Write-Host "Exporting logs to $filename..." -ForegroundColor Cyan

    docker compose logs crypto_shard execution_service crypto-spot-monitor > $filename 2>&1

    Show-Success "Logs exported to $filename"
    Write-Host ""
}

# Check if run mode requested from command line
if ($args[0] -eq "watch") {
    Watch-Signals
    exit
} elseif ($args[0] -eq "metrics") {
    Show-Metrics
    exit
} elseif ($args[0] -eq "activity") {
    Show-Activity
    exit
}

# Interactive mode
while ($true) {
    Show-Menu
    $choice = Read-Host "Choose option"

    switch ($choice) {
        "1" { Show-Activity }
        "2" { Show-Metrics }
        "3" { Watch-Signals }
        "4" { docker compose logs -f crypto_shard execution_service crypto-spot-monitor }
        "5" { Check-Health }
        "6" { Restart-Services }
        "7" { Export-Logs }
        "q" { Write-Host "Exiting..."; exit }
        default { Show-Error "Invalid option" }
    }
}
