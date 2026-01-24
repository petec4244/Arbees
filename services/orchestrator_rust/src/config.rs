use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub redis_url: String,
    pub database_url: String,
    pub discovery_interval_secs: u64,
    pub health_check_interval_secs: u64,
    pub shard_timeout_secs: u64,
    pub scheduled_sync_interval_secs: u64,
    pub pregame_discovery_window_hours: i64,
    pub market_discovery_mode: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            redis_url: env::var("REDIS_URL").unwrap_or_else(|_| "redis://redis:6379".to_string()),
            database_url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            discovery_interval_secs: env::var("DISCOVERY_INTERVAL_SECS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .unwrap(),
            health_check_interval_secs: env::var("HEALTH_CHECK_INTERVAL_SECS")
                .unwrap_or_else(|_| "15".to_string())
                .parse()
                .unwrap(),
            shard_timeout_secs: env::var("SHARD_TIMEOUT_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()
                .unwrap(),
            scheduled_sync_interval_secs: env::var("SCHEDULED_SYNC_INTERVAL_SECS")
                .unwrap_or_else(|_| "3600".to_string())
                .parse()
                .unwrap(),
            pregame_discovery_window_hours: env::var("PREGAME_DISCOVERY_WINDOW_HOURS")
                .unwrap_or_else(|_| "6".to_string())
                .parse()
                .unwrap(),
            market_discovery_mode: env::var("MARKET_DISCOVERY_MODE")
                .unwrap_or_else(|_| "rust".to_string()),
        }
    }
}
