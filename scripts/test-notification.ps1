# Test notification script - sends properly formatted JSON to Redis
$json = @'
{"type":"trade_entry","priority":"CRITICAL","data":{"message":"Test notification from PowerShell script"}}
'@

docker exec arbees-redis redis-cli PUBLISH "notification:events" $json

Write-Host "Sent test notification. Check logs with:" -ForegroundColor Cyan
Write-Host "  docker compose --profile full logs --tail=10 notification_service_rust" -ForegroundColor Gray
