"""
Crypto Spot Price Monitor

Real-time crypto spot price monitoring via Coinbase and Binance WebSocket feeds.
Publishes prices to ZMQ for crypto_shard_rust consumption.

Serves as the single source of truth for spot prices in the arbitrage system.
Enables spot-vs-prediction-market arbitrage by providing real-time spot prices.

Usage:
    python monitor.py

Environment Variables:
    ZMQ_PUB_ENDPOINT: ZMQ publishing endpoint (default: tcp://*:5560)
    MONITORED_ASSETS: Comma-separated assets to monitor (default: BTC,ETH,SOL)
    LOG_LEVEL: Logging level (default: INFO)
"""

import asyncio
import json
import logging
import os
import time
from datetime import datetime
from typing import Dict, Optional

import zmq
import zmq.asyncio
from websockets import connect

# Configure logging
LOG_LEVEL = os.getenv("LOG_LEVEL", "INFO")
logging.basicConfig(
    level=getattr(logging, LOG_LEVEL),
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)


class CryptoSpotMonitor:
    """Real-time crypto spot price monitor via Coinbase and Binance WebSocket"""

    def __init__(self):
        self.zmq_endpoint = os.getenv("ZMQ_PUB_ENDPOINT", "tcp://*:5560")
        self.monitored_assets = os.getenv("MONITORED_ASSETS", "BTC,ETH,SOL").split(",")
        self.monitored_assets = [a.strip().upper() for a in self.monitored_assets]

        # ZMQ setup
        self.zmq_context = zmq.asyncio.Context()
        self.zmq_pub = self.zmq_context.socket(zmq.PUB)
        self.zmq_pub.bind(self.zmq_endpoint)

        # WebSocket endpoints
        self.coinbase_ws = "wss://ws-feed.exchange.coinbase.com"
        self.binance_ws = "wss://stream.binance.us:9443/ws"

        # Sequence counter for ZMQ messages
        self.seq = 0

        # Price cache for statistics
        self.prices: Dict[str, Dict] = {}

        logger.info(f"CryptoSpotMonitor initialized")
        logger.info(f"  ZMQ endpoint: {self.zmq_endpoint}")
        logger.info(f"  Monitored assets: {', '.join(self.monitored_assets)}")

    async def start(self):
        """Start monitoring all sources in parallel"""
        logger.info("Starting crypto spot monitoring...")

        try:
            await asyncio.gather(
                self._monitor_coinbase(),
                self._monitor_binance(),
                self._log_stats(),
            )
        except Exception as e:
            logger.error(f"Fatal error in monitor: {e}")
            raise

    async def _monitor_coinbase(self):
        """Subscribe to Coinbase WebSocket for BTC, ETH, SOL spot prices"""
        subscribe_message = {
            "type": "subscribe",
            "product_ids": [f"{asset}-USD" for asset in self.monitored_assets],
            "channels": ["ticker"],
        }

        while True:
            try:
                async with connect(self.coinbase_ws) as ws:
                    await ws.send(json.dumps(subscribe_message))
                    logger.info("Connected to Coinbase WebSocket")

                    async for message in ws:
                        try:
                            data = json.loads(message)

                            # Process ticker updates
                            if data.get("type") == "ticker":
                                await self._publish_spot_price(
                                    source="coinbase",
                                    product_id=data["product_id"],  # e.g., "BTC-USD"
                                    price=float(data["price"]),
                                    volume_24h=float(data.get("volume_24h", 0)),
                                    timestamp=data["time"],
                                )
                        except (json.JSONDecodeError, KeyError, ValueError) as e:
                            logger.debug(f"Error parsing Coinbase message: {e}")
                            continue

            except asyncio.CancelledError:
                logger.info("Coinbase monitor cancelled")
                break
            except Exception as e:
                logger.error(f"Coinbase WebSocket error: {e}")
                await asyncio.sleep(5)  # Reconnect after 5 seconds

    async def _monitor_binance(self):
        """Subscribe to Binance WebSocket for BTC, ETH, SOL spot prices"""
        # Binance uses separate streams per symbol
        streams = [f"{asset.lower()}usdt@ticker" for asset in self.monitored_assets]
        stream_url = f"{self.binance_ws}/{'/'.join(streams)}"

        while True:
            try:
                async with connect(stream_url) as ws:
                    logger.info("Connected to Binance WebSocket")

                    async for message in ws:
                        try:
                            data = json.loads(message)

                            # Process 24hr ticker updates
                            if data.get("e") == "24hrTicker":
                                symbol = data["s"]  # e.g., "BTCUSDT"
                                asset = symbol.replace("USDT", "")
                                await self._publish_spot_price(
                                    source="binance",
                                    product_id=f"{asset}-USDT",
                                    price=float(data["c"]),  # Last price
                                    volume_24h=float(data.get("v", 0)),
                                    timestamp=data["E"],  # Event time (ms)
                                )
                        except (json.JSONDecodeError, KeyError, ValueError) as e:
                            logger.debug(f"Error parsing Binance message: {e}")
                            continue

            except asyncio.CancelledError:
                logger.info("Binance monitor cancelled")
                break
            except Exception as e:
                logger.error(f"Binance WebSocket error: {e}")
                await asyncio.sleep(5)

    async def _publish_spot_price(
        self,
        source: str,
        product_id: str,
        price: float,
        volume_24h: float,
        timestamp: str,
    ):
        """
        Publish spot price to ZMQ in crypto_shard expected format.

        This format matches IncomingCryptoPrice from crypto_shard_rust.
        """
        # Extract asset from product_id (e.g., "BTC-USD" -> "BTC")
        asset = product_id.split("-")[0].upper()

        if asset not in self.monitored_assets:
            return

        # Format message matching crypto_shard_rust expectations
        payload = {
            "market_id": f"SPOT_{asset}_USD",
            "platform": source,  # "coinbase" or "binance"
            "asset": asset,  # "BTC", "ETH", "SOL"
            "yes_bid": price,  # For spot, bid/ask/mid all same
            "yes_ask": price,
            "mid_price": price,
            "yes_bid_size": None,
            "yes_ask_size": None,
            "liquidity": volume_24h,
            "timestamp": timestamp,
        }

        # Wrap in ZMQ envelope for consistency
        envelope = {
            "seq": self.seq,
            "timestamp_ms": int(time.time() * 1000),
            "source": f"crypto_spot_monitor:{source}",
            "payload": payload,
        }

        # ZMQ topic format: crypto.prices.{ASSET}
        topic = f"crypto.prices.{asset.lower()}".encode()
        message = json.dumps(envelope).encode()

        await self.zmq_pub.send_multipart([topic, message])

        self.seq += 1

        # Update price cache for stats
        self.prices[asset] = {
            "price": price,
            "source": source,
            "timestamp": datetime.now().isoformat(),
        }

        if self.seq % 100 == 0:
            logger.info(
                f"Published {self.seq} spot prices | "
                f"{asset}@{source}: ${price:.2f}"
            )

    async def _log_stats(self):
        """Log statistics every 60 seconds"""
        while True:
            await asyncio.sleep(60)
            if self.prices:
                logger.info(f"Price cache snapshot: {json.dumps(self.prices, indent=2)}")


async def main():
    """Main entry point"""
    monitor = CryptoSpotMonitor()
    try:
        await monitor.start()
    except KeyboardInterrupt:
        logger.info("Shutting down...")
    except Exception as e:
        logger.error(f"Monitor failed: {e}")
        raise


if __name__ == "__main__":
    asyncio.run(main())
