"""
ZMQ bridge for crypto prices: polymarket (VPN) → arbees-network

This bridge subscribes to polymarket prices from the VPN-isolated polymarket_monitor
and re-publishes them to the arbees-network where crypto_shard can reach them.

Purpose:
- polymarket_monitor runs behind VPN (network_mode: "service:vpn")
- Publishes to tcp://polymarket_monitor:5556 (unreachable from arbees-network)
- This bridge receives from VPN network and republishes on arbees-network
- crypto_shard subscribes to this bridge (tcp://crypto-zmq-bridge:5564)

ZMQ connections:
- SUB: tcp://vpn:5556 (Polymarket on VPN network - HOST perspective)
- PUB: tcp://0.0.0.0:5564 (arbees-network - available to crypto_shard)
"""

import asyncio
import json
import logging
import os
import signal
import sys

import zmq
import zmq.asyncio

# Configure logging
logging.basicConfig(
    level=os.environ.get("LOG_LEVEL", "INFO"),
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)


class CryptoZmqBridge:
    """Bridge polymarket ZMQ (VPN) to arbees-network"""

    def __init__(self):
        self.zmq_context = zmq.asyncio.Context()
        self.sub_socket = None
        self.pub_socket = None

        # Connection endpoints
        self.poly_zmq_endpoint = os.environ.get("POLYMARKET_ZMQ_ENDPOINT", "tcp://vpn:5556")
        self.bridge_pub_endpoint = os.environ.get("BRIDGE_PUB_ENDPOINT", "tcp://0.0.0.0:5564")

        # Statistics
        self.messages_received = 0
        self.messages_published = 0
        self.start_time = None

        # Running flag
        self.running = False

    async def start(self):
        """Start the bridge"""
        logger.info("Starting Crypto ZMQ Bridge...")
        logger.info(f"  SUB: {self.poly_zmq_endpoint} (polymarket on VPN)")
        logger.info(f"  PUB: {self.bridge_pub_endpoint} (arbees-network)")

        try:
            # Create SUB socket (subscribe to polymarket)
            self.sub_socket = self.zmq_context.socket(zmq.SUB)
            self.sub_socket.connect(self.poly_zmq_endpoint)
            self.sub_socket.subscribe(b"prices.poly")
            logger.info(f"Connected to polymarket ZMQ on {self.poly_zmq_endpoint}")

            # Create PUB socket (publish to crypto_shard)
            self.pub_socket = self.zmq_context.socket(zmq.PUB)
            self.pub_socket.bind(self.bridge_pub_endpoint)
            logger.info(f"Bound PUB socket to {self.bridge_pub_endpoint}")

            # Allow brief startup time for PUB socket to bind
            await asyncio.sleep(0.5)

            self.running = True
            self.start_time = asyncio.get_event_loop().time()

            # Setup signal handlers for graceful shutdown
            loop = asyncio.get_event_loop()
            for sig in (signal.SIGINT, signal.SIGTERM):
                try:
                    loop.add_signal_handler(sig, lambda: asyncio.create_task(self.stop()))
                except NotImplementedError:
                    pass  # Windows

            logger.info("Bridge started. Forwarding prices...")

            # Start message forwarding loop
            await self._forward_loop()

        except Exception as e:
            logger.error(f"Failed to start bridge: {e}", exc_info=True)
            raise

    async def _forward_loop(self):
        """Main loop: receive from polymarket, republish to arbees-network"""
        logger.info("Entering forward loop...")

        while self.running:
            try:
                # Receive multipart message from polymarket
                msg = await asyncio.wait_for(
                    self.sub_socket.recv_multipart(),
                    timeout=30.0  # 30 second timeout
                )

                self.messages_received += 1

                # Forward the multipart message as-is
                await self.pub_socket.send_multipart(msg)
                self.messages_published += 1

                # Log every 100 messages
                if self.messages_received % 100 == 0:
                    uptime = asyncio.get_event_loop().time() - self.start_time
                    rate = self.messages_published / uptime if uptime > 0 else 0
                    logger.info(
                        f"Bridge stats: {self.messages_published} published "
                        f"({rate:.1f} msg/sec), {uptime:.1f}s uptime"
                    )

            except asyncio.TimeoutError:
                # No message in 30s - log and continue
                logger.debug("No message received in 30s (normal idle)")

            except Exception as e:
                if self.running:
                    logger.error(f"Forward loop error: {e}", exc_info=True)
                    await asyncio.sleep(1)
                else:
                    break

        logger.info(f"Forward loop ended. Total messages published: {self.messages_published}")

    async def stop(self):
        """Stop the bridge and cleanup"""
        logger.info("Stopping bridge...")
        self.running = False

        if self.sub_socket:
            self.sub_socket.close()
        if self.pub_socket:
            self.pub_socket.close()
        if self.zmq_context:
            self.zmq_context.term()

        logger.info(
            f"Bridge stopped. Statistics: "
            f"{self.messages_received} received, "
            f"{self.messages_published} published"
        )


async def main():
    """Entry point"""
    logger.info("Crypto ZMQ Bridge initializing...")
    bridge = CryptoZmqBridge()

    try:
        await bridge.start()
    except KeyboardInterrupt:
        logger.info("Interrupted by user")
    except Exception as e:
        logger.error(f"Fatal error: {e}", exc_info=True)
        raise
    finally:
        await bridge.stop()


if __name__ == "__main__":
    asyncio.run(main())
