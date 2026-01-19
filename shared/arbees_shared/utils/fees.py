"""
Platform-specific fee calculations.

CRITICAL for profitability - fees can eliminate edges entirely.
"""

from dataclasses import dataclass
from enum import Enum
from typing import Optional

from arbees_shared.models.market import Platform


@dataclass(frozen=True)
class FeeSchedule:
    """Fee schedule for a platform."""
    trading_fee_pct: float  # Per-trade fee as percentage
    deposit_fee_usd: float = 0.0  # Fixed deposit fee
    withdrawal_fee_usd: float = 0.0  # Fixed withdrawal fee
    gas_fee_usd: float = 0.0  # Estimated gas fee (crypto)
    min_trade_size: float = 1.0  # Minimum trade size


# Platform fee schedules (as of Jan 2026)
FEE_SCHEDULES: dict[Platform, FeeSchedule] = {
    Platform.KALSHI: FeeSchedule(
        trading_fee_pct=0.7,  # 0.7% per trade
        deposit_fee_usd=0.0,
        withdrawal_fee_usd=0.0,
        min_trade_size=1.0,
    ),
    Platform.POLYMARKET: FeeSchedule(
        trading_fee_pct=2.0,  # 2% per trade
        deposit_fee_usd=5.0,  # ~$5 gas for deposit
        withdrawal_fee_usd=10.0,  # ~$10 gas for withdrawal
        gas_fee_usd=5.0,  # ~$5 per transaction
        min_trade_size=1.0,
    ),
    Platform.SPORTSBOOK: FeeSchedule(
        trading_fee_pct=10.0,  # ~10% vig (juice)
        deposit_fee_usd=0.0,
        withdrawal_fee_usd=0.0,
        min_trade_size=5.0,
    ),
    Platform.PAPER: FeeSchedule(
        trading_fee_pct=0.0,
        deposit_fee_usd=0.0,
        withdrawal_fee_usd=0.0,
        min_trade_size=0.0,
    ),
}


