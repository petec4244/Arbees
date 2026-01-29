pub mod binance;
pub mod chained_price;
pub mod coingecko;
pub mod coinbase;
pub mod crypto_price;
pub mod espn;
pub mod fred;
pub mod kalshi;
pub mod polymarket;
pub mod polymarket_clob;

// Re-export commonly used types
pub use binance::BinanceClient;
pub use chained_price::{ChainedPriceProvider, CoinGeckoClientAdapter};
pub use coinbase::CoinbaseClient;
pub use crypto_price::{CryptoPrice, CryptoPriceProvider, ProviderStatus, VolatilityResult};
