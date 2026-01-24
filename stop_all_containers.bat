@echo off
echo Stopping all Docker containers...

for /f "tokens=*" %%i in ('docker ps -q 2^>nul') do (
    docker stop %%i
)

echo Done.
pause
