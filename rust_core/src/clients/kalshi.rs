//! Kalshi API client with RSA signature authentication for live trading.
//!
//! Supports both:
//! - Unauthenticated read-only operations (market data)
//! - Authenticated trading operations (order placement)
//!
//! Includes circuit breaker for API resilience.

use crate::circuit_breaker::{ApiCircuitBreaker, ApiCircuitBreakerConfig};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use log::{debug, info, warn};
use reqwest::Client;
use rsa::pkcs8::DecodePrivateKey;
use rsa::pss::{Signature, SigningKey};
use rsa::sha2::Sha256;
use rsa::signature::{RandomizedSigner, SignatureEncoding};
use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::league_config::LEAGUE_CONFIGS;

const KALSHI_API_PROD: &str = "https://api.elections.kalshi.com/trade-api/v2";
const KALSHI_API_DEMO: &str = "https://demo-api.kalshi.co/trade-api/v2";

/// Kalshi client with optional authentication for trading
#[derive(Clone)]
pub struct KalshiClient {
    client: Client,
    base_url: String,
    /// API key for authentication (optional for read-only operations)
    api_key: Option<String>,
    /// RSA private key for signing requests (optional for read-only operations)
    private_key: Option<Arc<RsaPrivateKey>>,
    /// Circuit breaker for API resilience
    circuit_breaker: Arc<ApiCircuitBreaker>,
}

impl std::fmt::Debug for KalshiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KalshiClient")
            .field("base_url", &self.base_url)
            .field("has_credentials", &self.has_credentials())
            .field("circuit_breaker_state", &self.circuit_breaker.state())
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KalshiMarket {
    pub ticker: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub status: String,
}

/// Order response from Kalshi API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KalshiOrder {
    pub order_id: String,
    pub ticker: String,
    pub side: String,
    pub action: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub status: String,
    pub yes_price: Option<i32>,
    pub no_price: Option<i32>,
    pub count: i32,
    pub remaining_count: Option<i32>,
    pub created_time: Option<String>,
}

/// Order placement request
#[derive(Debug, Clone, Serialize)]
pub struct KalshiOrderRequest {
    pub ticker: String,
    pub action: String,      // "buy" or "sell"
    pub side: String,        // "yes" or "no"
    #[serde(rename = "type")]
    pub order_type: String,  // "limit" or "market"
    pub count: i32,          // Number of contracts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yes_price: Option<i32>,  // Price in cents (for limit orders on yes side)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_price: Option<i32>,   // Price in cents (for limit orders on no side)
}

/// Order placement response
#[derive(Debug, Clone, Deserialize)]
pub struct KalshiOrderResponse {
    pub order: KalshiOrder,
}

impl KalshiClient {
    /// Create circuit breaker with default Kalshi-tuned settings
    fn create_circuit_breaker() -> Arc<ApiCircuitBreaker> {
        Arc::new(ApiCircuitBreaker::new(
            "kalshi",
            ApiCircuitBreakerConfig {
                failure_threshold: 3,           // Kalshi can be less reliable, lower threshold
                recovery_timeout: Duration::from_secs(60),  // Longer recovery for paid API
                success_threshold: 2,
            },
        ))
    }

    /// Create a new client without authentication (read-only operations)
    ///
    /// Returns Result to allow proper error handling if HTTP client creation fails.
    /// P1-1 Fix: Removed unsafe unwrap pattern.
    pub fn new() -> Result<Self> {
        let base_url = env::var("KALSHI_BASE_URL")
            .unwrap_or_else(|_| KALSHI_API_PROD.to_string());

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to create HTTP client for Kalshi")?;

        Ok(Self {
            client,
            base_url,
            api_key: None,
            private_key: None,
            circuit_breaker: Self::create_circuit_breaker(),
        })
    }

