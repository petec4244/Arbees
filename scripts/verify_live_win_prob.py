import asyncio
import logging
from arbees_shared.models.game import Sport
from data_providers.espn.client import ESPNClient
import arbees_core
from services.game_shard.shard import GameShard

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(name)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

async def verify_live_games():
    """Fetch live games and calculate win probabilities using the new GameShard integration."""
    print("--- Starting Live Game Verification ---")
    
    # Initialize GameShard (lighter version just for logic)
    shard = GameShard(shard_id="verifier")
    
    # We'll check a few sports commonly active
    sports_to_check = [Sport.NBA, Sport.NHL, Sport.NCAAB] # Add NFL/CFB if in season/time
    
    games_found = False
    
    for sport_enum in sports_to_check:
        print(f"\nChecking for live {sport_enum.value} games...")
        async with ESPNClient(sport=sport_enum) as client:
            live_games = await client.get_live_games()
            
            if not live_games:
                print(f"No live {sport_enum.value} games found.")
                continue
                
            games_found = True
            for game_info in live_games:
                print(f"\nAnalyzing Game: {game_info.display_name}")
                
                # Get detailed state
                state = await client.get_game_state(game_info.game_id)
                if not state:
                    print("  Could not fetch detailed game state.")
                    continue
                
                print(f"  Score: {state.home_team} {state.home_score} - {state.away_score} {state.away_team}")
                print(f"  Time: {state.time_remaining} (Period {state.period})")
                
                # Use the Shard's new logic to calculate win prob
                # We need to access the private method for testing, or we can copy the logic
                # Ideally we test via the shard method to verify the integration
                try:
                    win_prob = shard._calculate_win_prob(state)
                    print(f"  Rust Core Win Probability (Home): {win_prob:.4f} ({win_prob*100:.1f}%)")
                    
                    if 0.0 <= win_prob <= 1.0:
                        print("  ✅ Probability is valid.")
                    else:
                        print("  ❌ Probability is OUT OF RANGE.")
                        
                except Exception as e:
                    print(f"  ❌ Error calculating probability: {e}")

    if not games_found:
        print("\n⚠️ No live games found on checked sports. Cannot verify with live data.")
        print("Tip: If you know a game is live, ensure the sport is in the check list.")

if __name__ == "__main__":
    asyncio.run(verify_live_games())
