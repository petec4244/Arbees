
import asyncio
import json
import logging
import sys
import traceback

# Add services directory to path
sys.path.append("/app")

# Mock logger
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

from markets.polymarket.client import PolymarketClient

async def main():
    print("--- Inspecting 'VS' Market Structure ---")
    
    client = PolymarketClient()
    try:
        await client.connect()
        
        print("Searching for 'vs' markets in NCAAB...")
        # Using our new searching capabilities
        markets = await client.search_markets("vs", limit=5, sport="ncaab")
        
        if not markets:
            print("No 'vs' markets found. Searching 'nba'...")
            markets = await client.search_markets("vs", limit=5, sport="nba")
            
        print(f"Found {len(markets)} markets.")

        for m in markets[:2]:
            try:
                title = m.get("question", m.get("title", ""))
                mid = m.get("condition_id") or m.get("id")
                outcomes = m.get("outcomes")
                tokens = m.get("tokens")
                clob_ids = m.get("clobTokenIds")
                
                print(f"\n[MARKET] {title} (ID: {mid})")
                print(f"  Outcomes: {outcomes} (Type: {type(outcomes)})")
                print(f"  Tokens: {tokens} (Type: {type(tokens)})")
                print(f"  CLOB Token IDs: {clob_ids} (Type: {type(clob_ids)})")
                
                # Test token resolution
                yes_token = await client.resolve_yes_token_id(m)
                print(f"  Legacy YES Token: {yes_token}")
                
                # Test Explicit resolution
                # Guess team name from outcomes
                if outcomes and len(outcomes) > 0:
                    team_a = str(outcomes[0])
                    print(f"  Testing resolution for '{team_a}'...")
                    token_a = client.resolve_outcome_token_id(m, team_a)
                    print(f"  [TEST] Resolve '{team_a}': {token_a}")
                    
                    # Test partial match
                    partial = team_a.split(" ")[0] # e.g. "Notre"
                    token_partial = client.resolve_outcome_token_id(m, partial)
                    print(f"  [TEST] Resolve partial '{partial}': {token_partial}")
                    
                    if token_a == token_partial:
                         print("  [SUCCESS] Partial match resolution consistent.")
                    else:
                         print("  [WARNING] Partial match mismatch!")

                if not yes_token and not token_a:
                    print("  [WARNING] Could not resolve any token")

            except Exception as e:
                print(f"Error inspecting market {mid}: {e}")
                traceback.print_exc()
                
    except Exception as e:
        print(f"Global Error: {e}")
        traceback.print_exc()
    finally:
        await client.close()
        print("Client closed.")

if __name__ == "__main__":
    asyncio.run(main())