    /// Create a new client with authentication for trading
    ///
    /// P1-1 Fix: Proper error handling for HTTP client creation.
    pub fn with_credentials(api_key: String, private_key_pem: &str) -> Result<Self> {
        let base_url = env::var("KALSHI_BASE_URL")
            .unwrap_or_else(|_| KALSHI_API_PROD.to_string());

        let private_key = RsaPrivateKey::from_pkcs8_pem(private_key_pem)
            .context("Failed to parse Kalshi private key PEM")?;

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to create HTTP client for Kalshi")?;

        // Redact API key in logs - show only last 4 chars for identification
        let key_suffix = if api_key.len() > 4 { &api_key[api_key.len()-4..] } else { &api_key };
        info!("Kalshi client initialized with credentials (API key: ...{})", key_suffix);

        Ok(Self {
            client,
            base_url,
            api_key: Some(api_key),
            private_key: Some(Arc::new(private_key)),
            circuit_breaker: Self::create_circuit_breaker(),
        })
    }

    /// Create a client from environment variables
    ///
    /// Looks for:
    /// - KALSHI_API_KEY: The API key ID
    /// - KALSHI_PRIVATE_KEY: The RSA private key in PEM format (can include newlines as \n)
    /// - KALSHI_PRIVATE_KEY_PATH: Path to PEM file (alternative to KALSHI_PRIVATE_KEY)
    /// - KALSHI_ENV: "prod" or "demo" (defaults to prod)
    pub fn from_env() -> Result<Self> {
        let env_type = env::var("KALSHI_ENV").unwrap_or_else(|_| "prod".to_string());
        let base_url = match env_type.to_lowercase().as_str() {
            "demo" => KALSHI_API_DEMO.to_string(),
            _ => env::var("KALSHI_BASE_URL").unwrap_or_else(|_| KALSHI_API_PROD.to_string()),
        };

        let api_key = env::var("KALSHI_API_KEY").ok();

        // Try to load private key from environment or file
        let private_key = if let Ok(key_str) = env::var("KALSHI_PRIVATE_KEY") {
            // Handle escaped newlines in environment variable
            let key_pem = key_str.replace("\\n", "\n");
            Some(RsaPrivateKey::from_pkcs8_pem(&key_pem)
                .context("Failed to parse KALSHI_PRIVATE_KEY")?)
        } else if let Ok(key_path) = env::var("KALSHI_PRIVATE_KEY_PATH") {
            let key_pem = std::fs::read_to_string(&key_path)
                .with_context(|| format!("Failed to read private key from {}", key_path))?;
            Some(RsaPrivateKey::from_pkcs8_pem(&key_pem)
                .context("Failed to parse private key from file")?)
        } else {
            None
        };

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to create HTTP client for Kalshi")?;

        if api_key.is_some() && private_key.is_some() {
            info!("Kalshi client initialized with credentials from environment");
        } else {
            warn!("Kalshi client initialized without credentials (read-only mode)");
        }

        Ok(Self {
            client,
            base_url,
            api_key,
            private_key: private_key.map(Arc::new),
            circuit_breaker: Self::create_circuit_breaker(),
        })
    }

    /// Check if the Kalshi API is available (circuit breaker is not open)
    pub fn is_available(&self) -> bool {
        self.circuit_breaker.is_available()
    }

    /// Get the current circuit breaker state
    pub fn circuit_state(&self) -> crate::circuit_breaker::ApiCircuitState {
        self.circuit_breaker.state()
    }

    /// Reset the circuit breaker
    pub fn reset_circuit_breaker(&self) {
        self.circuit_breaker.reset();
    }

    /// Check if client has valid credentials for trading
    pub fn has_credentials(&self) -> bool {
        self.api_key.is_some() && self.private_key.is_some()
    }

