# OBE (Overcome By Events) & Reference Plans

## Overview
This document catalogs plans, analyses, and prompts that have been superseded by implementation, rendered obsolete by strategic shifts, or serve purely as historical reference. They should be archived or ignored in favor of the active documentation.

## Superseded / Implemented Documents

### Strategy & Analysis
- **`ANALYTICS_VS_REALTIME_STRATEGY.md`**: Strategic decision made. We implemented "Both" (Hybrid approach).
- **`MARKET_TYPES_ANALYSIS.md`**: Implemented. We now support Multi-Market types.
- **`TERAUSS_VS_ARBEES_ANALYSIS.md`**: Implemented. Arbee's adopted the `terauss` Rust core.
- **`REDDIT_SHARON_ANALYSIS.md`**: Background research. No longer actionable.
- **`PAPER_TRADING_ANALYSIS_2026-01-20.md`**: Snapshots of past performance. Historical data only.

### Technical Plans
- **`Full_plan.md`**: The original master plan. Most core components (Shards, Redis, DB) are built. Use `COMPLETE_REVIEW.md` for current state and `PLAN_FOR_PLANS.md` for future.
- **`INTEGRATED_IMPLEMENTATION_PLAN.md`**: Large parts (Rust integration) are done. The remaining parts (VPN) have been extracted to `VPN_FOR_POLYMARKETPLAN.md`.
- **`MIGRATION_ARBEES.md`**: Old migration logic. OBE.
- **`KALSHI_API_DEMO_README.md`**: Demo code no longer relevant to the main codebase.

### Prompts & Meta-Docs
- **`CLAUDE_CODE_INTEGRATION_PROMPT.md`**: Used to bootstrap the integration.
- **`TERAUSS_INTEGRATION_PROMPT.md`**: Used to guide the Rust port.
- **`BETTING_TERMINOLOGY_GUIDE.md`**: Reference dictionary.

### Bug Fixes
- **`DEBUGGING_NO_SIGNALS.md`**: Troubleshooting guide for a past issue.
- **`SECURITY_FIXES_2026-01-20.md`**: Log of applied security patches.
