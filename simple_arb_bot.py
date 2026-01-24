"""
SIMPLE ARBITRAGE BOT MVP

This is a minimal viable product that demonstrates the core arbitrage logic
BEFORE building out the full microservices architecture.

Run this to test if arbitrage detection works in production.

Usage:
    python simple_arb_bot.py

What it does:
1. Connects to Kalshi and Polymarket via WebSocket (10-50ms latency)
2. Auto-discovers markets for live NFL/NBA games
3. Scans for arbitrage opportunities every second
4. Logs opportunities (doesn't execute - add that once proven)
"""

import asyncio
import json
import logging
import os
import sys
from datetime import datetime
from pathlib import Path

# Add project root to path
sys.path.insert(0, str(Path(__file__).parent.parent))

from markets.kalshi.client import KalshiClient
from markets.kalshi.websocket import KalshiWebSocketClient
from markets.polymarket.client import PolymarketClient
from markets.polymarket.websocket import PolymarketWebSocketClient
# NOTE: MarketDiscoveryService removed - use Rust market_discovery_rust service instead
# from services.market_discovery import MarketDiscoveryService
from arbees_shared.models.game import Sport, GameState
from arbees_shared.models.market import Platform, MarketPrice
import arbees_core

# DEPRECATED: This standalone bot needs updating to use the Rust market discovery service
MarketDiscoveryService = None  # Placeholder - this script needs refactoring

logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