    /// Generate RSA-PSS signature for Kalshi API authentication
    fn generate_signature(&self, timestamp_ms: i64, method: &str, path: &str) -> Result<String> {
        let private_key = self.private_key.as_ref()
            .ok_or_else(|| anyhow!("No private key configured"))?;

        // Message format: timestamp + method + path
        let message = format!("{}{}{}", timestamp_ms, method, path);
        debug!("Signing message: {}", message);

        // Create PSS signing key
        let signing_key = SigningKey::<Sha256>::new((**private_key).clone());

        // Sign with randomized padding
        let mut rng = rand::thread_rng();
        let signature: Signature = signing_key.sign_with_rng(&mut rng, message.as_bytes());

        // Encode as base64
        let sig_base64 = BASE64.encode(signature.to_bytes());

        Ok(sig_base64)
    }

    /// Make an authenticated request to Kalshi API
    async fn authenticated_request(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let api_key = self.api_key.as_ref()
            .ok_or_else(|| anyhow!("No API key configured"))?;

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // Path for signature includes the full API path
        let full_path = format!("/trade-api/v2{}", endpoint);
        let signature = self.generate_signature(timestamp_ms, method, &full_path)?;

        let url = format!("{}{}", self.base_url, endpoint);

        let mut request = match method {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "DELETE" => self.client.delete(&url),
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        };

        request = request
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("KALSHI-ACCESS-KEY", api_key)
            .header("KALSHI-ACCESS-TIMESTAMP", timestamp_ms.to_string())
            .header("KALSHI-ACCESS-SIGNATURE", signature);

        if let Some(json_body) = body {
            request = request.json(&json_body);
        }

        let resp = request.send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error_text = resp.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Kalshi API error ({}): {}", status, error_text));
        }

