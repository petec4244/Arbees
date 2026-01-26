# Plan for Plans: Pending Implementation Steps

## Executive Summary
This document outlines the remaining strategic steps required to bring the Arbees system to full production readiness. The system core is built (see `COMPLETE_REVIEW.md`), and the focus now shifts to **Infrastructure, Connectivity, and Hardening**.

---

## 1. Polymarket VPN Architecture
**Primary Document:** `docs/VPN_FOR_POLYMARKETPLAN.md`

**Status:** ✅ **IMPLEMENTED**
The VPN architecture is complete and operational:
- Gluetun VPN container configured in `docker-compose.yml`
- Polymarket monitor runs behind VPN (network_mode: service:vpn)
- Kalshi connections remain direct (low-latency)
- All services access Polymarket prices via Redis (< 5ms latency)

**Reference:** See `docker-compose.yml` lines 565-615 for VPN configuration.

## 2. AWS Distributed Deployment
**Primary Document:** `docs/AWS_DEPLOYMENT.md`

**Status:** 📋 **Pending Execution**
The infrastructure code (Dockerfiles, `docker-compose.yml`) is ready, but the cloud environment needs instantiation.
- **Goal:** Deploy the multi-container stack to AWS ECS.
- **Requirements:**
  - Setup AWS VPC & Security Groups.
  - Create ECS Clusters.
  - Configure CI/CD pipelines (GitHub Actions).
  - Provision Databases (RDS/TimescaleDB) and Cache (ElastiCache/Redis).

## 3. Production Hardening & Monitoring
**Derived From:** `INTEGRATED_IMPLEMENTATION_PLAN.md` (Week 4)

**Status:** 📋 **Planned**
Once the infrastructure is live, the system requires robust monitoring.
- **Tasks:**
  - **Circuit Breaker Tuning:** Calibrate `circuit_breaker` thresholds in `shard.py` based on livetest data.
  - **Alerting:** Configure Prometheus/Grafana for system health and PnL monitoring.
  - **End-to-End Testing:** Verify the full loop (VPN -> Detection -> Execution) under load.

## 4. Advanced Risk Management refining

**Status:** ✅ **IMPLEMENTED** - Risk management is operational
- Risk limits enforced (per-game, per-sport, daily loss)
- Position correlation detection
- Latency circuit breaker
- Signal spam control

**Note:** Continuous improvement ongoing based on trading data.
