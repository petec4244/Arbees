Convert the Arbees prediction market clients from REST polling to WebSocket streaming for real-time orderbook and price updates.

## Current Architecture

The codebase has:
- `markets/base.py` - BaseMarketClient with REST polling, rate limiting, connection pooling
- `markets/kalshi/client.py` - KalshiClient with RSA-PSS auth, REST polling via `stream_prices()` 
- `markets/polymarket/client.py` - PolymarketClient with CLOB/Gamma APIs, REST polling

Current `stream_prices()` uses polling loops with `asyncio.sleep()` - inefficient and high latency.

## Target Architecture

Replace polling with persistent WebSocket connections that:
1. Receive push updates (orderbook snapshots, deltas, trades)
2. Auto-reconnect with exponential backoff
3. Maintain local orderbook state from deltas
4. Emit MarketPrice updates via async generator or callback

## Kalshi WebSocket API

**Endpoint:** `wss://api.elections.kalshi.com/trade-api/v1/ws`

**Authentication:** Same RSA-PSS signature in headers during connection upgrade

**Subscribe message:**
```json
{
  "id": 1,
  "cmd": "subscribe",
  "params": {
    "channels": ["orderbook_delta"],
    "market_tickers": ["KXNFL-24-KC-BUF"]
  }
}

Message types:

orderbook_snapshot - Full orderbook state (yes_bids, no_bids as [[price_cents, quantity], ...])
orderbook_delta - Incremental updates {market_ticker, price, delta, side}
Important: Kalshi only returns BIDS. YES ask at X = NO bid at (100-X).

Docs: https://docs.kalshi.com/websockets/orderbook-updates

Polymarket WebSocket API
Endpoint: wss://ws-subscriptions-clob.polymarket.com/ws/market

Subscribe message:

{
  "type": "subscribe",
  "channel": "market",
  "markets": ["0x1234...token_id"]  // YES token IDs
}

Message types:

book - Full orderbook snapshot
price_change - Price level updates (NOTE: schema changing Sept 15, 2025)
last_trade_price - Trade notifications
tick_size_change - Market parameter changes
Keep-alive: Send PING every 5 seconds

Docs: https://docs.polymarket.com/developers/CLOB/websocket/wss-overview

Implementation Requirements
1. New Base Class: BaseWebSocketClient
class BaseWebSocketClient(ABC):
    """Base class for WebSocket market clients."""
    
    def __init__(self, ws_url: str, platform: Platform):
        self.ws_url = ws_url
        self.platform = platform
        self._ws: Optional[aiohttp.ClientWebSocketResponse] = None
        self._orderbooks: dict[str, LocalOrderBook] = {}  # Local state
        self._reconnect_delay = 1.0
        self._max_reconnect_delay = 60.0
        self._running = False
        self._subscriptions: set[str] = set()
    
    @abstractmethod
    async def _authenticate(self) -> dict[str, str]:
        """Return auth headers for WS upgrade."""
        ...
    
    @abstractmethod
    async def _build_subscribe_message(self, market_ids: list[str]) -> dict:
        """Build platform-specific subscribe message."""
        ...
    
    @abstractmethod
    async def _handle_message(self, msg: dict) -> Optional[MarketPrice]:
        """Parse platform-specific message, update local orderbook, return price if changed."""
        ...
    
    async def connect(self) -> None:
        """Establish WebSocket connection with auth."""
        headers = await self._authenticate()
        session = aiohttp.ClientSession()
        self._ws = await session.ws_connect(self.ws_url, headers=headers)
        self._reconnect_delay = 1.0  # Reset on successful connect
    
    async def subscribe(self, market_ids: list[str]) -> None:
        """Subscribe to market updates."""
        msg = await self._build_subscribe_message(market_ids)
        await self._ws.send_json(msg)
        self._subscriptions.update(market_ids)
    
    async def unsubscribe(self, market_ids: list[str]) -> None:
        """Unsubscribe from markets."""
        # Platform-specific unsubscribe
        ...
    
    async def stream_prices(self, market_ids: list[str]) -> AsyncIterator[MarketPrice]:
        """Stream real-time price updates."""
        await self.connect()
        await self.subscribe(market_ids)
        self._running = True
        
        while self._running:
            try:
                msg = await self._ws.receive(timeout=30)
                
                if msg.type == aiohttp.WSMsgType.TEXT:
                    data = json.loads(msg.data)
                    price = await self._handle_message(data)
                    if price:
                        yield price
                        
                elif msg.type == aiohttp.WSMsgType.CLOSED:
                    await self._reconnect()
                    
                elif msg.type == aiohttp.WSMsgType.ERROR:
                    logger.error(f"WS error: {self._ws.exception()}")
                    await self._reconnect()
                    
            except asyncio.TimeoutError:
                # Send ping to keep alive
                await self._ws.ping()
    
    async def _reconnect(self) -> None:
        """Reconnect with exponential backoff."""
        await asyncio.sleep(self._reconnect_delay)
        self._reconnect_delay = min(self._reconnect_delay * 2, self._max_reconnect_delay)
        
        await self.connect()
        # Resubscribe to all markets
        if self._subscriptions:
            await self.subscribe(list(self._subscriptions))

2. Local Orderbook State
@dataclass
class LocalOrderBook:
    """Maintains orderbook state from deltas."""
    market_id: str
    yes_bids: dict[int, float]  # price_cents -> quantity
    yes_asks: dict[int, float]
    last_update: datetime
    
    def apply_delta(self, price: int, delta: float, side: str) -> None:
        """Apply incremental update."""
        book = self.yes_bids if side == "yes" else self.yes_asks
        if delta == 0:
            book.pop(price, None)
        else:
            book[price] = delta
        self.last_update = datetime.utcnow()
    
    def apply_snapshot(self, yes_bids: list, no_bids: list) -> None:
        """Replace state from snapshot."""
        self.yes_bids = {b[0]: b[1] for b in yes_bids}
        # NO bids at X = YES asks at (100 - X)
        self.yes_asks = {100 - b[0]: b[1] for b in no_bids}
        self.last_update = datetime.utcnow()
    
    def to_market_price(self) -> MarketPrice:
        """Convert to MarketPrice model."""
        best_bid = max(self.yes_bids.keys()) / 100 if self.yes_bids else 0.0
        best_ask = min(self.yes_asks.keys()) / 100 if self.yes_asks else 1.0
        return MarketPrice(
            market_id=self.market_id,
            yes_bid=best_bid,
            yes_ask=best_ask,
            ...
        )

3. KalshiWebSocketClient
class KalshiWebSocketClient(BaseWebSocketClient):
    WS_URL = "wss://api.elections.kalshi.com/trade-api/v1/ws"
    
    def __init__(self, api_key: str, private_key: RSAPrivateKey):
        super().__init__(self.WS_URL, Platform.KALSHI)
        self.api_key = api_key
        self._private_key = private_key
    
    async def _authenticate(self) -> dict[str, str]:
        timestamp_ms = int(time.time() * 1000)
        signature = self._generate_signature(timestamp_ms, "GET", "/trade-api/v1/ws")
        return {
            "KALSHI-ACCESS-KEY": self.api_key,
            "KALSHI-ACCESS-TIMESTAMP": str(timestamp_ms),
            "KALSHI-ACCESS-SIGNATURE": signature,
        }
    
    async def _build_subscribe_message(self, market_ids: list[str]) -> dict:
        return {
            "id": 1,
            "cmd": "subscribe",
            "params": {
                "channels": ["orderbook_delta"],
                "market_tickers": market_ids
            }
        }
    
    async def _handle_message(self, msg: dict) -> Optional[MarketPrice]:
        msg_type = msg.get("type")
        
        if msg_type == "orderbook_snapshot":
            market_id = msg["market_ticker"]
            book = self._orderbooks.setdefault(market_id, LocalOrderBook(market_id))
            book.apply_snapshot(msg["yes"], msg["no"])
            return book.to_market_price()
            
        elif msg_type == "orderbook_delta":
            market_id = msg["market_ticker"]
            book = self._orderbooks.get(market_id)
            if book:
                book.apply_delta(msg["price"], msg["delta"], msg["side"])
                return book.to_market_price()
        
        return None

4. PolymarketWebSocketClient
class PolymarketWebSocketClient(BaseWebSocketClient):
    WS_URL = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
    
    def __init__(self, api_key: Optional[str] = None):
        super().__init__(self.WS_URL, Platform.POLYMARKET)
        self.api_key = api_key
        self._ping_task: Optional[asyncio.Task] = None
    
    async def _authenticate(self) -> dict[str, str]:
        # Polymarket WS doesn't require auth headers for market data
        return {}
    
    async def _build_subscribe_message(self, market_ids: list[str]) -> dict:
        # market_ids should be token_ids (resolve before calling)
        return {
            "type": "subscribe", 
            "channel": "market",
            "markets": market_ids
        }
    
    async def connect(self) -> None:
        await super().connect()
        # Start ping task for keep-alive
        self._ping_task = asyncio.create_task(self._ping_loop())
    
    async def _ping_loop(self) -> None:
        while self._running:
            await asyncio.sleep(5)
            if self._ws and not self._ws.closed:
                await self._ws.ping()
    
    async def _handle_message(self, msg: dict) -> Optional[MarketPrice]:
        msg_type = msg.get("type")
        
        if msg_type == "book":
            market_id = msg["market"]  # token_id
            book = self._orderbooks.setdefault(market_id, LocalOrderBook(market_id))
            book.apply_snapshot(
                [(b["price"], b["size"]) for b in msg["bids"]],
                [(a["price"], a["size"]) for a in msg["asks"]]
            )
            return book.to_market_price()
            
        elif msg_type == "price_change":
            # Handle incremental price updates
            market_id = msg["market"]
            book = self._orderbooks.get(market_id)
            if book:
                # Apply delta based on side
                ...
                return book.to_market_price()
        
        return None

5. Integration with GameShard
Update services/game_shard/shard.py to use WebSocket clients:

class GameShard:
    def __init__(self, ...):
        # Replace REST clients with WS clients
        self._kalshi_ws = KalshiWebSocketClient(api_key, private_key)
        self._polymarket_ws = PolymarketWebSocketClient()
        
        # Background tasks for each platform
        self._price_streams: dict[str, asyncio.Task] = {}
    
    async def _start_price_stream(self, platform: Platform, market_ids: list[str]):
        """Start WebSocket stream for a platform."""
        client = self._kalshi_ws if platform == Platform.KALSHI else self._polymarket_ws
        
        async for price in client.stream_prices(market_ids):
            # Publish to Redis
            await self._redis.publish(f"market:{price.market_id}:price", price.to_msgpack())
            
            # Check for arbitrage with current game state
            game_id = self._market_to_game.get(price.market_id)
            if game_id:
                await self._check_signals(game_id, price)

6. Hybrid Approach (Recommended)
Keep REST clients for:

Market discovery (get_markets, search_markets)
Historical data
Order placement
Use WebSocket for:

Real-time orderbook streaming
Price updates during live games
class HybridKalshiClient:
    """Combines REST for queries and WebSocket for streaming."""
    
    def __init__(self, ...):
        self._rest = KalshiClient(...)  # Existing REST client
        self._ws = KalshiWebSocketClient(...)  # New WS client
    
    # Delegate query methods to REST
    async def get_markets(self, ...) -> list[dict]:
        return await self._rest.get_markets(...)
    
    async def search_markets(self, ...) -> list[dict]:
        return await self._rest.search_markets(...)
    
    async def place_order(self, ...) -> dict:
        return await self._rest.place_order(...)
    
    # Use WebSocket for streaming
    async def stream_prices(self, market_ids: list[str]) -> AsyncIterator[MarketPrice]:
        async for price in self._ws.stream_prices(market_ids):
            yield price

Files to Create/Modify
CREATE markets/base_ws.py - BaseWebSocketClient class
CREATE markets/kalshi/ws_client.py - KalshiWebSocketClient
CREATE markets/polymarket/ws_client.py - PolymarketWebSocketClient
MODIFY markets/kalshi/client.py - Add HybridKalshiClient wrapper
MODIFY markets/polymarket/client.py - Add HybridPolymarketClient wrapper
MODIFY services/game_shard/shard.py - Use WebSocket streams instead of polling
Testing
Unit test LocalOrderBook delta application
Integration test WS connection/reconnection
Load test with 20 concurrent market subscriptions
Measure latency improvement vs REST polling
Expected Improvement
Metric	REST Polling	WebSocket
Latency	500-3000ms	10-50ms
API calls/min	400+	~10 (subscribe only)
Rate limit risk	High	Low

---

**Sources:**
- [Kalshi WebSocket Connection](https://docs.kalshi.com/websockets/websocket-connection)
- [Kalshi Orderbook Updates](https://docs.kalshi.com/websockets/orderbook-updates)
- [Polymarket WSS Overview](https://docs.polymarket.com/developers/CLOB/websocket/wss-overview)
- [Polymarket Market Channel](https://docs.polymarket.com/developers/CLOB/websocket/market-channel)