class SimpleArbBot:
    """
    Simple arbitrage bot for testing core logic.
    
    This is a PROOF OF CONCEPT - once this works, migrate to full architecture.
    """
    
    def __init__(self):
        # Market clients
        self.kalshi_rest = None
        self.kalshi_ws = None
        self.poly_rest = None
        self.poly_ws = None
        
        # Market discovery
        self.discovery = None
        
        # State
        self.market_prices = {}  # {market_id: MarketPrice}
        self.monitored_markets = {}  # {game_id: {Platform: market_id}}
        self.opportunities_found = 0
        
    async def start(self):
        """Start the bot."""
        logger.info("=" * 80)
        logger.info("SIMPLE ARBITRAGE BOT MVP - STARTING")
        logger.info("=" * 80)
        
        # Get API keys from environment
        kalshi_key = os.environ.get("KALSHI_API_KEY")
        if not kalshi_key:
            logger.error("KALSHI_API_KEY not set in environment")
            return
        
        # Connect to REST clients
        logger.info("Connecting to REST clients...")
        self.kalshi_rest = KalshiClient()
        await self.kalshi_rest.connect()
        
        self.poly_rest = PolymarketClient()
        await self.poly_rest.connect()
        
        # Connect to WebSocket clients
        logger.info("Connecting to WebSocket clients...")
        self.kalshi_ws = KalshiWebSocketClient(api_key=kalshi_key)
        await self.kalshi_ws.connect()
        
        self.poly_ws = PolymarketWebSocketClient()
        await self.poly_ws.connect()
        
        # Initialize market discovery
        self.discovery = MarketDiscoveryService(
            kalshi_client=self.kalshi_rest,
            polymarket_client=self.poly_rest,
        )
        
        logger.info("✓ All clients connected")
        
        # Discover markets for live games
        await self.discover_live_games()
        
        # Start price streaming
        asyncio.create_task(self.stream_kalshi_prices())
        asyncio.create_task(self.stream_poly_prices())
        
        # Start arbitrage scanner
        asyncio.create_task(self.scan_for_arbitrage())
        
        # Keep running
        logger.info("Bot is now running. Press Ctrl+C to stop.")
        while True:
            await asyncio.sleep(60)
            logger.info(
                f"Status: Monitoring {len(self.monitored_markets)} games, "
                f"{len(self.market_prices)} markets, "
                f"{self.opportunities_found} opportunities found"
            )
    
    async def discover_live_games(self):
        """Discover markets for live NFL/NBA games."""
        logger.info("Discovering markets for live games...")
        
        # For MVP, we'll just search for today's NFL/NBA markets
        # In production, this would query ESPN for live games
        
        # Example: Create dummy game states for testing
        test_games = [
            GameState(
                game_id="test-nfl-1",
                sport=Sport.NFL,
                home_team="Kansas City Chiefs",
                away_team="Buffalo Bills",
                home_score=0,
                away_score=0,
                period=1,
                time_remaining=900,
            ),
        ]
        
        for game in test_games:
            markets = await self.discovery.find_markets_for_game(
                game,
                platforms=[Platform.KALSHI, Platform.POLYMARKET],
            )
            
            if markets:
                self.monitored_markets[game.game_id] = markets
                logger.info(f"Found markets for {game.away_team} @ {game.home_team}:")
                for platform, market_id in markets.items():
                    if market_id:
                        logger.info(f"  - {platform.value}: {market_id}")
        
        # Subscribe to WebSockets
        await self.subscribe_to_markets()
    
    async def subscribe_to_markets(self):
        """Subscribe to WebSocket streams for discovered markets."""
        kalshi_markets = []
        poly_markets = []
        
        for game_id, markets in self.monitored_markets.items():
            if Platform.KALSHI in markets and markets[Platform.KALSHI]:
                kalshi_markets.append(markets[Platform.KALSHI])
            
            if Platform.POLYMARKET in markets and markets[Platform.POLYMARKET]:
                # For Polymarket, we need to get token_id
                market_id = markets[Platform.POLYMARKET]
                market = await self.poly_rest.get_market(market_id)
                if market:
                    token_id = await self.poly_rest.resolve_yes_token_id(market)
                    if token_id:
                        poly_markets.append({
                            "token_id": token_id,
                            "condition_id": market_id,
                            "title": market.get("question", ""),
                            "game_id": game_id,
                        })
        
        if kalshi_markets:
            await self.kalshi_ws.subscribe(kalshi_markets)
            logger.info(f"Subscribed to {len(kalshi_markets)} Kalshi markets via WebSocket")
        
        if poly_markets:
            await self.poly_ws.subscribe_with_metadata(poly_markets)
            logger.info(f"Subscribed to {len(poly_markets)} Polymarket markets via WebSocket")
    
    async def stream_kalshi_prices(self):
        """Stream Kalshi prices via WebSocket."""
        try:
            async for price in self.kalshi_ws.stream_prices():
                self.market_prices[f"{Platform.KALSHI.value}:{price.market_id}"] = price
                logger.debug(
                    f"Kalshi price update: {price.market_id} "
                    f"(bid: {price.yes_bid:.3f}, ask: {price.yes_ask:.3f})"
                )
        except Exception as e:
            logger.error(f"Kalshi WebSocket stream error: {e}")
    
    async def stream_poly_prices(self):
        """Stream Polymarket prices via WebSocket."""
        try:
            async for price in self.poly_ws.stream_prices():
                self.market_prices[f"{Platform.POLYMARKET.value}:{price.market_id}"] = price
                logger.debug(
                    f"Polymarket price update: {price.market_id} "
                    f"(bid: {price.yes_bid:.3f}, ask: {price.yes_ask:.3f})"
                )
        except Exception as e:
            logger.error(f"Polymarket WebSocket stream error: {e}")
    
    async def scan_for_arbitrage(self):
        """Scan for arbitrage opportunities."""
        while True:
            try:
                # Group prices by game
                game_prices = {}
                for game_id, markets in self.monitored_markets.items():
                    prices = []
                    for platform, market_id in markets.items():
                        key = f"{platform.value}:{market_id}"
                        if key in self.market_prices:
                            prices.append(self.market_prices[key])
                    
                    if len(prices) >= 2:
                        game_prices[game_id] = prices
                
                # Scan each game
                for game_id, prices in game_prices.items():
                    # Check cross-platform arbitrage
                    for i in range(len(prices)):
                        for j in range(i + 1, len(prices)):
                            opps = arbees_core.find_cross_market_arbitrage(
                                prices[i],
                                prices[j],
                                game_id,
                                Sport.NFL,  # Should get from game state
                                "Test Game",
                            )
                            
                            for opp in opps:
                                self.opportunities_found += 1
                                logger.warning(
                                    f"🎯 ARBITRAGE FOUND: {opp.description} "
                                    f"(edge: {opp.edge_pct:.2f}%)"
                                )
                    
                    # Check same-platform arbitrage
                    for price in prices:
                        opp = arbees_core.find_same_platform_arbitrage(
                            price,
                            game_id,
                            Sport.NFL,
                            "Test Game",
                        )
                        if opp:
                            self.opportunities_found += 1
                            logger.warning(
                                f"🎯 SAME-PLATFORM ARBITRAGE: {opp.description} "
                                f"(edge: {opp.edge_pct:.2f}%)"
                            )
                
            except Exception as e:
                logger.error(f"Error in arbitrage scanner: {e}")
            
            await asyncio.sleep(1)  # Scan every second
    
    async def stop(self):
        """Stop the bot."""
        logger.info("Stopping bot...")
        
        if self.kalshi_ws:
            await self.kalshi_ws.disconnect()
        if self.poly_ws:
            await self.poly_ws.disconnect()
        if self.kalshi_rest:
            await self.kalshi_rest.disconnect()
        if self.poly_rest:
            await self.poly_rest.disconnect()
        
        logger.info("Bot stopped")


async def main():
    """Main entry point."""
    bot = SimpleArbBot()
    
    try:
        await bot.start()
    except KeyboardInterrupt:
        logger.info("Received interrupt signal")
    finally:
        await bot.stop()


if __name__ == "__main__":
    asyncio.run(main())
