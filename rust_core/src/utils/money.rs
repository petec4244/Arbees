//! Financial precision utilities for accurate money calculations.
//!
//! P0-1 Fix: Prevents floating-point precision errors in financial calculations.
//!
//! # Design Philosophy
//!
//! - All internal calculations use i64 cents (1/100 of a dollar)
//! - Conversion to/from f64 dollars happens only at API boundaries
//! - Rounding is explicit and documented
//!
//! # Usage
//!
//! ```rust
//! use arbees_rust_core::utils::money::{Money, to_cents, from_cents, round_to_cents};
//!
//! // Create from dollars
//! let price = Money::from_dollars(0.65);
//! assert_eq!(price.cents(), 65);
//!
//! // Arithmetic operations (in cents, no precision loss)
//! let pnl = Money::from_cents(150) - Money::from_cents(65);
//! assert_eq!(pnl.cents(), 85);
//!
//! // Convert back to dollars for display
//! println!("P&L: ${:.2}", pnl.as_dollars());
//! ```

use std::fmt;
use std::ops::{Add, Sub, Mul, Div, Neg};

/// Money value stored as cents (i64) for precision.
///
/// This type prevents floating-point precision errors in financial calculations
/// by using integer arithmetic internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Money {
    /// Value in cents (1/100 of a dollar)
    cents: i64,
}

impl Money {
    /// Create from cents directly (no conversion)
    #[inline]
    pub const fn from_cents(cents: i64) -> Self {
        Self { cents }
    }

    /// Create from dollars (rounds to nearest cent)
    #[inline]
    pub fn from_dollars(dollars: f64) -> Self {
        Self {
            cents: (dollars * 100.0).round() as i64,
        }
    }

    /// Create zero value
    #[inline]
    pub const fn zero() -> Self {
        Self { cents: 0 }
    }

    /// Get value in cents
    #[inline]
    pub const fn cents(&self) -> i64 {
        self.cents
    }

    /// Get value as dollars (for display/API)
    #[inline]
    pub fn as_dollars(&self) -> f64 {
        self.cents as f64 / 100.0
    }

    /// Check if value is zero
    #[inline]
    pub const fn is_zero(&self) -> bool {
        self.cents == 0
    }

    /// Check if value is positive
    #[inline]
    pub const fn is_positive(&self) -> bool {
        self.cents > 0
    }

    /// Check if value is negative
    #[inline]
    pub const fn is_negative(&self) -> bool {
        self.cents < 0
    }

    /// Get absolute value
    #[inline]
    pub const fn abs(&self) -> Self {
        Self {
            cents: self.cents.abs(),
        }
    }

    /// Clamp value to a range
    #[inline]
    pub fn clamp(self, min: Self, max: Self) -> Self {
        Self {
            cents: self.cents.clamp(min.cents, max.cents),
        }
    }
}

impl Add for Money {
    type Output = Self;

    #[inline]
    fn add(self, other: Self) -> Self {
        Self {
            cents: self.cents + other.cents,
        }
    }
}

impl Sub for Money {
    type Output = Self;

    #[inline]
    fn sub(self, other: Self) -> Self {
        Self {
            cents: self.cents - other.cents,
        }
    }
}

impl Mul<i64> for Money {
    type Output = Self;

    #[inline]
    fn mul(self, rhs: i64) -> Self {
        Self {
            cents: self.cents * rhs,
        }
    }
}

impl Div<i64> for Money {
    type Output = Self;

    #[inline]
    fn div(self, rhs: i64) -> Self {
        Self {
            cents: self.cents / rhs,
        }
    }
}

impl Neg for Money {
    type Output = Self;

    #[inline]
    fn neg(self) -> Self {
        Self {
            cents: -self.cents,
        }
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.cents < 0 {
            write!(f, "-${:.2}", (-self.cents) as f64 / 100.0)
        } else {
            write!(f, "${:.2}", self.cents as f64 / 100.0)
        }
    }
}

// ============================================================================
// Standalone conversion functions
// ============================================================================

/// Convert dollars to cents (rounds to nearest cent)
#[inline]
pub fn to_cents(dollars: f64) -> i64 {
    (dollars * 100.0).round() as i64
}

/// Convert cents to dollars
#[inline]
pub fn from_cents(cents: i64) -> f64 {
    cents as f64 / 100.0
}

/// Round a dollar amount to the nearest cent
#[inline]
pub fn round_to_cents(dollars: f64) -> f64 {
    (dollars * 100.0).round() / 100.0
}

/// Round a dollar amount down to the nearest cent (floor)
#[inline]
pub fn floor_to_cents(dollars: f64) -> f64 {
    (dollars * 100.0).floor() / 100.0
}

/// Round a dollar amount up to the nearest cent (ceil)
#[inline]
pub fn ceil_to_cents(dollars: f64) -> f64 {
    (dollars * 100.0).ceil() / 100.0
}

// ============================================================================
// P&L calculation helpers
// ============================================================================

