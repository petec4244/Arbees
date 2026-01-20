#!/bin/bash
# OpenVPN entrypoint script for Polymarket RPi Monitor
# Starts OpenVPN if configured, then runs the main application

set -e

OPENVPN_CONFIG="${OPENVPN_CONFIG:-/etc/openvpn/client.ovpn}"
VPN_ENABLED="${VPN_ENABLED:-true}"

# Function to wait for VPN connection
wait_for_vpn() {
    echo "Waiting for VPN connection..."
    local max_attempts=30
    local attempt=0

    while [ $attempt -lt $max_attempts ]; do
        if ip addr show tun0 &>/dev/null; then
            echo "VPN connected (tun0 interface up)"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done

    echo "ERROR: VPN connection timeout"
    return 1
}

# Start OpenVPN if enabled and config exists
if [ "$VPN_ENABLED" = "true" ] && [ -f "$OPENVPN_CONFIG" ]; then
    echo "Starting OpenVPN with config: $OPENVPN_CONFIG"

    # Start OpenVPN in background
    openvpn --config "$OPENVPN_CONFIG" --daemon --log /var/log/openvpn.log

    # Wait for VPN to establish
    if ! wait_for_vpn; then
        echo "VPN failed to connect, check /var/log/openvpn.log"
        cat /var/log/openvpn.log || true
        exit 1
    fi

    # Verify external IP (optional, for debugging)
    echo "Checking external IP..."
    curl -s --max-time 10 https://api.ipify.org || echo "(IP check failed)"
    echo ""
else
    if [ "$VPN_ENABLED" = "true" ]; then
        echo "WARNING: VPN enabled but config not found at $OPENVPN_CONFIG"
        echo "Running without VPN - Polymarket may be geo-blocked"
    else
        echo "VPN disabled, running without VPN"
    fi
fi

# Execute the main command
exec "$@"
