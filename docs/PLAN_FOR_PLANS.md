# Plan for Plans: Pending Implementation Steps

## Executive Summary
This document outlines the remaining strategic steps required to bring the Arbees system to full production readiness. The system core is built (see `COMPLETE_REVIEW.md`), and the focus now shifts to **Infrastructure, Connectivity, and Hardening**.

---

## 1. Polymarket VPN Architecture (IMMEDIATE PRIORITY)
**Primary Document:** `docs/VPN_FOR_POLYMARKETPLAN.md`

**Status:** 🚧 **Next Up**
The system currently lacks a reliable way to bypass Polymarket's geofencing for automated trading.
- **Goal:** Deploy a specialized monitor shard behind a VPN to stream Polymarket CLOB data.
- **Key Step:** Isolate Polymarket traffic from Kalshi traffic (which requires low-latency direct lines).
- **Consolidated From:** `INTEGRATED_IMPLEMENTATION_PLAN.md` (Foundation Layer).

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
**Derived From:** `RISK_MANAGEMENT_2026-01-20.md`

**Status:** 🔄 **Continuous Improvement**
- **Tasks:**
  - Refine position sizing logic in `PositionManager`.
  - Implement tighter stop-loss mechanisms for high-volatility markets.
  - Verify "No Market" rejection logic (recently added) prevents execution against stale data.
