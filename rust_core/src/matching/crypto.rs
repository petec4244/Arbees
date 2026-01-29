//! Crypto Asset Entity Matcher
//!
//! Implements the EntityMatcher trait for cryptocurrency asset matching.
//! Supports matching by symbol, full name, and common aliases.

use super::{EntityMatcher, MatchConfidence, MatchContext, MatchResult};
use crate::models::MarketType;
use async_trait::async_trait;
use std::collections::HashMap;

/// Crypto asset entity matcher
///
/// Provides matching for crypto assets using symbols, names, and aliases.
/// Supports the most commonly traded cryptocurrencies.
pub struct CryptoAssetMatcher {
    /// Map from canonical symbol to list of aliases
    aliases: HashMap<String, Vec<String>>,
}

impl CryptoAssetMatcher {
    /// Create a new crypto asset matcher with default aliases
    pub fn new() -> Self {
        let mut aliases = HashMap::new();

        // Major cryptocurrencies and their aliases
        aliases.insert(
            "BTC".to_string(),
            vec![
                "bitcoin".to_string(),
                "btc".to_string(),
                "xbt".to_string(),
                "satoshi".to_string(),
            ],
        );
        aliases.insert(
            "ETH".to_string(),
            vec![
                "ethereum".to_string(),
                "eth".to_string(),
                "ether".to_string(),
            ],
        );
        aliases.insert(
            "SOL".to_string(),
            vec!["solana".to_string(), "sol".to_string()],
        );
        aliases.insert(
            "XRP".to_string(),
            vec!["ripple".to_string(), "xrp".to_string()],
        );
        aliases.insert(
            "DOGE".to_string(),
            vec![
                "dogecoin".to_string(),
                "doge".to_string(),
                "shibe".to_string(),
            ],
        );
        aliases.insert(
            "ADA".to_string(),
            vec!["cardano".to_string(), "ada".to_string()],
        );
        aliases.insert(
            "AVAX".to_string(),
            vec!["avalanche".to_string(), "avax".to_string()],
        );
        aliases.insert(
            "DOT".to_string(),
            vec!["polkadot".to_string(), "dot".to_string()],
        );
        aliases.insert(
            "MATIC".to_string(),
            vec![
                "polygon".to_string(),
                "matic".to_string(),
                "pol".to_string(),
            ],
        );
        aliases.insert(
            "LINK".to_string(),
            vec!["chainlink".to_string(), "link".to_string()],
        );
        aliases.insert(
            "UNI".to_string(),
            vec!["uniswap".to_string(), "uni".to_string()],
        );
        aliases.insert(
            "ATOM".to_string(),
            vec!["cosmos".to_string(), "atom".to_string()],
        );
        aliases.insert(
            "LTC".to_string(),
            vec!["litecoin".to_string(), "ltc".to_string()],
        );
        aliases.insert(
            "SHIB".to_string(),
            vec!["shiba".to_string(), "shibainu".to_string(), "shib".to_string()],
        );
        aliases.insert(
            "NEAR".to_string(),
            vec!["near".to_string(), "near protocol".to_string()],
        );
        aliases.insert(
            "APT".to_string(),
            vec!["aptos".to_string(), "apt".to_string()],
        );
        aliases.insert(
            "ARB".to_string(),
            vec!["arbitrum".to_string(), "arb".to_string()],
        );
        aliases.insert(
            "OP".to_string(),
            vec!["optimism".to_string(), "op".to_string()],
        );

        Self { aliases }
    }

    /// Check if text contains the entity (case-insensitive word boundary match)
    fn contains_word(&self, text: &str, word: &str) -> bool {
        let text_lower = text.to_lowercase();
        let word_lower = word.to_lowercase();

        // Try exact word boundary match
        for text_word in text_lower.split(|c: char| !c.is_alphanumeric()) {
            if text_word == word_lower {
                return true;
            }
        }

        false
    }

    /// Check if text contains the entity as a substring
    fn contains_substring(&self, text: &str, word: &str) -> bool {
        text.to_lowercase().contains(&word.to_lowercase())
    }

    /// Get canonical symbol from an alias
    pub fn get_canonical_symbol(&self, name: &str) -> Option<String> {
        let name_lower = name.to_lowercase();

        for (symbol, aliases) in &self.aliases {
            if symbol.to_lowercase() == name_lower {
                return Some(symbol.clone());
            }
            for alias in aliases {
                if alias == &name_lower {
                    return Some(symbol.clone());
                }
            }
        }

        None
    }
}

