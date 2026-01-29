use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Sport {
    NFL,
    NBA,
    NHL,
    MLB,
    NCAAF,
    NCAAB,
    MLS,
}

impl Sport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Sport::NFL => "nfl",
            Sport::NBA => "nba",
            Sport::NHL => "nhl",
            Sport::MLB => "mlb",
            Sport::NCAAF => "ncaaf",
            Sport::NCAAB => "ncaab",
            Sport::MLS => "mls",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameInfo {
    pub game_id: String,
    pub sport: Sport,
    pub home_team: String,
    pub away_team: String,
    pub home_team_abbrev: String,
    pub away_team_abbrev: String,
    pub scheduled_time: DateTime<Utc>,
    pub status: String,
    pub venue: Option<String>,
    pub broadcast: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardInfo {
    pub shard_id: String,
    pub game_count: usize,
    pub max_games: usize,
    pub games: Vec<String>,
    pub last_heartbeat: DateTime<Utc>,
    pub is_healthy: bool,
}

impl ShardInfo {
    pub fn available_capacity(&self) -> usize {
        if self.game_count >= self.max_games {
            0
        } else {
            self.max_games - self.game_count
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameAssignment {
    pub game_id: String,
    pub sport: Sport,
    pub shard_id: String,
    pub kalshi_market_id: Option<String>,
    pub polymarket_market_id: Option<String>,
    pub market_ids_by_type: HashMap<String, HashMap<String, String>>, // market_type -> platform -> id
    pub assigned_at: DateTime<Utc>,
}

// ============================================================================
// Fault Tolerance & Service Management Types
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceType {
    GameShard,
    CryptoShard,
    PolymarketMonitor,
    KalshiMonitor,
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Starting,   // Just came online
    Healthy,    // All checks passing
    Degraded,   // Some checks failing but operational
    Unhealthy,  // Cannot perform primary function
    Stopping,   // Graceful shutdown
    Dead,       // No heartbeat for timeout period
}

impl Default for ServiceStatus {
    fn default() -> Self {
        ServiceStatus::Starting
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitState {
    Closed,    // Normal operation, assigning games
    HalfOpen,  // Testing recovery, limited assignments
    Open,      // Not assigning games due to failures
}

impl Default for CircuitState {
    fn default() -> Self {
        CircuitState::Closed
    }
}

#[derive(Clone, Debug)]
pub struct AssignmentCircuitConfig {
    pub failure_threshold: u32,       // Open after this many failures (default: 3)
    pub success_threshold: u32,       // Close after this many successes in half-open (default: 2)
    pub half_open_timeout: Duration,  // Wait before trying half-open (default: 30s)
}

impl Default for AssignmentCircuitConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            success_threshold: 2,
            half_open_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssignmentCircuitBreaker {
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    pub last_state_change: DateTime<Utc>,
    pub config: AssignmentCircuitConfig,
}

impl AssignmentCircuitBreaker {
    pub fn new(config: AssignmentCircuitConfig) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            last_state_change: Utc::now(),
            config,
        }
    }

    pub fn can_assign(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if timeout elapsed, transition to half-open
                let elapsed = Utc::now()
                    .signed_duration_since(self.last_state_change)
                    .num_seconds() as u64;

                if elapsed >= self.config.half_open_timeout.as_secs() {
                    self.state = CircuitState::HalfOpen;
                    self.success_count = 0;
                    self.failure_count = 0;
                    self.last_state_change = Utc::now();
                    log::info!("Assignment circuit breaker -> HalfOpen");
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true, // Allow limited assignments
        }
    }

    pub fn record_assignment_success(&mut self) {
        self.failure_count = 0;

        match self.state {
            CircuitState::HalfOpen => {
                self.success_count += 1;
                if self.success_count >= self.config.success_threshold {
                    self.state = CircuitState::Closed;
                    self.last_state_change = Utc::now();
                    log::info!("Assignment circuit breaker -> Closed (recovered)");
                }
            }
            _ => {
                self.state = CircuitState::Closed; // Ensure closed on success
            }
        }
    }

    pub fn record_assignment_failure(&mut self) {
        self.failure_count += 1;

        match self.state {
            CircuitState::Closed => {
                if self.failure_count >= self.config.failure_threshold {
                    self.state = CircuitState::Open;
                    self.last_state_change = Utc::now();
                    log::warn!("Assignment circuit breaker -> Open (too many failures)");
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open -> back to open
                self.state = CircuitState::Open;
                self.last_state_change = Utc::now();
                log::warn!("Assignment circuit breaker -> Open (half-open test failed)");
            }
            CircuitState::Open => {
                // Already open, stay open
            }
        }
    }
}

impl Default for AssignmentCircuitBreaker {
    fn default() -> Self {
        Self::new(AssignmentCircuitConfig::default())
    }
}

#[derive(Clone, Debug)]
pub struct ServiceState {
    // Identity
    pub service_type: ServiceType,
    pub instance_id: String,

    // Restart detection
    pub last_process_id: Option<String>,
    pub last_started_at: Option<DateTime<Utc>>,

    // Health tracking
    pub status: ServiceStatus,
    pub previous_status: Option<ServiceStatus>,
    pub last_heartbeat: DateTime<Utc>,
    pub consecutive_heartbeat_failures: u32,

    // Component checks
    pub component_status: HashMap<String, bool>, // redis_ok, ws_ok, vpn_ok, etc.

    // Game assignments (for shards only)
    pub assigned_games: HashSet<String>,

    // Circuit breaker state
    pub assignment_circuit: AssignmentCircuitBreaker,

    // Metrics from heartbeat
    pub metrics: HashMap<String, serde_json::Value>,
}

impl ServiceState {
    pub fn new(service_type: ServiceType, instance_id: String) -> Self {
        Self {
            service_type,
            instance_id,
            last_process_id: None,
            last_started_at: None,
            status: ServiceStatus::Starting,
            previous_status: None,
            last_heartbeat: Utc::now(),
            consecutive_heartbeat_failures: 0,
            component_status: HashMap::new(),
            assigned_games: HashSet::new(),
            assignment_circuit: AssignmentCircuitBreaker::default(),
            metrics: HashMap::new(),
        }
    }

    pub fn available_capacity(&self) -> usize {
        // For game shards, extract from metrics
        if let ServiceType::GameShard = self.service_type {
            let max_games = self.metrics
                .get("max_games")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let game_count = self.assigned_games.len();

            if game_count >= max_games {
                0
            } else {
                max_games - game_count
            }
        } else {
            0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DegradationSeverity {
    Warning,   // Non-critical check failed
    Critical,  // Critical check failed, service limping
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestratorNotification {
    ServiceRestarted {
        service: String,
        instance_id: String,
        old_started_at: String,
        new_started_at: String,
        timestamp: String,
    },

    ServiceResyncComplete {
        service: String,
        instance_id: String,
        games_resent: usize,
        duration_ms: u64,
        timestamp: String,
    },

    ServiceDegraded {
        service: String,
        instance_id: String,
        failed_checks: Vec<String>,
        severity: DegradationSeverity,
        timestamp: String,
    },

    ServiceRecovered {
        service: String,
        instance_id: String,
        was_degraded_for_secs: i64,
        timestamp: String,
    },

    CircuitBreakerOpened {
        service: String,
        instance_id: String,
        reason: String,
        failure_count: u32,
        timestamp: String,
    },

    CircuitBreakerClosed {
        service: String,
        instance_id: String,
        was_open_for_secs: i64,
        timestamp: String,
    },

    ServiceDead {
        service: String,
        instance_id: String,
        last_heartbeat: String,
        assigned_games: Vec<String>,
        timestamp: String,
    },
}
