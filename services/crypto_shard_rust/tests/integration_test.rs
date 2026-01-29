//! Crypto Integration Tests
//!
//! Tests for crypto market discovery, price fetching, and probability calculations.
//! These tests require network access and should be run with `cargo test --ignored`.

use arbees_rust_core::clients::coingecko::CoinGeckoClient;
use arbees_rust_core::models::{CryptoPredictionType, MarketType};
use arbees_rust_core::probability::crypto::CryptoProbabilityModel;
use arbees_rust_core::probability::ProbabilityModel;
use arbees_rust_core::providers::crypto::CryptoEventProvider;
use arbees_rust_core::providers::{CryptoStateData, EventProvider, EventState, EventStatus, StateData};
use chrono::{Duration, Utc};

#[tokio::test]
#[ignore] // Requires network
async fn test_crypto_market_discovery() {
    let provider = CryptoEventProvider::new();
    let events = provider.get_live_events().await;

    match events {
        Ok(events) => {
            println!("Discovered {} crypto events", events.len());
            for event in events.iter().take(5) {
                println!("  - {} ({})", event.event_id, event.entity_a);
            }
            // Note: May be 0 if no crypto markets are currently listed
            // This test primarily verifies the API calls work
        }
        Err(e) => {
            // Log but don't fail - API may be unavailable
            println!("Warning: Could not fetch crypto markets: {}", e);
        }
    }
}

#[tokio::test]
#[ignore] // Requires network
async fn test_coingecko_price_fetch() {
    let client = CoinGeckoClient::new();

    let price = client.get_price("bitcoin").await;
    match price {
        Ok(price) => {
            assert!(price.current_price > 0.0);
            println!("BTC price: ${:.2}", price.current_price);
            println!("BTC market cap: ${:.0}", price.market_cap);
            println!("BTC 24h volume: ${:.0}", price.total_volume);
        }
        Err(e) => {
            println!("Warning: Could not fetch BTC price: {}", e);
        }
    }
}

#[tokio::test]
#[ignore] // Requires network
async fn test_coingecko_volatility_calculation() {
    let client = CoinGeckoClient::new();

    let vol = client.calculate_volatility("bitcoin", 30).await;
    match vol {
        Ok(vol) => {
            assert!(vol.annualized_volatility > 0.0);
            println!("BTC 30d volatility: {:.1}%", vol.annualized_volatility * 100.0);
            println!("BTC daily volatility: {:.2}%", vol.daily_volatility * 100.0);
        }
        Err(e) => {
            println!("Warning: Could not calculate volatility: {}", e);
        }
    }
}

#[tokio::test]
#[ignore] // Requires network
async fn test_coingecko_multiple_prices() {
    let client = CoinGeckoClient::new();

    let prices = client.get_prices(&["bitcoin", "ethereum", "solana"]).await;
    match prices {
        Ok(prices) => {
            println!("Fetched {} coin prices", prices.len());
            for price in &prices {
                println!("  {} ({}): ${:.2}", price.name, price.symbol, price.current_price);
            }
            assert!(prices.len() >= 2, "Should fetch at least BTC and ETH");
        }
        Err(e) => {
            println!("Warning: Could not fetch multiple prices: {}", e);
        }
    }
}

#[tokio::test]
async fn test_crypto_probability_calculation() {
    let model = CryptoProbabilityModel::new();

    // BTC at $50k, target $100k, 1 year out, 80% volatility
    let event_state = EventState {
        event_id: "test-btc-100k".to_string(),
        market_type: MarketType::Crypto {
            asset: "BTC".to_string(),
            prediction_type: CryptoPredictionType::PriceTarget,
        },
        entity_a: "BTC > $100000".to_string(),
        entity_b: None,
        status: EventStatus::Live,
        fetched_at: Utc::now(),
        state: StateData::Crypto(CryptoStateData {
            current_price: 50000.0,
            target_price: 100000.0,
            target_date: Utc::now() + Duration::days(365),
            volatility_24h: 0.8,
            volume_24h: Some(50_000_000_000.0),
            metadata: serde_json::json!({}),
        }),
    };

    let prob = model.calculate_probability(&event_state, true).await.unwrap();

    println!("P(BTC > $100k in 1 year) = {:.1}%", prob * 100.0);
    assert!(prob > 0.0 && prob < 1.0);
    assert!(prob > 0.1 && prob < 0.9, "Probability should be reasonable: {}", prob);
}

