# Rebuild all docker containers one at a time with no cache
# Include full profile to get all services

$services = docker compose --profile full config --services

Write-Host "Found $($services.Count) services to rebuild`n" -ForegroundColor Yellow

foreach ($service in $services) {
    Write-Host "Building $service..." -ForegroundColor Green
    docker compose build --no-cache $service
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Failed to build $service" -ForegroundColor Red
        exit 1
    }
    Write-Host "$service completed`n" -ForegroundColor Cyan
}

Write-Host "All services rebuilt successfully!" -ForegroundColor Green
