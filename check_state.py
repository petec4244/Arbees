import asyncio
import os
import asyncpg
from datetime import datetime

async def check_db():
    url = os.environ.get("DATABASE_URL")
    if not url:
        print("DATABASE_URL not found")
        return

    conn = await asyncpg.connect(url)
    
    print("\n--- Bankroll ---")
    rows = await conn.fetch("SELECT account_name, current_balance, piggybank_balance FROM bankroll")
    for r in rows:
        print(dict(r))

    print("\n--- Live Games (Game States) ---")
    rows = await conn.fetch("""
        SELECT gs.game_id, gs.sport, gs.status, gs.home_win_prob, gs.time
        FROM game_states gs
        ORDER BY gs.time DESC
        LIMIT 5
    """)
    for r in rows:
        print(dict(r))

    print("\n--- Active Games Count ---")
    count = await conn.fetchval("SELECT COUNT(DISTINCT game_id) FROM game_states WHERE status NOT IN ('final', 'completed')")
    print(f"Active games in states: {count}")

    print("\n--- Games Table (Teams) ---")
    rows = await conn.fetch("SELECT game_id, sport, home_team, away_team FROM games LIMIT 3")
    for r in rows:
        print(dict(r))

    await conn.close()

if __name__ == "__main__":
    asyncio.run(check_db())