impl Default for CryptoAssetMatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EntityMatcher for CryptoAssetMatcher {
    async fn match_entity_in_text(
        &self,
        entity_name: &str,
        text: &str,
        _context: &MatchContext,
    ) -> MatchResult {
        let entity_upper = entity_name.to_uppercase();

        // 1. Exact symbol match (highest confidence)
        if self.contains_word(text, entity_name) {
            return MatchResult {
                confidence: MatchConfidence::Exact,
                score: 1.0,
                reason: format!("Exact symbol match: {}", entity_name),
            };
        }

        // 2. Check aliases for this symbol
        if let Some(aliases) = self.aliases.get(&entity_upper) {
            // Check for exact alias match
            for alias in aliases {
                if self.contains_word(text, alias) {
                    return MatchResult {
                        confidence: MatchConfidence::High,
                        score: 0.95,
                        reason: format!("Alias match: {} -> {}", alias, entity_name),
                    };
                }
            }

            // Check for substring alias match (slightly lower confidence)
            for alias in aliases {
                if alias.len() >= 4 && self.contains_substring(text, alias) {
                    return MatchResult {
                        confidence: MatchConfidence::Medium,
                        score: 0.80,
                        reason: format!("Partial alias match: {} contains {}", text, alias),
                    };
                }
            }
        }

        // 3. Try reverse lookup - maybe entity_name is an alias
        if let Some(canonical) = self.get_canonical_symbol(entity_name) {
            // If the text contains the canonical symbol
            if self.contains_word(text, &canonical) {
                return MatchResult {
                    confidence: MatchConfidence::High,
                    score: 0.90,
                    reason: format!("Reverse alias match: {} -> {}", entity_name, canonical),
                };
            }

            // Check other aliases of the canonical symbol
            if let Some(aliases) = self.aliases.get(&canonical) {
                for alias in aliases {
                    if self.contains_word(text, alias) {
                        return MatchResult {
                            confidence: MatchConfidence::Medium,
                            score: 0.85,
                            reason: format!("Cross-alias match: {} via {}", alias, canonical),
                        };
                    }
                }
            }
        }

        // 4. No match
        MatchResult::none()
    }

    fn supports(&self, market_type: &MarketType) -> bool {
        matches!(market_type, MarketType::Crypto { .. })
    }

    fn matcher_name(&self) -> &str {
        "CryptoAssetMatcher"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CryptoPredictionType;

    #[tokio::test]
    async fn test_crypto_matcher_supports() {
        let matcher = CryptoAssetMatcher::new();
        assert_eq!(matcher.matcher_name(), "CryptoAssetMatcher");

        let crypto_market = MarketType::Crypto {
            asset: "BTC".to_string(),
            prediction_type: CryptoPredictionType::PriceTarget,
        };
        assert!(matcher.supports(&crypto_market));

        let sport_market = MarketType::sport(crate::models::Sport::NBA);
        assert!(!matcher.supports(&sport_market));
    }

    #[tokio::test]
    async fn test_exact_symbol_match() {
        let matcher = CryptoAssetMatcher::new();
        let context = MatchContext::new();

        let result = matcher
            .match_entity_in_text("BTC", "Will BTC reach $100,000?", &context)
            .await;

        assert!(result.is_match());
        assert_eq!(result.confidence, MatchConfidence::Exact);
    }

    #[tokio::test]
    async fn test_alias_match() {
        let matcher = CryptoAssetMatcher::new();
        let context = MatchContext::new();

        let result = matcher
            .match_entity_in_text("BTC", "Bitcoin price prediction for 2026", &context)
            .await;

        assert!(result.is_match());
        assert_eq!(result.confidence, MatchConfidence::High);
        assert!(result.reason.contains("bitcoin"));
    }

    #[tokio::test]
    async fn test_ethereum_match() {
        let matcher = CryptoAssetMatcher::new();
        let context = MatchContext::new();

        let result = matcher
            .match_entity_in_text("ETH", "Will Ethereum hit $10,000?", &context)
            .await;

        assert!(result.is_match());
        assert!(result.score > 0.9);
    }

    #[tokio::test]
    async fn test_no_match() {
        let matcher = CryptoAssetMatcher::new();
        let context = MatchContext::new();

        let result = matcher
            .match_entity_in_text("BTC", "Lakers vs Celtics game prediction", &context)
            .await;

        assert!(!result.is_match());
        assert_eq!(result.confidence, MatchConfidence::None);
    }

    #[tokio::test]
    async fn test_case_insensitive() {
        let matcher = CryptoAssetMatcher::new();
        let context = MatchContext::new();

        let result = matcher
            .match_entity_in_text("btc", "BTC price target", &context)
            .await;

        assert!(result.is_match());
    }

    #[tokio::test]
    async fn test_canonical_symbol_lookup() {
        let matcher = CryptoAssetMatcher::new();

        assert_eq!(matcher.get_canonical_symbol("bitcoin"), Some("BTC".to_string()));
        assert_eq!(matcher.get_canonical_symbol("ETHEREUM"), Some("ETH".to_string()));
        assert_eq!(matcher.get_canonical_symbol("sol"), Some("SOL".to_string()));
        assert_eq!(matcher.get_canonical_symbol("unknown"), None);
    }

    #[tokio::test]
    async fn test_multiple_aliases() {
        let matcher = CryptoAssetMatcher::new();
        let context = MatchContext::new();

        // Test various aliases for the same asset
        let texts = vec![
            "XBT futures trading",
            "Buy some Bitcoin today",
            "BTC/USD pair",
        ];

        for text in texts {
            let result = matcher
                .match_entity_in_text("BTC", text, &context)
                .await;
            assert!(result.is_match(), "Should match BTC in: {}", text);
        }
    }
}
