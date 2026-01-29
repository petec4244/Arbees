<#
.SYNOPSIS
    Kill switch control for Arbees execution service.

.DESCRIPTION
    Activates or deactivates the trading kill switch via Redis pub/sub.
    When activated, all order execution is halted immediately.

.PARAMETER Action
    The action to perform: enable, disable, or status

.EXAMPLE
    .\killswitch.ps1 enable
    Activates the kill switch (halts all trading)

.EXAMPLE
    .\killswitch.ps1 disable
    Deactivates the kill switch (resumes trading)

.EXAMPLE
    .\killswitch.ps1 status
    Shows current kill switch status
#>

param(
    [Parameter(Position=0)]
    [ValidateSet("enable", "disable", "status", "on", "off", "halt", "resume")]
    [string]$Action = "status"
)

$RedisHost = $env:REDIS_HOST
if (-not $RedisHost) { $RedisHost = "localhost" }

$RedisPort = $env:REDIS_PORT
if (-not $RedisPort) { $RedisPort = "6379" }

$KillSwitchChannel = "trading:kill_switch"
$KillSwitchFile = "/tmp/arbees_kill_switch"

function Send-RedisCommand {
    param([string]$Command)

    try {
        # Try using redis-cli if available
        $result = & redis-cli -h $RedisHost -p $RedisPort $Command.Split(" ") 2>&1
        if ($LASTEXITCODE -eq 0) {
            return $result
        }
    } catch {
        # redis-cli not found
    }

    # Fallback: Use TCP socket directly
    try {
        $tcp = New-Object System.Net.Sockets.TcpClient($RedisHost, [int]$RedisPort)
        $stream = $tcp.GetStream()
        $writer = New-Object System.IO.StreamWriter($stream)
        $reader = New-Object System.IO.StreamReader($stream)

        $writer.WriteLine($Command)
        $writer.Flush()

        Start-Sleep -Milliseconds 100
        $response = $reader.ReadLine()

        $tcp.Close()
        return $response
    } catch {
        Write-Error "Failed to connect to Redis at ${RedisHost}:${RedisPort}"
        Write-Error $_.Exception.Message
        return $null
    }
}

function Enable-KillSwitch {
    Write-Host ""
    Write-Host "============================================" -ForegroundColor Red
    Write-Host "  ACTIVATING KILL SWITCH - HALTING TRADING  " -ForegroundColor Red
    Write-Host "============================================" -ForegroundColor Red
    Write-Host ""

    # Send Redis command
    $result = & redis-cli -h $RedisHost -p $RedisPort PUBLISH $KillSwitchChannel "ENABLE" 2>&1

    if ($LASTEXITCODE -eq 0) {
        Write-Host "[OK] Kill switch ENABLED via Redis" -ForegroundColor Green
        Write-Host "     Subscribers notified: $result" -ForegroundColor Gray
    } else {
        Write-Host "[WARN] Redis publish failed, trying file-based fallback..." -ForegroundColor Yellow

        # Create file-based kill switch (works on WSL/Docker)
        try {
            # For Windows, create a local marker file
            $windowsPath = Join-Path $env:TEMP "arbees_kill_switch"
            New-Item -Path $windowsPath -ItemType File -Force | Out-Null
            Write-Host "[OK] File-based kill switch created at: $windowsPath" -ForegroundColor Green
        } catch {
            Write-Host "[ERROR] Failed to create kill switch file" -ForegroundColor Red
        }
    }

    Write-Host ""
    Write-Host "Trading is now HALTED. Use 'killswitch.ps1 disable' to resume." -ForegroundColor Yellow
}

function Disable-KillSwitch {
    Write-Host ""
    Write-Host "============================================" -ForegroundColor Green
    Write-Host "  DEACTIVATING KILL SWITCH - RESUMING TRADE " -ForegroundColor Green
    Write-Host "============================================" -ForegroundColor Green
    Write-Host ""

    # Send Redis command
    $result = & redis-cli -h $RedisHost -p $RedisPort PUBLISH $KillSwitchChannel "DISABLE" 2>&1

    if ($LASTEXITCODE -eq 0) {
        Write-Host "[OK] Kill switch DISABLED via Redis" -ForegroundColor Green
        Write-Host "     Subscribers notified: $result" -ForegroundColor Gray
    } else {
        Write-Host "[WARN] Redis publish may have failed" -ForegroundColor Yellow
    }

    # Remove file-based kill switch if exists
    $windowsPath = Join-Path $env:TEMP "arbees_kill_switch"
    if (Test-Path $windowsPath) {
        Remove-Item $windowsPath -Force
        Write-Host "[OK] File-based kill switch removed" -ForegroundColor Green
    }

    Write-Host ""
    Write-Host "Trading is now RESUMED." -ForegroundColor Green
}

function Get-KillSwitchStatus {
    Write-Host ""
    Write-Host "Kill Switch Status" -ForegroundColor Cyan
    Write-Host "==================" -ForegroundColor Cyan
    Write-Host ""

    # Check Redis connection
    $ping = & redis-cli -h $RedisHost -p $RedisPort PING 2>&1
    if ($ping -eq "PONG") {
        Write-Host "[OK] Redis connection: Connected ($RedisHost`:$RedisPort)" -ForegroundColor Green
    } else {
        Write-Host "[ERROR] Redis connection: FAILED" -ForegroundColor Red
        Write-Host "        Make sure Redis is running" -ForegroundColor Gray
    }

    # Check file-based kill switch
    $windowsPath = Join-Path $env:TEMP "arbees_kill_switch"
    if (Test-Path $windowsPath) {
        Write-Host "[ACTIVE] File-based kill switch: ENABLED" -ForegroundColor Red
        Write-Host "         File: $windowsPath" -ForegroundColor Gray
    } else {
        Write-Host "[OK] File-based kill switch: Not active" -ForegroundColor Green
    }

    Write-Host ""
    Write-Host "Commands:" -ForegroundColor Gray
    Write-Host "  .\killswitch.ps1 enable   - Halt all trading" -ForegroundColor Gray
    Write-Host "  .\killswitch.ps1 disable  - Resume trading" -ForegroundColor Gray
    Write-Host ""
}

# Normalize action
$normalizedAction = switch ($Action.ToLower()) {
    "enable"  { "enable" }
    "on"      { "enable" }
    "halt"    { "enable" }
    "disable" { "disable" }
    "off"     { "disable" }
    "resume"  { "disable" }
    "status"  { "status" }
    default   { "status" }
}

# Execute action
switch ($normalizedAction) {
    "enable"  { Enable-KillSwitch }
    "disable" { Disable-KillSwitch }
    "status"  { Get-KillSwitchStatus }
}
