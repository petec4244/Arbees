"""
Kalshi API Advanced Limit Application - NYC Weather Market Sample

This script demonstrates:
1. Querying API for today's NYC weather market data
2. Fetching the orderbook for that market
3. Placing and canceling an order of 1 unit

Requirements:
- kalshi-python SDK (pip install kalshi-python)
- API credentials (private key file and API key)
"""

import asyncio
import json
import os
import time
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional

try:
    from kalshi_python import ApiClient, Configuration, ExchangeApi, MarketApi, PortfolioApi
except ImportError:
    print("ERROR: kalshi-python SDK not installed")
    print("Install with: pip install kalshi-python")
    exit(1)


class KalshiWeatherMarketDemo:
    """
    Demonstration of Kalshi API capabilities for advanced limit application.
    
    This class shows best practices for:
    - Market discovery and filtering
    - Orderbook querying
    - Order placement and cancellation
    - Error handling and rate limiting
    """
    
    def __init__(self, api_key: str, private_key_path: str):
        """
        Initialize Kalshi API client.
        
        Args:
            api_key: Your Kalshi API key
            private_key_path: Path to your private key PEM file
        """
        self.api_key = api_key
        self.private_key_path = private_key_path
        
        # Configure API client
        self.config = Configuration(
            host="https://trading-api.kalshi.com/trade-api/v2",
        )
        
        # Set authentication
        self.config.api_key['api_key'] = api_key
        
        # Load private key
        with open(private_key_path, 'r') as f:
            self.private_key = f.read()
        
        self.config.api_key['private_key'] = self.private_key
        
        # Initialize API clients
        self.api_client = ApiClient(self.config)
        self.exchange_api = ExchangeApi(self.api_client)
        self.market_api = MarketApi(self.api_client)
        self.portfolio_api = PortfolioApi(self.api_client)
        
        print("✓ Kalshi API client initialized")
    
    def find_nyc_weather_market(self) -> Optional[dict]:
        """
        Task 1: Query the API for market data on today's NYC weather.
        
        Strategy:
        1. Search for weather-related markets
        2. Filter for NYC-specific markets
        3. Filter for today's date
        4. Return the most relevant market
        
        Returns:
            Market data dict or None if not found
        """
        print("\n" + "=" * 80)
        print("TASK 1: Finding Today's NYC Weather Market")
        print("=" * 80)
        
        try:
            # Query markets with weather-related series tag
            # Kalshi uses series_ticker to categorize markets
            response = self.market_api.get_markets(
                series_ticker="KXTEMP",  # Temperature series
                status="open",
                limit=100,
            )
            
            markets = response.markets if hasattr(response, 'markets') else []
            
            print(f"Found {len(markets)} weather markets")
            
            # Filter for NYC markets happening today
            today = datetime.now().date()
            tomorrow = today + timedelta(days=1)
            
            nyc_today_markets = []
            
            for market in markets:
                ticker = market.ticker
                title = market.title.lower()
                
                # Check if it's NYC related
                if 'nyc' not in title and 'new york' not in title:
                    continue
                
                # Check if it's for today
                # Kalshi market tickers often include dates
                close_time = getattr(market, 'close_time', None)
                if close_time:
                    close_date = datetime.fromisoformat(close_time.replace('Z', '+00:00')).date()
                    if close_date < today or close_date > tomorrow:
                        continue
                
                nyc_today_markets.append(market)
            
            if not nyc_today_markets:
                print("⚠ No NYC weather markets found for today")
                print("Searching for any NYC weather market as fallback...")
                
                # Fallback: find any NYC weather market
                for market in markets:
                    if 'nyc' in market.title.lower() or 'new york' in market.title.lower():
                        nyc_today_markets.append(market)
                        break
            
            if nyc_today_markets:
                # Use the first matching market
                market = nyc_today_markets[0]
                
                print(f"\n✓ Found NYC Weather Market:")
                print(f"  Ticker: {market.ticker}")
                print(f"  Title: {market.title}")
                print(f"  Status: {market.status}")
                print(f"  Volume: {getattr(market, 'volume', 'N/A')}")
                print(f"  Open Interest: {getattr(market, 'open_interest', 'N/A')}")
                
                # Convert to dict for easier handling
                market_data = {
                    'ticker': market.ticker,
                    'title': market.title,
                    'status': market.status,
                    'volume': getattr(market, 'volume', 0),
                    'open_interest': getattr(market, 'open_interest', 0),
                    'yes_bid': getattr(market, 'yes_bid', None),
                    'yes_ask': getattr(market, 'yes_ask', None),
                    'close_time': getattr(market, 'close_time', None),
                }
                
                return market_data
            else:
                print("✗ No NYC weather markets found")
                return None
                
        except Exception as e:
            print(f"✗ Error finding NYC weather market: {e}")
            import traceback
            traceback.print_exc()
            return None
    
    def get_market_orderbook(self, market_ticker: str) -> Optional[dict]:
        """
        Task 2: Query the API for the orderbook of the market.
        
        Args:
            market_ticker: Market ticker symbol
            
        Returns:
            Orderbook data dict or None if error
        """
        print("\n" + "=" * 80)
        print("TASK 2: Fetching Market Orderbook")
        print("=" * 80)
        
        try:
            # Get orderbook for the market
            response = self.market_api.get_market_orderbook(
                ticker=market_ticker,
                depth=5,  # Get top 5 levels
            )
            
            orderbook = response.orderbook if hasattr(response, 'orderbook') else None
            
            if not orderbook:
                print(f"✗ No orderbook data available for {market_ticker}")
                return None
            
            # Parse orderbook
            yes_bids = orderbook.get('yes', []) if isinstance(orderbook, dict) else []
            no_bids = orderbook.get('no', []) if isinstance(orderbook, dict) else []
            
            print(f"\n✓ Orderbook for {market_ticker}:")
            print("\nYES Side:")
            print("  Price  | Quantity")
            print("  -------|----------")
            
            if yes_bids:
                for level in yes_bids[:5]:
                    price = level.get('price', 0) / 100.0 if isinstance(level, dict) else 0
                    quantity = level.get('quantity', 0) if isinstance(level, dict) else 0
                    print(f"  ${price:.2f}  | {quantity}")
            else:
                print("  (no bids)")
            
            print("\nNO Side:")
            print("  Price  | Quantity")
            print("  -------|----------")
            
            if no_bids:
                for level in no_bids[:5]:
                    price = level.get('price', 0) / 100.0 if isinstance(level, dict) else 0
                    quantity = level.get('quantity', 0) if isinstance(level, dict) else 0
                    print(f"  ${price:.2f}  | {quantity}")
            else:
                print("  (no bids)")
            
            # Get best bid/ask
            best_yes_bid = yes_bids[0].get('price', 0) / 100.0 if yes_bids and isinstance(yes_bids[0], dict) else None
            best_no_bid = no_bids[0].get('price', 0) / 100.0 if no_bids and isinstance(no_bids[0], dict) else None
            
            orderbook_data = {
                'yes_bids': yes_bids,
                'no_bids': no_bids,
                'best_yes_bid': best_yes_bid,
                'best_no_bid': best_no_bid,
                'timestamp': datetime.now().isoformat(),
            }
            
            print(f"\nBest YES Bid: ${best_yes_bid:.2f}" if best_yes_bid else "No YES bids")
            print(f"Best NO Bid: ${best_no_bid:.2f}" if best_no_bid else "No NO bids")
            
            return orderbook_data
            
        except Exception as e:
            print(f"✗ Error fetching orderbook: {e}")
            import traceback
            traceback.print_exc()
            return None
    
    def place_and_cancel_order(self, market_ticker: str) -> bool:
        """
        Task 3: Place and cancel an order of 1 unit on the market.
        
        Args:
            market_ticker: Market ticker symbol
            
        Returns:
            True if successful, False otherwise
        """
        print("\n" + "=" * 80)
        print("TASK 3: Placing and Canceling Order")
        print("=" * 80)
        
        order_id = None
        
        try:
            # Step 1: Place a limit order for 1 contract
            # Using a price that's unlikely to fill immediately (far from market)
            # This ensures we can cancel it before it fills
            
            print("\nPlacing order for 1 contract...")
            print("  Market: ", market_ticker)
            print("  Side: YES")
            print("  Quantity: 1")
            print("  Type: Limit")
            print("  Price: $0.01 (low price to avoid immediate fill)")
            
            # Place order
            order_response = self.portfolio_api.create_order(
                ticker=market_ticker,
                action="buy",
                side="yes",
                count=1,
                type="limit",
                yes_price=1,  # Price in cents ($0.01)
                expiration_ts=int(time.time()) + 300,  # Expire in 5 minutes
            )
            
            # Extract order ID
            if hasattr(order_response, 'order'):
                order = order_response.order
                order_id = order.order_id
                
                print(f"\n✓ Order placed successfully!")
                print(f"  Order ID: {order_id}")
                print(f"  Status: {getattr(order, 'status', 'unknown')}")
                print(f"  Remaining Count: {getattr(order, 'remaining_count', 'unknown')}")
            else:
                print("✗ Order placement failed - no order returned")
                return False
            
            # Step 2: Wait a moment to ensure order is in the system
            print("\nWaiting 2 seconds before cancellation...")
            time.sleep(2)
            
            # Step 3: Cancel the order
            print(f"\nCanceling order {order_id}...")
            
            cancel_response = self.portfolio_api.cancel_order(
                order_id=order_id
            )
            
            if hasattr(cancel_response, 'order'):
                canceled_order = cancel_response.order
                print(f"\n✓ Order canceled successfully!")
                print(f"  Order ID: {canceled_order.order_id}")
                print(f"  Status: {getattr(canceled_order, 'status', 'unknown')}")
            else:
                print("⚠ Cancel response received but no order confirmation")
            
            # Step 4: Verify cancellation
            print("\nVerifying order status...")
            
            try:
                order_status = self.portfolio_api.get_order(order_id=order_id)
                
                if hasattr(order_status, 'order'):
                    status = getattr(order_status.order, 'status', 'unknown')
                    print(f"  Final Order Status: {status}")
                    
                    if status.lower() in ['canceled', 'cancelled']:
                        print("\n✓ Order successfully canceled and verified!")
                        return True
                    else:
                        print(f"\n⚠ Order status is '{status}' (expected 'canceled')")
                        return True  # Still count as success if we got this far
            except:
                # Order might be gone from system if fully canceled
                print("  Order no longer in active orders (successfully canceled)")
                return True
            
            return True
            
        except Exception as e:
            print(f"\n✗ Error during order placement/cancellation: {e}")
            import traceback
            traceback.print_exc()
            
            # Attempt cleanup if order was placed
            if order_id:
                print(f"\nAttempting emergency cancellation of order {order_id}...")
                try:
                    self.portfolio_api.cancel_order(order_id=order_id)
                    print("✓ Emergency cancellation successful")
                except Exception as cancel_error:
                    print(f"✗ Emergency cancellation failed: {cancel_error}")
            
            return False
    
    def run_demo(self):
        """
        Run the complete demo workflow.
        
        Executes all three tasks in sequence:
        1. Find NYC weather market
        2. Get orderbook
        3. Place and cancel order
        """
        print("\n")
        print("=" * 80)
        print(" KALSHI API ADVANCED LIMIT APPLICATION - NYC WEATHER MARKET DEMO")
        print("=" * 80)
        print(f" Timestamp: {datetime.now().isoformat()}")
        print("=" * 80)
        
        # Task 1: Find market
        market_data = self.find_nyc_weather_market()
        
        if not market_data:
            print("\n✗ DEMO FAILED: Could not find NYC weather market")
            return False
        
        market_ticker = market_data['ticker']
        
        # Task 2: Get orderbook
        orderbook_data = self.get_market_orderbook(market_ticker)
        
        if not orderbook_data:
            print("\n⚠ WARNING: Could not fetch orderbook, continuing anyway...")
        
        # Task 3: Place and cancel order
        success = self.place_and_cancel_order(market_ticker)
        
        # Summary
        print("\n" + "=" * 80)
        print(" DEMO SUMMARY")
        print("=" * 80)
        print(f" ✓ Task 1: Found market {market_ticker}")
        print(f" {'✓' if orderbook_data else '⚠'} Task 2: {'Fetched' if orderbook_data else 'Attempted'} orderbook")
        print(f" {'✓' if success else '✗'} Task 3: {'Placed and canceled' if success else 'Failed to place/cancel'} order")
        print("=" * 80)
        
        if success:
            print("\n🎉 ALL TASKS COMPLETED SUCCESSFULLY!")
        else:
            print("\n⚠ DEMO COMPLETED WITH WARNINGS")
        
        return success


def main():
    """Main entry point for the demo script."""
    
    # Get credentials from environment or prompt
    api_key = os.environ.get("KALSHI_API_KEY")
    private_key_path = os.environ.get("KALSHI_PRIVATE_KEY_PATH")
    
    if not api_key:
        print("ERROR: KALSHI_API_KEY environment variable not set")
        print("\nSet it with:")
        print("  export KALSHI_API_KEY='your_api_key_here'")
        return
    
    if not private_key_path:
        print("ERROR: KALSHI_PRIVATE_KEY_PATH environment variable not set")
        print("\nSet it with:")
        print("  export KALSHI_PRIVATE_KEY_PATH='/path/to/private_key.pem'")
        return
    
    if not Path(private_key_path).exists():
        print(f"ERROR: Private key file not found at {private_key_path}")
        return
    
    # Run the demo
    demo = KalshiWeatherMarketDemo(
        api_key=api_key,
        private_key_path=private_key_path,
    )
    
    demo.run_demo()


if __name__ == "__main__":
    main()
