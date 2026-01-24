"""
One-off maintenance script:
Close any OPEN paper trades for games that are already FINAL/COMPLETE in game_states.

This is intended to unblock paper trading capacity when end-of-game settlement
didn't run (e.g., transient monitoring errors).

Usage (inside a container with DATABASE_URL):
  python scripts/settle_final_trades.py
"""

import asyncio
import os
from datetime import datetime, timezone

import asyncpg


SPORT_PERIODS: dict[str, int] = {
    "nfl": 4,
    "ncaaf": 4,
    "nba": 4,
    "ncaab": 2,
    "nhl": 3,
    "mlb": 9,
    "mls": 2,
    "soccer": 2,
    "tennis": 3,
    "mma": 5,
}


def _score_match(team: str, title: str) -> int:
    """Match score: 0 none, 1 partial, 2 full team, 3 nickname exact."""
    if not team or not title:
        return 0
    team_lower = team.lower()
    title_lower = title.lower()

    parts = [p for p in team_lower.split() if p]
    nickname = parts[-1] if parts else ""
    if nickname:
        # word-ish match
        if f" {nickname} " in f" {title_lower} " or title_lower.startswith(nickname + " "):
            return 3

    if team_lower in title_lower:
        return 2

    # weak partial match
    for p in parts:
        if len(p) >= 4 and p in title_lower:
            return 1

    return 0


async def main() -> None:
    dsn = os.environ["DATABASE_URL"]
    conn = await asyncpg.connect(dsn)

    rows = await conn.fetch(
        """
        WITH latest AS (
          SELECT DISTINCT ON (game_id)
            game_id, sport, time, status, home_score, away_score, period, time_remaining
          FROM game_states
          ORDER BY game_id, time DESC
        )
        SELECT
          pt.trade_id,
          pt.game_id,
          pt.side,
          pt.entry_price,
          pt.size,
          COALESCE(pt.market_title, '') AS market_title,
          l.status AS last_status,
          l.sport AS sport,
          l.home_score,
          l.away_score,
          COALESCE(g.home_team, '') AS home_team,
          COALESCE(g.away_team, '') AS away_team,
          l.period,
          l.time_remaining
        FROM paper_trades pt
        JOIN latest l ON l.game_id = pt.game_id
        LEFT JOIN games g ON g.game_id = pt.game_id
        WHERE pt.status = 'open'
          AND pt.game_id IS NOT NULL
          AND (
            l.status = 'final'
            OR l.status ILIKE '%final%'
            OR l.status ILIKE '%complete%'
            OR (l.status ILIKE '%end_period%' AND (l.time_remaining LIKE '0:%' OR l.time_remaining IN ('0', '0.0', '0:00')))
          )
        ORDER BY l.time DESC
        """
    )

    if not rows:
        print("No open trades found for final/complete games.")
        await conn.close()
        return

    # DB column type is timestamptz; pass datetime (not string)
    exit_time = datetime.now(timezone.utc)
    closed = 0

    for r in rows:
        trade_id = r["trade_id"]
        game_id = r["game_id"]
        side = (r["side"] or "").lower()
        entry_price = float(r["entry_price"])
        size = float(r["size"])
        title = r["market_title"] or ""
        home_team = r["home_team"] or ""
        away_team = r["away_team"] or ""
        home_score = int(r["home_score"])
        away_score = int(r["away_score"])
        last_status = (r.get("last_status") or "").lower()
        sport = (r.get("sport") or "").lower()
        period = int(r.get("period") or 0)
        time_remaining = (r.get("time_remaining") or "").strip()

        # If we're only at end_period, ensure it's likely end-of-game (not end-of-quarter).
        if "end_period" in last_status and "final" not in last_status and "complete" not in last_status:
            expected_periods = SPORT_PERIODS.get(sport)
            if expected_periods is not None and period < expected_periods:
                continue
            if not (time_remaining.startswith("0") or time_remaining in ("0.0", "0:00", "0")):
                continue

        if home_score == away_score:
            home_won = None
        else:
            home_won = home_score > away_score

        hs = _score_match(home_team, title)
        as_ = _score_match(away_team, title)

        team_won: bool | None = None
        exit_price = entry_price  # default push

        if home_won is None:
            team_won = None
            exit_price = entry_price
        else:
            if hs > as_ and hs > 0:
                team_won = bool(home_won)
                exit_price = 1.0 if team_won else 0.0
            elif as_ > hs and as_ > 0:
                team_won = not bool(home_won)
                exit_price = 1.0 if team_won else 0.0
            else:
                team_won = None
                exit_price = entry_price

        if team_won is None:
            outcome = "push"
        elif side == "buy":
            outcome = "win" if team_won else "loss"
        else:
            outcome = "loss" if team_won else "win"

        gross_pnl = size * (exit_price - entry_price) if side == "buy" else size * (entry_price - exit_price)
        pnl = gross_pnl
        risk = size * entry_price if side == "buy" else size * (1.0 - entry_price)
        pnl_pct = (pnl / risk * 100.0) if risk > 0 else 0.0

        await conn.execute(
            """
            UPDATE paper_trades
            SET exit_price = $2,
                exit_time = $3,
                status = 'closed',
                outcome = $4,
                exit_fees = $5,
                pnl = $6,
                pnl_pct = $7
            WHERE trade_id = $1
              AND status = 'open'
            """,
            trade_id,
            float(exit_price),
            exit_time,
            outcome,
            0.0,
            float(pnl),
            float(pnl_pct),
        )

        closed += 1
        print(
            f"Closed {trade_id} game={game_id} side={side} "
            f"exit={exit_price:.4f} outcome={outcome} pnl={pnl:.2f} title='{title}' "
            f"exit_time={exit_time.isoformat()}"
        )

    await conn.close()
    print(f"Done. Closed {closed} trade(s).")


if __name__ == "__main__":
    asyncio.run(main())