        let data: serde_json::Value = resp.json().await?;
        Ok(data)
    }

    // ==========================================================================
    // Market Data Methods (unauthenticated)
    // ==========================================================================

    /// Fetch markets, optionally filtering by sport/series_ticker.
    pub async fn get_markets(&self, sport: Option<&str>) -> Result<Vec<KalshiMarket>> {
        // Check circuit breaker before making request
        if !self.circuit_breaker.is_available() {
            return Err(anyhow!(
                "Kalshi API circuit breaker is open (sport={:?})",
                sport
            ));
        }

        let result = self.get_markets_internal(sport).await;

        // Record success or failure
        match &result {
            Ok(_) => self.circuit_breaker.record_success(),
            Err(_) => self.circuit_breaker.record_failure(),
        }

        result
    }

    /// Internal method that performs the actual API call
    async fn get_markets_internal(&self, sport: Option<&str>) -> Result<Vec<KalshiMarket>> {
        let url = format!("{}/markets", self.base_url);

        let mut params = vec![
            ("limit", "500"),
            ("status", "open"),
        ];

        let series_ticker;
        if let Some(s) = sport {
            if let Some(cfg) = LEAGUE_CONFIGS
                .iter()
                .find(|cfg| cfg.league_code.eq_ignore_ascii_case(s))
            {
                series_ticker = cfg.kalshi_series_game.to_string();
            } else {
                warn!(
                    "Unknown Kalshi series for sport '{}', using uppercase fallback",
                    s
                );
                series_ticker = s.to_uppercase();
            }
            params.push(("series_ticker", &series_ticker));
        }

        let resp = self.client.get(&url).query(&params).send().await?;

        if !resp.status().is_success() {
            resp.error_for_status_ref()?;
        }

        let data: serde_json::Value = resp.json().await?;

        let markets = match data.get("markets") {
             Some(v) if !v.is_null() => serde_json::from_value(v.clone())?,
             _ => Vec::new(),
        };

        Ok(markets)
    }

    /// Legacy compat / search helper
    pub async fn search_markets(&self, query: &str, sport: &str) -> Result<Vec<KalshiMarket>> {
        let markets = self.get_markets(Some(sport)).await?;

        let query_norm = query.to_lowercase();
        if query_norm.is_empty() {
             return Ok(markets);
        }

        let filtered = markets
            .into_iter()
            .filter(|m| m.title.to_lowercase().contains(&query_norm))
            .collect();

        Ok(filtered)
    }

    // ==========================================================================
    // Trading Methods (authenticated)
    // ==========================================================================

    /// Place an order on Kalshi
    ///
    /// # Arguments
    /// * `ticker` - Market ticker (e.g., "KXNFL-25-KC-NYG")
    /// * `side` - Contract side: "yes" or "no"
    /// * `price` - Price in decimal (0.0-1.0), will be converted to cents
    /// * `quantity` - Number of contracts
    ///
    /// # Returns
    /// * `Ok(KalshiOrder)` - The placed order
    /// * `Err` - If order placement fails
    pub async fn place_order(
        &self,
        ticker: &str,
        side: &str,
        price: f64,
        quantity: i32,
    ) -> Result<KalshiOrder> {
        if !self.has_credentials() {
            return Err(anyhow!("Cannot place order: no credentials configured"));
        }

        let side_lower = side.to_lowercase();
        if side_lower != "yes" && side_lower != "no" {
            return Err(anyhow!("Invalid side '{}': must be 'yes' or 'no'", side));
        }

        // Convert price to cents (Kalshi uses integer cents)
        let price_cents = (price * 100.0).round() as i32;
        if price_cents < 1 || price_cents > 99 {
            return Err(anyhow!("Invalid price {}: must be between 0.01 and 0.99", price));
        }

        // Build order request
        let order_req = KalshiOrderRequest {
            ticker: ticker.to_string(),
            action: "buy".to_string(),  // We're always buying contracts
            side: side_lower.clone(),
            order_type: "limit".to_string(),
            count: quantity,
            yes_price: if side_lower == "yes" { Some(price_cents) } else { None },
            no_price: if side_lower == "no" { Some(price_cents) } else { None },
        };

        info!(
            "Placing Kalshi order: {} {} x{} @ {}c on {}",
            order_req.action, order_req.side, order_req.count,
            if side_lower == "yes" { price_cents } else { 100 - price_cents },
            ticker
        );

        let body = serde_json::to_value(&order_req)?;
        let resp = self.authenticated_request("POST", "/portfolio/orders", Some(body)).await?;

        let order_resp: KalshiOrderResponse = serde_json::from_value(resp)
            .context("Failed to parse order response")?;

        info!("Order placed successfully: {}", order_resp.order.order_id);

        Ok(order_resp.order)
    }

    /// Get current positions
    pub async fn get_positions(&self) -> Result<Vec<serde_json::Value>> {
        if !self.has_credentials() {
            return Err(anyhow!("Cannot get positions: no credentials configured"));
        }

        let resp = self.authenticated_request("GET", "/portfolio/positions", None).await?;

        let positions = resp.get("market_positions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        Ok(positions)
    }

    /// Cancel an order by ID
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        if !self.has_credentials() {
            return Err(anyhow!("Cannot cancel order: no credentials configured"));
        }

        let endpoint = format!("/portfolio/orders/{}", order_id);
        self.authenticated_request("DELETE", &endpoint, None).await?;

        info!("Order {} cancelled successfully", order_id);
        Ok(())
    }

    /// Get account balance
    pub async fn get_balance(&self) -> Result<f64> {
        if !self.has_credentials() {
            return Err(anyhow!("Cannot get balance: no credentials configured"));
        }

        let resp = self.authenticated_request("GET", "/portfolio/balance", None).await?;

        let balance = resp.get("balance")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as f64 / 100.0;  // Convert cents to dollars

        Ok(balance)
    }
}

impl Default for KalshiClient {
    fn default() -> Self {
        Self::new().expect("Failed to create default KalshiClient")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = KalshiClient::new().expect("Failed to create client");
        assert!(!client.has_credentials());
    }

    #[test]
    fn test_price_conversion() {
        // Test that price in decimal is properly converted to cents
        let price: f64 = 0.45;
        let price_cents = (price * 100.0).round() as i32;
        assert_eq!(price_cents, 45);

        let price: f64 = 0.50;
        let price_cents = (price * 100.0).round() as i32;
        assert_eq!(price_cents, 50);
    }
}