/// Calculate P&L for a trade in cents for maximum precision.
///
/// # Arguments
/// * `size_cents` - Position size in cents (e.g., $10 position = 1000)
/// * `entry_price` - Entry price as a probability (0.0 to 1.0)
/// * `exit_price` - Exit price as a probability (0.0 to 1.0)
/// * `is_buy` - True if this is a buy position (bet YES), false for sell (bet NO)
/// * `entry_fee_cents` - Entry fees in cents
/// * `exit_fee_cents` - Exit fees in cents
///
/// # Returns
/// Net P&L in cents
pub fn calculate_pnl_cents(
    size_cents: i64,
    entry_price: f64,
    exit_price: f64,
    is_buy: bool,
    entry_fee_cents: i64,
    exit_fee_cents: i64,
) -> i64 {
    // Calculate gross P&L in cents
    // For a $1 binary option:
    // - Buy side profit = (exit_price - entry_price) * size
    // - Sell side profit = (entry_price - exit_price) * size
    let price_diff = if is_buy {
        exit_price - entry_price
    } else {
        entry_price - exit_price
    };

    // Convert to cents (price_diff is a decimal like 0.05 for 5 cents)
    let gross_pnl_cents = (price_diff * size_cents as f64).round() as i64;

    // Net P&L = Gross - Fees
    gross_pnl_cents - entry_fee_cents - exit_fee_cents
}

/// Calculate P&L percentage relative to risk amount.
///
/// # Arguments
/// * `pnl_cents` - P&L in cents
/// * `risk_cents` - Risk amount in cents
///
/// # Returns
/// P&L as percentage of risk (e.g., 10.0 for 10%)
pub fn pnl_percentage(pnl_cents: i64, risk_cents: i64) -> f64 {
    if risk_cents == 0 {
        return 0.0;
    }
    (pnl_cents as f64 / risk_cents as f64) * 100.0
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_money_from_dollars() {
        assert_eq!(Money::from_dollars(1.23).cents(), 123);
        assert_eq!(Money::from_dollars(0.01).cents(), 1);
        assert_eq!(Money::from_dollars(-5.50).cents(), -550);
    }

    #[test]
    fn test_money_from_dollars_rounding() {
        // Test banker's rounding behavior
        assert_eq!(Money::from_dollars(1.234).cents(), 123);
        assert_eq!(Money::from_dollars(1.235).cents(), 124); // Rounds up at .5
        assert_eq!(Money::from_dollars(1.236).cents(), 124);
    }

    #[test]
    fn test_money_arithmetic() {
        let a = Money::from_cents(100);
        let b = Money::from_cents(35);

        assert_eq!((a + b).cents(), 135);
        assert_eq!((a - b).cents(), 65);
        assert_eq!((a * 3).cents(), 300);
        assert_eq!((a / 2).cents(), 50);
        assert_eq!((-a).cents(), -100);
    }

    #[test]
    fn test_money_display() {
        assert_eq!(Money::from_cents(123).to_string(), "$1.23");
        assert_eq!(Money::from_cents(-456).to_string(), "-$4.56");
        assert_eq!(Money::from_cents(5).to_string(), "$0.05");
    }

    #[test]
    fn test_round_to_cents() {
        assert_eq!(round_to_cents(1.234), 1.23);
        assert_eq!(round_to_cents(1.235), 1.24);
        assert_eq!(round_to_cents(1.999), 2.00);
    }

    #[test]
    fn test_calculate_pnl_cents() {
        // Buy at 0.50, exit at 0.60, size $10 (1000 cents)
        // Gross = (0.60 - 0.50) * 1000 = 100 cents = $1
        // Net = 100 - 5 - 5 = 90 cents
        let pnl = calculate_pnl_cents(1000, 0.50, 0.60, true, 5, 5);
        assert_eq!(pnl, 90);

        // Sell at 0.50, exit at 0.40, size $10 (1000 cents)
        // Gross = (0.50 - 0.40) * 1000 = 100 cents
        let pnl = calculate_pnl_cents(1000, 0.50, 0.40, false, 5, 5);
        assert_eq!(pnl, 90);

        // Losing trade: buy at 0.60, exit at 0.50
        let pnl = calculate_pnl_cents(1000, 0.60, 0.50, true, 5, 5);
        assert_eq!(pnl, -110);
    }

    #[test]
    fn test_pnl_percentage() {
        assert_eq!(pnl_percentage(100, 1000), 10.0);
        assert_eq!(pnl_percentage(-50, 500), -10.0);
        assert_eq!(pnl_percentage(0, 0), 0.0); // Edge case: zero risk
    }

    #[test]
    fn test_precision_no_accumulation() {
        // This would fail with f64 due to floating-point errors
        let mut total = Money::zero();
        for _ in 0..1000 {
            total = total + Money::from_cents(1);
        }
        assert_eq!(total.cents(), 1000);
        assert_eq!(total.as_dollars(), 10.0);
    }
}
