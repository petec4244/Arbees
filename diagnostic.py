"""
Arbees Diagnostic Script

Run this to check if everything is configured correctly.
"""

import asyncio
import os
import sys
from pathlib import Path


async def diagnose():
    print("=" * 80)
    print(" ARBEES DIAGNOSTIC TOOL")
    print("=" * 80)
    print()
    
    errors = []
    warnings = []
    
    # 1. Check credentials
    print("1. Checking Credentials...")
    print("-" * 80)
    
    kalshi_key = os.environ.get("KALSHI_API_KEY")
    kalshi_pkey_path = os.environ.get("KALSHI_PRIVATE_KEY_PATH")
    poly_key = os.environ.get("POLYMARKET_PRIVATE_KEY")
    
    if kalshi_key:
        print(f"  ✓ KALSHI_API_KEY is set ({kalshi_key[:10]}...)")
    else:
        print("  ✗ KALSHI_API_KEY is NOT set")
        errors.append("KALSHI_API_KEY environment variable missing")
    
    if kalshi_pkey_path:
        print(f"  ✓ KALSHI_PRIVATE_KEY_PATH is set: {kalshi_pkey_path}")
        if Path(kalshi_pkey_path).exists():
            print(f"    ✓ Private key file exists")
        else:
            print(f"    ✗ Private key file NOT found!")
            errors.append(f"Private key file not found: {kalshi_pkey_path}")
    else:
        print("  ✗ KALSHI_PRIVATE_KEY_PATH is NOT set")
        errors.append("KALSHI_PRIVATE_KEY_PATH environment variable missing")
    
    if poly_key:
        print(f"  ✓ POLYMARKET_PRIVATE_KEY is set ({poly_key[:10]}...)")
    else:
        print("  ✗ POLYMARKET_PRIVATE_KEY is NOT set")
        warnings.append("POLYMARKET_PRIVATE_KEY missing (optional for read-only)")
    
    print()
    
    # 2. Check Rust core
    print("2. Checking Rust Core...")
    print("-" * 80)
    
    try:
        import arbees_core
        print("  ✓ Rust core (arbees_core) imported successfully")
        
        # Try to use it
        try:
            sport = arbees_core.Sport.NBA
            print(f"  ✓ Rust core is functional (Sport.NBA = {sport})")
        except Exception as e:
            print(f"  ⚠ Rust core imported but not functional: {e}")
            warnings.append(f"Rust core error: {e}")
    except ImportError as e:
        print(f"  ✗ Rust core import failed: {e}")
        errors.append("Rust core not built. Run: cd arbees_core && maturin develop --release")
    
    print()
    
    # 3. Check for live games
    print("3. Checking for Live Games...")
    print("-" * 80)
    
    try:
        from data_providers.espn.client import ESPNClient
        from arbees_shared.models.game import Sport
        
        # Check NBA
        print("  Checking NBA...")
        espn = ESPNClient(Sport.NBA)
        await espn.connect()
        games = await espn.get_live_games()
        print(f"    ✓ Found {len(games)} live NBA games")
        
        if games:
            game_id = games[0]
            state, _ = await espn.poll_game(game_id, None)
            if state:
                print(f"    Example: {state.away_team} @ {state.home_team}")
                print(f"    Score: {state.away_score}-{state.home_score}, {state.status}")
        else:
            print("    ℹ No live NBA games right now")
        
        await espn.disconnect()
        
    except Exception as e:
        print(f"  ✗ ESPN check failed: {e}")
        errors.append(f"ESPN client error: {e}")
    
    print()
    
    # 4. Check Kalshi client
    print("4. Checking Kalshi Client...")
    print("-" * 80)
    
    try:
        from markets.kalshi.client import KalshiClient
        
        if kalshi_key and kalshi_pkey_path:
            kalshi = KalshiClient()
            await kalshi.connect()
            print("  ✓ Kalshi REST client connected")
            
            # Try to get markets
            try:
                markets = await kalshi.get_markets(status="open", limit=5)
                if markets:
                    print(f"  ✓ Retrieved {len(markets)} sample markets")
                else:
                    print("  ⚠ No markets returned (might be OK)")
            except Exception as e:
                print(f"  ⚠ Could not retrieve markets: {e}")
            
            await kalshi.disconnect()
        else:
            print("  ⚠ Skipping (credentials not set)")
            
    except Exception as e:
        print(f"  ✗ Kalshi client failed: {e}")
        errors.append(f"Kalshi client error: {e}")
    
    print()
    
    # 5. Check Polymarket client
    print("5. Checking Polymarket Client...")
    print("-" * 80)
    
    try:
        from markets.polymarket.client import PolymarketClient
        
        poly = PolymarketClient()
        await poly.connect()
        print("  ✓ Polymarket REST client connected")
        
        # Try to get markets
        try:
            markets = await poly.get_markets(limit=5)
            if markets:
                print(f"  ✓ Retrieved {len(markets)} sample markets")
            else:
                print("  ⚠ No markets returned")
        except Exception as e:
            print(f"  ⚠ Could not retrieve markets: {e}")
        
        await poly.disconnect()
        
    except Exception as e:
        print(f"  ✗ Polymarket client failed: {e}")
        errors.append(f"Polymarket client error: {e}")
    
    print()
    
    # 6. Check WebSocket clients
    print("6. Checking WebSocket Clients...")
    print("-" * 80)
    
    try:
        from markets.kalshi.websocket.ws_client import KalshiWebSocketClient
        print("  ✓ Kalshi WebSocket client can be imported")
    except ImportError as e:
        print(f"  ✗ Kalshi WebSocket import failed: {e}")
        errors.append("Kalshi WebSocket client missing")
    
    try:
        from markets.polymarket.websocket.ws_client import PolymarketWebSocketClient
        print("  ✓ Polymarket WebSocket client can be imported")
    except ImportError as e:
        print(f"  ✗ Polymarket WebSocket import failed: {e}")
        errors.append("Polymarket WebSocket client missing")
    
    print()
    
    # 7. Check database (if configured)
    print("7. Checking Database Connection...")
    print("-" * 80)
    
    try:
        from arbees_shared.db.connection import get_pool, DatabaseClient
        
        pool = await get_pool()
        db = DatabaseClient(pool)
        
        result = await db.pool.fetch("SELECT 1 as test")
        print(f"  ✓ Database connected successfully: {result}")
        
        await pool.close()
        
    except Exception as e:
        print(f"  ⚠ Database check failed: {e}")
        warnings.append(f"Database not configured (optional): {e}")
    
    print()
    
    # Summary
    print("=" * 80)
    print(" SUMMARY")
    print("=" * 80)
    print()
    
    if not errors and not warnings:
        print("🎉 All checks passed! Your system is ready to trade.")
        print()
        print("Next steps:")
        print("  1. Start the bot: python simple_arb_bot.py")
        print("  2. Or run GameShard: python -m services.game_shard.shard")
        print()
    else:
        if errors:
            print(f"❌ {len(errors)} CRITICAL ERROR(S):")
            for i, error in enumerate(errors, 1):
                print(f"  {i}. {error}")
            print()
        
        if warnings:
            print(f"⚠️  {len(warnings)} WARNING(S):")
            for i, warning in enumerate(warnings, 1):
                print(f"  {i}. {warning}")
            print()
        
        if errors:
            print("Fix the errors above before running the bot.")
            return False
        else:
            print("Warnings can be ignored if you know what you're doing.")
            return True
    
    return True


if __name__ == "__main__":
    success = asyncio.run(diagnose())
    sys.exit(0 if success else 1)