class FeeCalculator:
    """Calculate platform-specific fees for trades."""

    def __init__(self, custom_schedules: Optional[dict[Platform, FeeSchedule]] = None):
        """
        Initialize fee calculator.

        Args:
            custom_schedules: Override default fee schedules
        """
        self.schedules = FEE_SCHEDULES.copy()
        if custom_schedules:
            self.schedules.update(custom_schedules)

    def get_schedule(self, platform: Platform) -> FeeSchedule:
        """Get fee schedule for a platform."""
        return self.schedules.get(platform, FEE_SCHEDULES[Platform.PAPER])

    def calculate_trading_fee(
        self,
        platform: Platform,
        trade_size: float,
    ) -> float:
        """
        Calculate trading fee for a single trade.

        Args:
            platform: Trading platform
            trade_size: Size of trade in dollars

        Returns:
            Fee in dollars
        """
        schedule = self.get_schedule(platform)
        return trade_size * (schedule.trading_fee_pct / 100.0)

    def calculate_entry_cost(
        self,
        platform: Platform,
        trade_size: float,
        is_first_deposit: bool = False,
    ) -> float:
        """
        Calculate total cost to enter a position.

        Args:
            platform: Trading platform
            trade_size: Size of trade in dollars
            is_first_deposit: Whether this requires initial deposit

        Returns:
            Total entry cost in dollars
        """
        schedule = self.get_schedule(platform)

        # Trading fee
        cost = self.calculate_trading_fee(platform, trade_size)

        # Gas fee for crypto platforms
        if schedule.gas_fee_usd > 0:
            cost += schedule.gas_fee_usd

        # Deposit fee if first deposit
        if is_first_deposit:
            cost += schedule.deposit_fee_usd

        return cost

    def calculate_exit_cost(
        self,
        platform: Platform,
        trade_size: float,
        include_withdrawal: bool = False,
    ) -> float:
        """
        Calculate total cost to exit a position.

        Args:
            platform: Trading platform
            trade_size: Size of trade in dollars
            include_withdrawal: Whether to include withdrawal fee

        Returns:
            Total exit cost in dollars
        """
        schedule = self.get_schedule(platform)

        # Trading fee
        cost = self.calculate_trading_fee(platform, trade_size)

        # Gas fee for crypto platforms
        if schedule.gas_fee_usd > 0:
            cost += schedule.gas_fee_usd

        # Withdrawal fee
        if include_withdrawal:
            cost += schedule.withdrawal_fee_usd

        return cost

    def calculate_round_trip_cost(
        self,
        platform_buy: Platform,
        platform_sell: Platform,
        trade_size: float,
        include_deposits: bool = False,
        include_withdrawals: bool = False,
    ) -> float:
        """
        Calculate total round-trip cost for an arbitrage.

        Args:
            platform_buy: Platform to buy on
            platform_sell: Platform to sell on
            trade_size: Size of each leg in dollars
            include_deposits: Include deposit fees
            include_withdrawals: Include withdrawal fees

        Returns:
            Total round-trip cost in dollars
        """
        buy_cost = self.calculate_entry_cost(platform_buy, trade_size, include_deposits)
        sell_cost = self.calculate_exit_cost(platform_sell, trade_size, include_withdrawals)

        return buy_cost + sell_cost

    def get_net_edge(
        self,
        gross_edge_pct: float,
        platform_buy: Platform,
        platform_sell: Platform,
        trade_size: float,
    ) -> float:
        """
        Calculate net edge after fees.

        Args:
            gross_edge_pct: Gross edge in percentage points
            platform_buy: Platform to buy on
            platform_sell: Platform to sell on
            trade_size: Trade size in dollars

        Returns:
            Net edge in percentage points
        """
        fees = self.calculate_round_trip_cost(platform_buy, platform_sell, trade_size)
        fee_pct = (fees / trade_size) * 100.0 if trade_size > 0 else 0.0
        return gross_edge_pct - fee_pct

    def get_breakeven_edge(
        self,
        platform_buy: Platform,
        platform_sell: Platform,
    ) -> float:
        """
        Calculate minimum edge needed to break even.

        Args:
            platform_buy: Platform to buy on
            platform_sell: Platform to sell on

        Returns:
            Breakeven edge in percentage points
        """
        schedule_buy = self.get_schedule(platform_buy)
        schedule_sell = self.get_schedule(platform_sell)

        # Sum of trading fees
        total_fee_pct = schedule_buy.trading_fee_pct + schedule_sell.trading_fee_pct

        return total_fee_pct

    def validate_profitability(
        self,
        gross_edge_pct: float,
        platform_buy: Platform,
        platform_sell: Platform,
        trade_size: float,
        min_profit_usd: float = 1.0,
    ) -> tuple[bool, str]:
        """
        Validate if a trade would be profitable after fees.

        Args:
            gross_edge_pct: Gross edge in percentage points
            platform_buy: Platform to buy on
            platform_sell: Platform to sell on
            trade_size: Trade size in dollars
            min_profit_usd: Minimum acceptable profit

        Returns:
            Tuple of (is_profitable, reason)
        """
        # Check minimum trade size
        schedule_buy = self.get_schedule(platform_buy)
        schedule_sell = self.get_schedule(platform_sell)

        if trade_size < schedule_buy.min_trade_size:
            return False, f"Below min trade size for {platform_buy.value}"
        if trade_size < schedule_sell.min_trade_size:
            return False, f"Below min trade size for {platform_sell.value}"

        # Calculate net edge
        net_edge = self.get_net_edge(gross_edge_pct, platform_buy, platform_sell, trade_size)

        if net_edge <= 0:
            return False, f"Negative net edge: {net_edge:.2f}%"

        # Calculate expected profit
        expected_profit = trade_size * (net_edge / 100.0)

        if expected_profit < min_profit_usd:
            return False, f"Profit ${expected_profit:.2f} below minimum ${min_profit_usd}"

        return True, f"Profitable: ${expected_profit:.2f} ({net_edge:.2f}% net edge)"

    def estimate_monthly_costs(
        self,
        platform: Platform,
        trades_per_day: int,
        avg_trade_size: float,
        include_one_deposit: bool = True,
        include_one_withdrawal: bool = True,
    ) -> dict[str, float]:
        """
        Estimate monthly trading costs.

        Args:
            platform: Trading platform
            trades_per_day: Average trades per day
            avg_trade_size: Average trade size
            include_one_deposit: Include one deposit per month
            include_one_withdrawal: Include one withdrawal per month

        Returns:
            Cost breakdown dictionary
        """
        schedule = self.get_schedule(platform)
        days_per_month = 30
        total_trades = trades_per_day * days_per_month

        trading_fees = total_trades * self.calculate_trading_fee(platform, avg_trade_size)
        gas_fees = total_trades * schedule.gas_fee_usd
        deposit_fees = schedule.deposit_fee_usd if include_one_deposit else 0.0
        withdrawal_fees = schedule.withdrawal_fee_usd if include_one_withdrawal else 0.0

        total = trading_fees + gas_fees + deposit_fees + withdrawal_fees

        return {
            "trading_fees": trading_fees,
            "gas_fees": gas_fees,
            "deposit_fees": deposit_fees,
            "withdrawal_fees": withdrawal_fees,
            "total": total,
            "trades": total_trades,
            "avg_cost_per_trade": total / total_trades if total_trades > 0 else 0,
        }


# Convenience functions
def get_net_edge(
    gross_edge_pct: float,
    platform_buy: Platform,
    platform_sell: Platform,
    trade_size: float,
) -> float:
    """Calculate net edge after fees."""
    calc = FeeCalculator()
    return calc.get_net_edge(gross_edge_pct, platform_buy, platform_sell, trade_size)


def is_profitable(
    gross_edge_pct: float,
    platform_buy: Platform,
    platform_sell: Platform,
    trade_size: float,
) -> bool:
    """Check if trade would be profitable."""
    calc = FeeCalculator()
    profitable, _ = calc.validate_profitability(
        gross_edge_pct, platform_buy, platform_sell, trade_size
    )
    return profitable


# Common arbitrage scenarios
def kalshi_to_kalshi_costs(trade_size: float) -> float:
    """Round-trip costs for Kalshi ↔ Kalshi arbitrage."""
    calc = FeeCalculator()
    return calc.calculate_round_trip_cost(Platform.KALSHI, Platform.KALSHI, trade_size)


def kalshi_to_polymarket_costs(trade_size: float) -> float:
    """Round-trip costs for Kalshi ↔ Polymarket arbitrage."""
    calc = FeeCalculator()
    return calc.calculate_round_trip_cost(Platform.KALSHI, Platform.POLYMARKET, trade_size)
