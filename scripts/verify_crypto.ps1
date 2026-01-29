# Crypto Support Verification Script
# This script verifies that crypto support is working correctly

Write-Host "=== Crypto Support Verification ===" -ForegroundColor Cyan

# 1. Test CoinGecko API
Write-Host "`n1. Testing CoinGecko API..." -ForegroundColor Yellow
try {
    $btcPrice = Invoke-RestMethod -Uri "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
    Write-Host "   BTC Price: `$$($btcPrice.bitcoin.usd)" -ForegroundColor Green
} catch {
    Write-Host "   Failed to fetch CoinGecko data: $_" -ForegroundColor Red
}

# 2. Test Polymarket crypto markets
Write-Host "`n2. Testing Polymarket crypto market discovery..." -ForegroundColor Yellow
try {
    $markets = Invoke-RestMethod -Uri "https://gamma-api.polymarket.com/markets?closed=false&limit=50"
    $cryptoMarkets = $markets | Where-Object {
        $_.question -match "bitcoin|btc|crypto|ethereum|eth|solana|sol"
    }
    Write-Host "   Found $($cryptoMarkets.Count) potential crypto markets" -ForegroundColor Green
    if ($cryptoMarkets.Count -gt 0) {
        Write-Host "   Sample markets:" -ForegroundColor Gray
        $cryptoMarkets | Select-Object -First 3 | ForEach-Object {
            Write-Host "     - $($_.question.Substring(0, [Math]::Min(60, $_.question.Length)))..." -ForegroundColor Gray
        }
    }
} catch {
    Write-Host "   Failed to fetch Polymarket data: $_" -ForegroundColor Red
}

# 3. Check if crypto is enabled in config
Write-Host "`n3. Checking configuration..." -ForegroundColor Yellow
$envFile = Join-Path $PSScriptRoot "../.env"
if (Test-Path $envFile) {
    $envContent = Get-Content $envFile -Raw
    if ($envContent -match "ENABLE_CRYPTO_MARKETS=true") {
        Write-Host "   ENABLE_CRYPTO_MARKETS=true" -ForegroundColor Green
    } elseif ($envContent -match "ENABLE_CRYPTO_MARKETS=false") {
        Write-Host "   ENABLE_CRYPTO_MARKETS=false (disabled)" -ForegroundColor Yellow
    } else {
        Write-Host "   ENABLE_CRYPTO_MARKETS not set in .env" -ForegroundColor Yellow
    }
} else {
    Write-Host "   .env file not found" -ForegroundColor Yellow
}

# 4. Check Rust build status
Write-Host "`n4. Checking Rust build..." -ForegroundColor Yellow
$servicesPath = Join-Path $PSScriptRoot "../services"
Push-Location $servicesPath
try {
    $result = cargo check --package arbees_rust_core 2>&1
    if ($LASTEXITCODE -eq 0) {
        Write-Host "   arbees_rust_core: OK" -ForegroundColor Green
    } else {
        Write-Host "   arbees_rust_core: FAILED" -ForegroundColor Red
        Write-Host $result -ForegroundColor Red
    }
} catch {
    Write-Host "   Could not run cargo check: $_" -ForegroundColor Red
}
Pop-Location

# 5. Test CoinGecko volatility calculation capability
Write-Host "`n5. Testing CoinGecko volatility data..." -ForegroundColor Yellow
try {
    $chartData = Invoke-RestMethod -Uri "https://api.coingecko.com/api/v3/coins/bitcoin/market_chart?vs_currency=usd&days=7"
    $priceCount = $chartData.prices.Count
    Write-Host "   Fetched $priceCount price points for volatility calculation" -ForegroundColor Green
} catch {
    Write-Host "   Failed to fetch market chart: $_" -ForegroundColor Red
}

Write-Host "`n=== Verification Complete ===" -ForegroundColor Cyan
Write-Host "`nTo enable crypto markets, add to .env:" -ForegroundColor Gray
Write-Host "   ENABLE_CRYPTO_MARKETS=true" -ForegroundColor White
