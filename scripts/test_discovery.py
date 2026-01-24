
import asyncio
import logging
import sys
import json

# Add services directory to path
sys.path.append("/app")

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger("TestDiscovery")

from markets.polymarket.client import PolymarketClient
# NOTE: Market parsing is now handled by the Rust market_discovery_rust service

# Mock aliases for testing
TEST_ALIASES = {
    "notre dame": ["notre dame", "notre dame fighting irish"],
    "uconn": ["uconn", "connecticut", "connecticut huskies"],
    "lakers": ["lakers", "los angeles lakers"],
    "celtics": ["celtics", "boston celtics"]
}

async def main():
    if len(sys.argv) < 3:
        print("Usage: python scripts/test_discovery.py <sport> <query> [home_team] [away_team]")
        print("Example: python scripts/test_discovery.py ncaab 'Notre Dame' 'Notre Dame' 'UConn'")
        return

    sport = sys.argv[1]
    query = sys.argv[2]
    home_team = sys.argv[3] if len(sys.argv) > 3 else query
    away_team = sys.argv[4] if len(sys.argv) > 4 else "Opponent"

    print(f"\n--- Testing Discovery for Sport: {sport}, Query: {query} ---")
    print(f"Target Match: {home_team} vs {away_team}")
    
    client = PolymarketClient()
    try:
        await client.connect()
        
        # 1. Search
        print(f"\n[1] Searching markets with tag '{sport}'...")
        markets = await client.search_markets(query, limit=10, sport=sport)
        print(f"Found {len(markets)} markets.")

        # 2. Analyze Matches
        print(f"\n[2] Analyzing Matches...")
        for m in markets:
            title = m.get("question", m.get("title", ""))
            mid = m.get("condition_id") or m.get("id")
            
            print(f"\n  Market: {title} (ID: {mid})")
            
            # Check Team Match
            home_aliases = TEST_ALIASES.get(home_team.lower(), [home_team.lower()])
            away_aliases = TEST_ALIASES.get(away_team.lower(), [away_team.lower()])
            
            title_lower = title.lower()
            home_match = any(a in title_lower for a in home_aliases)
            away_match = any(a in title_lower for a in away_aliases)
            
            print(f"    Home Match ({home_team}): {home_match}")
            print(f"    Away Match ({away_team}): {away_match}")
            
            if home_match and away_match:
                print("    [MATCH FOUND]")
                
                # Resolve Token
                token_id = client.resolve_outcome_token_id(m, home_aliases)
                if token_id:
                     print(f"    [SUCCESS] Resolved Home Token ID: {token_id}")
                else:
                     print(f"    [WARNING] Failed to resolve Home Token ID (Aliases: {home_aliases})")
                     print(f"    Outcomes: {m.get('outcomes')}")
            
    except Exception as e:
        logger.error(f"Error: {e}")
        import traceback
        traceback.print_exc()
    finally:
        await client.close()

if __name__ == "__main__":
    asyncio.run(main())
