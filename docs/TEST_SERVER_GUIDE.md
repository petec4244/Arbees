# Test Server Guide for Live Trading Integration

This guide explains how to test Polymarket and Kalshi trading integrations using their respective test environments before going live with real funds.

---

## Overview

| Platform | Test Environment | Funds Required | API Differences |
|----------|------------------|----------------|-----------------|
| **Kalshi** | Demo environment | Free test money | Same API, different URLs |
| **Polymarket** | Amoy testnet | Free testnet MATIC | Same API, different chain ID |

---

## Kalshi Demo Environment

Kalshi provides a full demo environment with simulated markets and free test funds.

### Setup

1. **Create Demo Account**
   - Visit [https://demo.kalshi.com](https://demo.kalshi.com)
   - Sign up for a demo account (separate from production)
   - Generate API keys in the demo dashboard

2. **Generate Demo API Keys**
   - In demo account settings, create an API key pair
   - Download or copy your private key (RSA format)

### Environment Variables

```bash
# Tell the system to use demo environment
KALSHI_ENV=demo

# Demo-specific credentials (preferred)
KALSHI_DEMO_API_KEY=your_demo_api_key
KALSHI_DEMO_PRIVATE_KEY="-----BEGIN RSA PRIVATE KEY-----
...your demo private key...
-----END RSA PRIVATE KEY-----"

# Or use a key file path
KALSHI_DEMO_PRIVATE_KEY_PATH=/path/to/demo_private_key.pem

# Alternatively, use URL overrides (for custom setups)
# KALSHI_BASE_URL=https://demo-api.kalshi.co/trade-api/v2
# KALSHI_WS_URL=wss://demo-api.kalshi.co/trade-api/ws/v2
```

### Demo Endpoints

| Service | Production | Demo |
|---------|------------|------|
| REST API | `https://api.elections.kalshi.com/trade-api/v2` | `https://demo-api.kalshi.co/trade-api/v2` |
| WebSocket | `wss://api.elections.kalshi.com/trade-api/ws/v2` | `wss://demo-api.kalshi.co/trade-api/ws/v2` |

### Testing Commands

```bash
# Run with demo environment
KALSHI_ENV=demo cargo run --package execution_service_rust

# Or in Python services
KALSHI_ENV=demo python services/signal_processor/main.py

# Smoke test
KALSHI_ENV=demo python scripts/api_smoke_test.py --kalshi-only
```

### Kalshi Demo Notes

- Demo account has free simulated funds for testing
- Markets in demo may differ from production (fewer sports markets)
- Demo credentials are separate from production credentials
- Rate limits are the same as production
- Order matching works the same way

---

## Polymarket Amoy Testnet

Polymarket uses Polygon's Amoy testnet for testing. This requires testnet MATIC tokens (free from faucets).

### Setup

1. **Create Testnet Wallet**
   - Use any Ethereum wallet (MetaMask, etc.)
   - Switch to Polygon Amoy testnet (Chain ID: 80002)
   - Get testnet MATIC from a faucet

2. **Get Testnet MATIC**
   - [Polygon Faucet](https://faucet.polygon.technology/) - Select Amoy testnet
   - [Alchemy Faucet](https://www.alchemy.com/faucets/polygon-amoy)
   - You need MATIC for gas fees

3. **Deposit to Polymarket Testnet**
   - The Polymarket testnet CLOB uses the same deposit flow
   - You'll get a funder/proxy wallet address after deposit

### Environment Variables

```bash
# Testnet chain ID (Polygon Amoy)
POLYMARKET_CHAIN_ID=80002

# Your testnet wallet credentials
POLYMARKET_PRIVATE_KEY=0x...your_testnet_wallet_private_key...
POLYMARKET_FUNDER_ADDRESS=0x...your_testnet_funder_address...

# CLOB host (same for testnet, chain ID determines network)
POLYMARKET_CLOB_HOST=https://clob.polymarket.com

# API key derivation nonce (usually 0)
POLYMARKET_API_NONCE=0

# Disable paper trading to test real CLOB calls
PAPER_TRADING=0
```

### Testnet Exchange Addresses

The CLOB client automatically uses these addresses based on chain ID:

| Chain | neg_risk Exchange | Standard Exchange |
|-------|-------------------|-------------------|
| Mainnet (137) | `0xC5d563A36AE78145C45a50134d48A1215220f80a` | `0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E` |
| Amoy (80002) | `0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296` | `0xdFE02Eb6733538f8Ea35D585af8DE5958AD99E40` |

### Testing Commands

```bash
# Run execution service with testnet
POLYMARKET_CHAIN_ID=80002 PAPER_TRADING=0 cargo run --package execution_service_rust

# Run unit tests
cargo test --package arbees_rust_core -- polymarket_clob
```

### Polymarket Testnet Notes

- **VPN Required**: The CLOB API is geo-restricted even on testnet
- **Limited Markets**: Testnet may have fewer or different markets
- **Token IDs**: Testnet token IDs differ from mainnet
- **Free to Test**: Testnet MATIC is free from faucets
- **Same Code Path**: Same signing, same API calls, different chain

---

## Combined Testing Strategy

### Phase 1: Unit Tests (No API Calls)

```bash
# Run all CLOB unit tests
cargo test --package arbees_rust_core -- polymarket_clob

# Expected output:
# test_price_to_bps ... ok
# test_size_to_micro ... ok
# test_get_order_amounts_buy ... ok
# test_get_order_amounts_sell ... ok
# test_price_valid ... ok
# test_exchange_addresses ... ok
```

### Phase 2: Kalshi Demo Integration

```bash
# Set demo environment
export KALSHI_ENV=demo
export KALSHI_DEMO_API_KEY=your_demo_key
export KALSHI_DEMO_PRIVATE_KEY_PATH=/path/to/demo_key.pem
export PAPER_TRADING=0

# Start execution service
cargo run --package execution_service_rust

# In another terminal, send a test execution request via Redis
# (or use the signal processor to generate a signal)
```

### Phase 3: Polymarket Testnet Integration

```bash
# Set testnet environment
export POLYMARKET_CHAIN_ID=80002
export POLYMARKET_PRIVATE_KEY=0x...testnet_key...
export POLYMARKET_FUNDER_ADDRESS=0x...testnet_funder...
export PAPER_TRADING=0

# Ensure VPN is running (required even for testnet)
docker-compose --profile vpn up -d vpn

# Start execution service through VPN
# Option 1: Run locally with VPN proxy
# Option 2: Run in Docker with network_mode: "service:vpn"
cargo run --package execution_service_rust
```

### Phase 4: End-to-End Test

```bash
# Full stack with both demo/testnet configs
docker-compose --profile full up -d

# Check logs for successful initialization
docker-compose logs -f execution_service

# Expected log output:
# INFO  Kalshi client initialized with trading credentials
# INFO  Polymarket CLOB executor initialized
# INFO  Execution Service ready (Paper Trading: false, Kalshi Live: true, Polymarket Live: true)
```

---

## Troubleshooting

### Kalshi Demo Issues

| Issue | Solution |
|-------|----------|
| "Invalid credentials" | Ensure using demo-specific keys, not production |
| "Market not found" | Demo has limited markets; check available markets first |
| Rate limit errors | Same limits as prod; add delays between requests |

### Polymarket Testnet Issues

| Issue | Solution |
|-------|----------|
| "derive-api-key failed" | Check VPN is active and routing through EU |
| "Insufficient funds" | Get more testnet MATIC from faucet |
| "Invalid token_id" | Testnet token IDs differ from mainnet |
| "Unsupported chain" | Verify `POLYMARKET_CHAIN_ID=80002` |
| Connection timeout | VPN may be required even for testnet |

### General Issues

| Issue | Solution |
|-------|----------|
| "CLOB not configured" | Check all `POLYMARKET_*` env vars are set |
| "Kalshi credentials not configured" | Check `KALSHI_ENV` and credential vars |
| Orders rejected | Check price is within valid range (0.01-0.99) |

---

## Environment Variable Quick Reference

### Kalshi Demo
```bash
KALSHI_ENV=demo
KALSHI_DEMO_API_KEY=...
KALSHI_DEMO_PRIVATE_KEY=...      # Or use _PATH variant
```

### Polymarket Testnet
```bash
POLYMARKET_CHAIN_ID=80002
POLYMARKET_PRIVATE_KEY=0x...
POLYMARKET_FUNDER_ADDRESS=0x...
POLYMARKET_CLOB_HOST=https://clob.polymarket.com
POLYMARKET_API_NONCE=0
```

### General Testing
```bash
PAPER_TRADING=0                  # Required for live API calls
RUST_LOG=debug                   # More verbose logging
```

---

## Sample .env for Testing

```bash
# =============================================================================
# TEST ENVIRONMENT CONFIGURATION
# =============================================================================

# General
PAPER_TRADING=0
RUST_LOG=info,arbees_rust_core=debug,execution_service_rust=debug

# -----------------------------------------------------------------------------
# Kalshi Demo
# -----------------------------------------------------------------------------
KALSHI_ENV=demo
KALSHI_DEMO_API_KEY=your_demo_api_key_here
KALSHI_DEMO_PRIVATE_KEY="-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEA...
...your demo private key here...
-----END RSA PRIVATE KEY-----"

# -----------------------------------------------------------------------------
# Polymarket Testnet (Amoy)
# -----------------------------------------------------------------------------
POLYMARKET_CHAIN_ID=80002
POLYMARKET_PRIVATE_KEY=0xabc123...your_testnet_private_key...
POLYMARKET_FUNDER_ADDRESS=0xdef456...your_testnet_funder...
POLYMARKET_CLOB_HOST=https://clob.polymarket.com
POLYMARKET_API_NONCE=0

# -----------------------------------------------------------------------------
# Infrastructure (same for test and prod)
# -----------------------------------------------------------------------------
REDIS_URL=redis://localhost:6379
DATABASE_URL=postgresql://arbees:password@localhost:5432/arbees
```

---

## Progression to Production

1. **Unit Tests** - No external dependencies
2. **Kalshi Demo** - Free simulated trading
3. **Polymarket Testnet** - Free testnet tokens
4. **Production (Small)** - Start with minimum order sizes
5. **Production (Full)** - Gradual increase based on confidence

### Production Checklist

- [ ] All unit tests pass
- [ ] Demo/testnet orders execute successfully
- [ ] Error handling verified (rejections, timeouts)
- [ ] VPN reliability tested (for Polymarket)
- [ ] Logging sufficient for debugging
- [ ] Credentials secured (not in version control)
- [ ] Rate limits understood and respected
- [ ] Circuit breakers configured
- [ ] Monitoring/alerting in place

---

## Additional Resources

- [Kalshi API Documentation](https://docs.kalshi.com/)
- [Kalshi Demo Environment](https://demo.kalshi.com)
- [Polymarket CLOB Documentation](https://docs.polymarket.com/)
- [Polygon Amoy Testnet](https://wiki.polygon.technology/docs/tools/wallets/metamask/config-polygon-on-wallet/)
- [Polygon Faucet](https://faucet.polygon.technology/)
