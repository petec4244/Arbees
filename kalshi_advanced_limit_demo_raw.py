"""
Kalshi API Advanced Limit Application - NYC Weather Market (Raw HTTP Version)

This script demonstrates direct HTTP API interaction without the SDK:
1. Querying API for today's NYC weather market data
2. Fetching the orderbook for that market
3. Placing and canceling an order of 1 unit

This version uses raw HTTP requests to show understanding of the API protocol.

Requirements:
- requests library (pip install requests)
- cryptography library for API signing (pip install cryptography)
"""

import hashlib
import json
import os
import time
from base64 import b64encode
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional, Tuple

import requests
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding
from cryptography.hazmat.backends import default_backend


class KalshiAPIClient:
    """
    Raw HTTP client for Kalshi API with proper authentication.
    
    Demonstrates understanding of:
    - API authentication (API key + RSA signature)
    - Request signing
    - Rate limiting
    - Error handling
    """
    
    BASE_URL = "https://trading-api.kalshi.com/trade-api/v2"
    
    def __init__(self, api_key: str, private_key_path: str):
        """
        Initialize Kalshi API client.
        
        Args:
            api_key: Your Kalshi API key
            private_key_path: Path to your private key PEM file
        """
        self.api_key = api_key
        
        # Load private key for request signing
        with open(private_key_path, 'rb') as f:
            self.private_key = serialization.load_pem_private_key(
                f.read(),
                password=None,
                backend=default_backend()
            )
        
        self.session = requests.Session()
        print("✓ Kalshi API client initialized (raw HTTP)")
    
    def _generate_signature(self, timestamp: str, method: str, path: str, body: str = "") -> str:
        """
        Generate RSA signature for API request.
        
        Args:
            timestamp: Unix timestamp as string
            method: HTTP method (GET, POST, DELETE, etc.)
            path: API path (e.g., "/markets")
            body: JSON body as string (empty for GET requests)
            
        Returns:
            Base64-encoded signature
        """
        # Construct the message to sign
        # Format: timestamp + method + path + body
        message = f"{timestamp}{method}{path}{body}"
        
        # Sign with private key
        signature = self.private_key.sign(
            message.encode('utf-8'),
            padding.PKCS1v15(),
            hashes.SHA256()
        )
        
        # Return base64-encoded signature
        return b64encode(signature).decode('utf-8')
    
    def _make_request(
        self,
        method: str,
        path: str,
        params: Optional[dict] = None,
        json_data: Optional[dict] = None,
    ) -> Tuple[Optional[dict], Optional[str]]:
        """
        Make authenticated API request.
        
        Args:
            method: HTTP method
            path: API path
            params: Query parameters
            json_data: JSON body for POST/PUT requests
            
        Returns:
            (response_data, error_message)
        """
        url = f"{self.BASE_URL}{path}"
        
        # Generate timestamp
        timestamp = str(int(time.time()))
        
        # Prepare body
        body = json.dumps(json_data) if json_data else ""
        
        # Generate signature
        signature = self._generate_signature(timestamp, method, path, body)
        
        # Set headers
        headers = {
            "Content-Type": "application/json",
            "KALSHI-ACCESS-KEY": self.api_key,
            "KALSHI-ACCESS-SIGNATURE": signature,
            "KALSHI-ACCESS-TIMESTAMP": timestamp,
        }
        
        try:
            # Make request
            response = self.session.request(
                method=method,
                url=url,
                headers=headers,
                params=params,
                json=json_data,
                timeout=30,
            )
            
            # Check for errors
            if response.status_code >= 400:
                error_msg = f"HTTP {response.status_code}: {response.text}"
                return None, error_msg
            
            # Parse JSON response
            try:
                return response.json(), None
            except:
                return {"raw": response.text}, None
                
        except Exception as e:
            return None, str(e)
    
    def get_markets(self, **filters) -> Optional[list]:
        """Get markets with optional filters."""
        data, error = self._make_request("GET", "/markets", params=filters)
        
        if error:
            print(f"Error fetching markets: {error}")
            return None
        
        return data.get('markets', []) if data else []
    
    def get_market_orderbook(self, ticker: str, depth: int = 5) -> Optional[dict]:
        """Get orderbook for a specific market."""
        data, error = self._make_request(
            "GET",
            f"/markets/{ticker}/orderbook",
            params={"depth": depth}
        )
        
        if error:
            print(f"Error fetching orderbook: {error}")
            return None
        
        return data.get('orderbook') if data else None
    
    def create_order(
        self,
        ticker: str,
        action: str,
        side: str,
        count: int,
        order_type: str,
        yes_price: Optional[int] = None,
        no_price: Optional[int] = None,
        expiration_ts: Optional[int] = None,
    ) -> Optional[dict]:
        """Place an order."""
        order_data = {
            "ticker": ticker,
            "action": action,
            "side": side,
            "count": count,
            "type": order_type,
        }
        
        if yes_price is not None:
            order_data["yes_price"] = yes_price
        if no_price is not None:
            order_data["no_price"] = no_price
        if expiration_ts is not None:
            order_data["expiration_ts"] = expiration_ts
        
        data, error = self._make_request("POST", "/portfolio/orders", json_data=order_data)
        
        if error:
            print(f"Error creating order: {error}")
            return None
        
        return data.get('order') if data else None
    
    def cancel_order(self, order_id: str) -> Optional[dict]:
        """Cancel an order."""
        data, error = self._make_request("DELETE", f"/portfolio/orders/{order_id}")
        
        if error:
            print(f"Error canceling order: {error}")
            return None
        
        return data.get('order') if data else None
    
    def get_order(self, order_id: str) -> Optional[dict]:
        """Get order status."""
        data, error = self._make_request("GET", f"/portfolio/orders/{order_id}")
        
        if error:
            print(f"Error fetching order: {error}")
            return None
        
        return data.get('order') if data else None


