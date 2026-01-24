"""
Synthetic trade injector for paper engine.

Usage example (inside position_manager container):
  python /app/scripts/synthetic_trade.py --platform kalshi --team "Test Team A" --cleanup
"""

import argparse
import asyncio
import os
from datetime import datetime, timezone

import asyncpg

from arbees_shared.db.connection import DatabaseClient
from arbees_shared.models.game import Sport
from arbees_shared.models.market import MarketPrice, Platform
from arbees_shared.models.signal import TradingSignal, SignalDirection, SignalType
from markets.paper.engine import PaperTradingEngine


async def _maybe_insert_game(conn: asyncpg.Connection, game_id: str, sport: str, home: str, away: str) -> None:
    await conn.execute(
        """
        INSERT INTO games (game_id, sport, home_team, away_team, scheduled_time, status)
        VALUES ($1, $2, $3, $4, $5, 'in_progress')
        ON CONFLICT (game_id) DO UPDATE
        SET sport = EXCLUDED.sport,
            home_team = EXCLUDED.home_team,
            away_team = EXCLUDED.away_team,
            scheduled_time = EXCLUDED.scheduled_time,
            status = EXCLUDED.status
        """,
        game_id,
        sport,
        home,
        away,
        datetime.now(timezone.utc),
    )


async def _maybe_insert_market_price(
    conn: asyncpg.Connection,
    market_id: str,
    platform: str,
    game_id: str,
    title: str,
    team: str,
    yes_bid: float,
    yes_ask: float,
    yes_bid_size: float,
    yes_ask_size: float,
) -> None:
    await conn.execute(
        """
        INSERT INTO market_prices (
            time, market_id, platform, game_id, market_title,
            yes_bid, yes_ask, yes_bid_size, yes_ask_size,
            volume, liquidity, status, market_type, contract_team
        )
        VALUES (NOW(), $1, $2, $3, $4, $5, $6, $7, $8, 0, 1000, 'open', 'moneyline', $9)
        """,
        market_id,
        platform,
        game_id,
        title,
        yes_bid,
        yes_ask,
        yes_bid_size,
        yes_ask_size,
        team,
    )


async def _cleanup_test_data(conn: asyncpg.Connection, game_id: str) -> None:
    await conn.execute("DELETE FROM market_prices WHERE game_id = $1", game_id)
    await conn.execute("DELETE FROM paper_trades WHERE game_id = $1", game_id)
    await conn.execute("DELETE FROM games WHERE game_id = $1", game_id)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Inject a synthetic paper trade.")
    parser.add_argument("--platform", choices=["kalshi", "polymarket"], default="kalshi")
    parser.add_argument("--game-id", default="SYNTH_GAME_KALSHI")
    parser.add_argument("--market-id", default="SYNTH_MARKET_1")
    parser.add_argument("--sport", default="ncaab")
    parser.add_argument("--home-team", default="Test Team A")
    parser.add_argument("--away-team", default="Test Team B")
    parser.add_argument("--team", default="Test Team A")
    parser.add_argument("--direction", choices=["buy", "sell"], default="buy")
    parser.add_argument("--edge", type=float, default=9.0)
    parser.add_argument("--model-prob", type=float, default=0.60)
    parser.add_argument("--market-prob", type=float, default=0.51)
    parser.add_argument("--yes-bid", type=float, default=0.50)
    parser.add_argument("--yes-ask", type=float, default=0.52)
    parser.add_argument("--yes-bid-size", type=float, default=500.0)
    parser.add_argument("--yes-ask-size", type=float, default=500.0)
    parser.add_argument("--exit-yes-bid", type=float, default=0.56)
    parser.add_argument("--exit-yes-ask", type=float, default=0.58)
    parser.add_argument("--initial-bankroll", type=float, default=1000.0)
    parser.add_argument("--persist", action="store_true", help="Persist trades to DB")
    parser.add_argument("--record-market", action="store_true", help="Insert market prices into DB")
    parser.add_argument("--cleanup", action="store_true", help="Cleanup test data after run")
    return parser


async def main() -> None:
    args = _build_parser().parse_args()

    # DB connection for optional persistence and cleanup
    db_url = os.environ.get("DATABASE_URL")
    conn = await asyncpg.connect(db_url) if db_url else None

    try:
        if conn:
            await _maybe_insert_game(conn, args.game_id, args.sport, args.home_team, args.away_team)
            if args.record_market:
                await _maybe_insert_market_price(
                    conn,
                    args.market_id,
                    args.platform,
                    args.game_id,
                    f"{args.home_team} vs {args.away_team} [{args.team}]",
                    args.team,
                    args.yes_bid,
                    args.yes_ask,
                    args.yes_bid_size,
                    args.yes_ask_size,
                )

        # Build signal + market price
        signal = TradingSignal(
            signal_id=f"synth-{args.platform}",
            signal_type=SignalType.WIN_PROB_SHIFT,
            game_id=args.game_id,
            sport=Sport(args.sport),
            team=args.team,
            direction=SignalDirection.BUY if args.direction == "buy" else SignalDirection.SELL,
            model_prob=args.model_prob,
            market_prob=args.market_prob,
            edge_pct=args.edge,
            confidence=0.8,
            reason="synthetic test",
        )

        price = MarketPrice(
            market_id=args.market_id,
            platform=Platform(args.platform),
            game_id=args.game_id,
            market_title=f"{args.home_team} vs {args.away_team} [{args.team}]",
            contract_team=args.team,
            yes_bid=args.yes_bid,
            yes_ask=args.yes_ask,
            yes_bid_size=args.yes_bid_size,
            yes_ask_size=args.yes_ask_size,
        )

        db_client = DatabaseClient() if args.persist else None
        engine = PaperTradingEngine(initial_bankroll=args.initial_bankroll, db_client=db_client)

        trade = await engine.execute_signal(signal, price)
        if not trade:
            print("Trade rejected by paper engine.")
            return

        exit_price = args.exit_yes_bid if trade.side.value == "buy" else args.exit_yes_ask
        closed = await engine.close_trade(trade, exit_price, already_executable=True)

        print("Synthetic trade completed:")
        print(f"  side={closed.side.value} entry={closed.entry_price:.3f} exit={closed.exit_price:.3f}")
        print(f"  entry_fees=${closed.entry_fees:.2f} exit_fees=${closed.exit_fees:.2f} pnl=${closed.pnl:.2f}")

    finally:
        if conn and args.cleanup:
            await _cleanup_test_data(conn, args.game_id)
        if conn:
            await conn.close()


if __name__ == "__main__":
    asyncio.run(main())