#[tokio::test]
async fn test_crypto_probability_already_above_target() {
    let model = CryptoProbabilityModel::new();

    // BTC at $120k, target $100k - already above
    let event_state = EventState {
        event_id: "test-btc-above-100k".to_string(),
        market_type: MarketType::Crypto {
            asset: "BTC".to_string(),
            prediction_type: CryptoPredictionType::PriceTarget,
        },
        entity_a: "BTC > $100000".to_string(),
        entity_b: None,
        status: EventStatus::Live,
        fetched_at: Utc::now(),
        state: StateData::Crypto(CryptoStateData {
            current_price: 120000.0,
            target_price: 100000.0,
            target_date: Utc::now() + Duration::days(30),
            volatility_24h: 0.8,
            volume_24h: Some(50_000_000_000.0),
            metadata: serde_json::json!({}),
        }),
    };

    let prob = model.calculate_probability(&event_state, true).await.unwrap();

    println!("P(BTC stays > $100k, currently at $120k) = {:.1}%", prob * 100.0);
    assert!(prob > 0.6, "Should have high probability when already above: {}", prob);
}

#[tokio::test]
async fn test_crypto_probability_short_time_frame() {
    let model = CryptoProbabilityModel::new();

    // BTC at $95k, target $100k, only 7 days out
    let event_state = EventState {
        event_id: "test-btc-short-term".to_string(),
        market_type: MarketType::Crypto {
            asset: "BTC".to_string(),
            prediction_type: CryptoPredictionType::PriceTarget,
        },
        entity_a: "BTC > $100000".to_string(),
        entity_b: None,
        status: EventStatus::Live,
        fetched_at: Utc::now(),
        state: StateData::Crypto(CryptoStateData {
            current_price: 95000.0,
            target_price: 100000.0,
            target_date: Utc::now() + Duration::days(7),
            volatility_24h: 0.8,
            volume_24h: Some(50_000_000_000.0),
            metadata: serde_json::json!({}),
        }),
    };

    let prob = model.calculate_probability(&event_state, true).await.unwrap();

    println!("P(BTC > $100k in 7 days, currently $95k) = {:.1}%", prob * 100.0);
    // With ~5% needed gain in 7 days and high vol, should be reasonable
    assert!(prob > 0.2 && prob < 0.8, "Short-term probability: {}", prob);
}

#[tokio::test]
async fn test_crypto_probability_model_supports() {
    let model = CryptoProbabilityModel::new();

    let crypto_market = MarketType::Crypto {
        asset: "ETH".to_string(),
        prediction_type: CryptoPredictionType::PriceTarget,
    };
    assert!(model.supports(&crypto_market));

    let sport_market = MarketType::sport(arbees_rust_core::models::Sport::NBA);
    assert!(!model.supports(&sport_market));
}

#[tokio::test]
async fn test_black_scholes_boundary_conditions() {
    // Test edge cases for the probability calculation

    // Edge case 1: Very short time (near expiry)
    let prob = CryptoProbabilityModel::calculate_price_target_probability(
        100.0, // current
        100.0, // target (at-the-money)
        0.1,   // 0.1 days
        0.8,   // 80% vol
    );
    assert!(prob > 0.4 && prob < 0.6, "Near-expiry ATM should be ~50%: {}", prob);

    // Edge case 2: Very high volatility
    let prob = CryptoProbabilityModel::calculate_price_target_probability(
        50.0,  // current
        100.0, // target (100% upside needed)
        30.0,  // 30 days
        2.0,   // 200% annualized vol
    );
    assert!(prob > 0.1, "High vol should give meaningful probability: {}", prob);

    // Edge case 3: Very low volatility
    let prob = CryptoProbabilityModel::calculate_price_target_probability(
        50.0,  // current
        100.0, // target
        30.0,  // 30 days
        0.1,   // 10% annualized vol
    );
    assert!(prob < 0.5, "Low vol with big gap should be low probability: {}", prob);
}

#[tokio::test]
#[ignore] // Requires network
async fn test_crypto_scheduled_events() {
    let provider = CryptoEventProvider::new();

    let events = provider.get_scheduled_events(90).await;
    match events {
        Ok(events) => {
            println!("Found {} scheduled crypto events in next 90 days", events.len());
            for event in events.iter().take(3) {
                println!(
                    "  - {} | {} | expires {}",
                    event.event_id,
                    event.entity_a,
                    event.scheduled_time
                );
            }
        }
        Err(e) => {
            println!("Warning: Could not fetch scheduled events: {}", e);
        }
    }
}

#[tokio::test]
async fn test_symbol_to_id_mapping() {
    // Verify the symbol mapping works correctly
    assert_eq!(CoinGeckoClient::symbol_to_id("BTC"), "bitcoin");
    assert_eq!(CoinGeckoClient::symbol_to_id("btc"), "bitcoin");
    assert_eq!(CoinGeckoClient::symbol_to_id("ETH"), "ethereum");
    assert_eq!(CoinGeckoClient::symbol_to_id("SOL"), "solana");
    assert_eq!(CoinGeckoClient::symbol_to_id("unknown"), "unknown");
}