class KalshiWeatherMarketDemo:
    """NYC Weather market demo using raw HTTP client."""
    
    def __init__(self, api_key: str, private_key_path: str):
        self.client = KalshiAPIClient(api_key, private_key_path)
    
    def find_nyc_weather_market(self) -> Optional[dict]:
        """Task 1: Find NYC weather market."""
        print("\n" + "=" * 80)
        print("TASK 1: Finding Today's NYC Weather Market (Raw HTTP)")
        print("=" * 80)
        
        # Query weather markets
        markets = self.client.get_markets(
            series_ticker="KXTEMP",
            status="open",
            limit=100
        )
        
        if not markets:
            print("✗ No markets returned from API")
            return None
        
        print(f"Found {len(markets)} weather markets")
        
        # Filter for NYC and today
        today = datetime.now().date()
        tomorrow = today + timedelta(days=1)
        
        for market in markets:
            title = market.get('title', '').lower()
            
            if 'nyc' not in title and 'new york' not in title:
                continue
            
            # Check date
            close_time = market.get('close_time')
            if close_time:
                close_date = datetime.fromisoformat(close_time.replace('Z', '+00:00')).date()
                if close_date < today or close_date > tomorrow:
                    continue
            
            print(f"\n✓ Found NYC Weather Market:")
            print(f"  Ticker: {market.get('ticker')}")
            print(f"  Title: {market.get('title')}")
            print(f"  Status: {market.get('status')}")
            print(f"  Volume: {market.get('volume', 'N/A')}")
            
            return market
        
        # Fallback: any NYC market
        for market in markets:
            if 'nyc' in market.get('title', '').lower():
                print(f"\n✓ Found NYC Weather Market (fallback):")
                print(f"  Ticker: {market.get('ticker')}")
                print(f"  Title: {market.get('title')}")
                return market
        
        print("✗ No NYC weather markets found")
        return None
    
    def get_market_orderbook(self, ticker: str) -> Optional[dict]:
        """Task 2: Get orderbook."""
        print("\n" + "=" * 80)
        print("TASK 2: Fetching Market Orderbook (Raw HTTP)")
        print("=" * 80)
        
        orderbook = self.client.get_market_orderbook(ticker, depth=5)
        
        if not orderbook:
            return None
        
        yes_bids = orderbook.get('yes', [])
        no_bids = orderbook.get('no', [])
        
        print(f"\n✓ Orderbook for {ticker}:")
        print("\nYES Side:")
        print("  Price  | Quantity")
        print("  -------|----------")
        
        for level in yes_bids[:5]:
            price = level.get('price', 0) / 100.0
            qty = level.get('quantity', 0)
            print(f"  ${price:.2f}  | {qty}")
        
        print("\nNO Side:")
        print("  Price  | Quantity")
        print("  -------|----------")
        
        for level in no_bids[:5]:
            price = level.get('price', 0) / 100.0
            qty = level.get('quantity', 0)
            print(f"  ${price:.2f}  | {qty}")
        
        return orderbook
    
    def place_and_cancel_order(self, ticker: str) -> bool:
        """Task 3: Place and cancel order."""
        print("\n" + "=" * 80)
        print("TASK 3: Placing and Canceling Order (Raw HTTP)")
        print("=" * 80)
        
        # Place order
        print("\nPlacing order for 1 contract...")
        print(f"  Market: {ticker}")
        print(f"  Side: YES")
        print(f"  Price: $0.01")
        
        order = self.client.create_order(
            ticker=ticker,
            action="buy",
            side="yes",
            count=1,
            order_type="limit",
            yes_price=1,  # $0.01 in cents
            expiration_ts=int(time.time()) + 300,
        )
        
        if not order:
            print("✗ Order placement failed")
            return False
        
        order_id = order.get('order_id')
        print(f"\n✓ Order placed!")
        print(f"  Order ID: {order_id}")
        print(f"  Status: {order.get('status')}")
        
        # Wait
        print("\nWaiting 2 seconds...")
        time.sleep(2)
        
        # Cancel
        print(f"\nCanceling order {order_id}...")
        canceled = self.client.cancel_order(order_id)
        
        if not canceled:
            print("✗ Cancellation failed")
            return False
        
        print(f"\n✓ Order canceled!")
        print(f"  Status: {canceled.get('status')}")
        
        # Verify
        final_status = self.client.get_order(order_id)
        if final_status:
            print(f"  Final Status: {final_status.get('status')}")
        
        return True
    
    def run_demo(self):
        """Run complete demo."""
        print("\n" + "=" * 80)
        print(" KALSHI API DEMO - RAW HTTP VERSION")
        print("=" * 80)
        
        # Task 1
        market = self.find_nyc_weather_market()
        if not market:
            print("\n✗ Demo failed - no market found")
            return False
        
        ticker = market.get('ticker')
        
        # Task 2
        orderbook = self.get_market_orderbook(ticker)
        
        # Task 3
        success = self.place_and_cancel_order(ticker)
        
        # Summary
        print("\n" + "=" * 80)
        print(" DEMO SUMMARY")
        print("=" * 80)
        print(f" ✓ Task 1: Found market {ticker}")
        print(f" {'✓' if orderbook else '⚠'} Task 2: Orderbook")
        print(f" {'✓' if success else '✗'} Task 3: Order placed/canceled")
        print("=" * 80)
        
        return success


def main():
    """Main entry point."""
    api_key = os.environ.get("KALSHI_API_KEY")
    private_key_path = os.environ.get("KALSHI_PRIVATE_KEY_PATH")
    
    if not api_key or not private_key_path:
        print("ERROR: Set KALSHI_API_KEY and KALSHI_PRIVATE_KEY_PATH")
        return
    
    demo = KalshiWeatherMarketDemo(api_key, private_key_path)
    demo.run_demo()


if __name__ == "__main__":
    main()
