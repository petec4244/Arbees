use crate::config::Config;
use crate::state::{
    AssignmentCircuitBreaker, AssignmentCircuitConfig, CircuitState, GameAssignment,
    OrchestratorNotification, ServiceState, ServiceStatus, ServiceType,
};
use anyhow::{anyhow, Result};
use arbees_rust_core::redis::bus::RedisBus;
use chrono::{DateTime, Utc};
use log::{debug, error, info, warn};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

const CHANNEL_SERVICE_HEALTH: &str = "notifications:service_health";
const CHANNEL_SERVICE_RESYNC: &str = "notifications:service_resync";
const CHANNEL_CIRCUIT_BREAKER: &str = "notifications:circuit_breaker";
const CHANNEL_DEGRADATION: &str = "notifications:degradation";

pub struct ServiceRegistry {
    services: Arc<RwLock<HashMap<String, ServiceState>>>,
    pending_resyncs: Arc<RwLock<VecDeque<String>>>,
    redis: Arc<RedisBus>,
    config: Config,
    assignments: Arc<RwLock<HashMap<String, GameAssignment>>>,
}

impl ServiceRegistry {
    pub fn new(redis: Arc<RedisBus>, config: Config) -> Self {
        Self {
            services: Arc::new(RwLock::new(HashMap::new())),
            pending_resyncs: Arc::new(RwLock::new(VecDeque::new())),
            redis,
            config,
            assignments: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn set_assignments(&self, assignments: Arc<RwLock<HashMap<String, GameAssignment>>>) {
        // Store reference to assignments for resync
        // Note: This is a workaround for the borrow checker. In production, consider
        // refactoring to pass assignments directly to resync methods.
    }

    /// Handle incoming heartbeat from a service
    pub async fn handle_heartbeat(&self, payload: Value) -> Result<()> {
        // Parse heartbeat payload
        let service_name = payload
            .get("service")
            .or_else(|| payload.get("shard_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing service or shard_id in heartbeat"))?;

        let instance_id = payload
            .get("instance_id")
            .or_else(|| payload.get("shard_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing instance_id in heartbeat"))?;

        let instance_key = format!("{}:{}", service_name, instance_id);

        // Determine service type
        let service_type = self.determine_service_type(service_name);

        let mut services = self.services.write().await;
        let state = services
            .entry(instance_key.clone())
            .or_insert_with(|| ServiceState::new(service_type.clone(), instance_id.to_string()));

        // Extract heartbeat fields
        let process_id = payload.get("process_id").and_then(|v| v.as_str()).map(String::from);

        let started_at = payload
            .get("started_at")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        // RESTART DETECTION
        let is_restart = self.detect_restart(state, &process_id, &started_at);

        // Collect notification data before dropping lock
        let notification_data = if is_restart {
            warn!(
                "Detected restart: {} (old_start={:?}, new_start={:?})",
                instance_key, state.last_started_at, started_at
            );

            // Mark for resync
            let mut pending = self.pending_resyncs.write().await;
            pending.push_back(instance_key.clone());

            // Reset circuit breaker (fresh start)
            state.assignment_circuit = AssignmentCircuitBreaker::new(AssignmentCircuitConfig {
                failure_threshold: self.config.assignment_circuit_failure_threshold,
                success_threshold: self.config.assignment_circuit_success_threshold,
                half_open_timeout: Duration::from_secs(self.config.assignment_circuit_half_open_timeout_secs),
            });

            // Clear stale game assignments (will resync)
            state.assigned_games.clear();

            // Collect data for notification
            Some((
                service_name.to_string(),
                instance_id.to_string(),
                state.last_started_at,
                started_at,
            ))
        } else {
            None
        };

        // Update state
        if let Some(state) = services.get_mut(&instance_key) {
            state.last_process_id = state.last_process_id.clone().or(process_id.clone());
            state.last_started_at = state.last_started_at.or(started_at);
            state.last_heartbeat = Utc::now();
            state.consecutive_heartbeat_failures = 0;

            // Update status
            if let Some(status_str) = payload.get("status").and_then(|v| v.as_str()) {
                state.status = match status_str {
                    "starting" => ServiceStatus::Starting,
                    "healthy" => ServiceStatus::Healthy,
                    "degraded" => ServiceStatus::Degraded,
                    "unhealthy" => ServiceStatus::Unhealthy,
                    "stopping" => ServiceStatus::Stopping,
                    _ => ServiceStatus::Healthy,
                };
            }

            // Update component checks
            if let Some(checks) = payload.get("checks").and_then(|v| v.as_object()) {
                state.component_status.clear();
                for (key, val) in checks {
                    if let Some(b) = val.as_bool() {
                        state.component_status.insert(key.clone(), b);
                    }
                }
            }

            // Update metrics
            if let Some(metrics) = payload.get("metrics").and_then(|v| v.as_object()) {
                state.metrics.clear();
                for (key, val) in metrics {
                    state.metrics.insert(key.clone(), val.clone());
                }
            }

            // For game shards, also store max_games and games list
            if let ServiceType::GameShard = state.service_type {
                if let Some(max_games) = payload.get("max_games") {
                    state.metrics.insert("max_games".to_string(), max_games.clone());
                }

                if let Some(games) = payload.get("games").and_then(|v| v.as_array()) {
                    let reported_games: HashSet<String> = games
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();

                    // Check for missing games (assignments that didn't stick)
                    let missing_games: Vec<String> = state
                        .assigned_games
                        .difference(&reported_games)
                        .cloned()
                        .collect();

                    for game_id in missing_games {
                        warn!("Game {} missing from shard {} heartbeat report", game_id, instance_id);

                        // Record assignment failure
                        state.assignment_circuit.record_assignment_failure();

                        // Remove from assigned set
                        state.assigned_games.remove(&game_id);
                    }
                }
            }
        }

        // Drop services lock before async notification
        drop(services);

        // Publish restart notification if needed
        if let Some((service, instance, old_start, new_start)) = notification_data {
            if let (Some(old), Some(new)) = (old_start, new_start) {
                self.publish_notification(&OrchestratorNotification::ServiceRestarted {
                    service,
                    instance_id: instance,
                    old_started_at: old.to_rfc3339(),
                    new_started_at: new.to_rfc3339(),
                    timestamp: Utc::now().to_rfc3339(),
                })
                .await;
            }
        }

        Ok(())
    }

    fn detect_restart(
        &self,
        state: &ServiceState,
        new_process_id: &Option<String>,
        new_started_at: &Option<DateTime<Utc>>,
    ) -> bool {
        // Check if process_id changed
        if let (Some(new_pid), Some(old_pid)) = (new_process_id, &state.last_process_id) {
            if new_pid != old_pid {
                return true;
            }
        }

        // Check if started_at changed
        if let (Some(new_start), Some(old_start)) = (new_started_at, &state.last_started_at) {
            if new_start != old_start {
                return true;
            }
        }

        false
    }

    fn determine_service_type(&self, service_name: &str) -> ServiceType {
        if service_name.contains("shard") {
            ServiceType::GameShard
        } else if service_name.contains("polymarket") {
            ServiceType::PolymarketMonitor
        } else if service_name.contains("kalshi") {
            ServiceType::KalshiMonitor
        } else {
            ServiceType::Other(service_name.to_string())
        }
    }

    /// Run health check on all services
    pub async fn check_health(&self) {
        let now = Utc::now();
        let timeout_secs = self.config.shard_timeout_secs as i64;

        // Collect notifications to send
        let mut notifications = Vec::new();

        {
            let mut services = self.services.write().await;

            for (_instance_key, state) in services.iter_mut() {
                let age_secs = now
                    .signed_duration_since(state.last_heartbeat)
                    .num_seconds();

                // Check if service is dead
                if age_secs >= timeout_secs && state.status != ServiceStatus::Dead {
                    warn!(
                        "Service {} marked as Dead (last heartbeat {} seconds ago)",
                        state.instance_id, age_secs
                    );

                    state.status = ServiceStatus::Dead;

                    // Collect notification data
                    let assigned_games: Vec<String> = state.assigned_games.iter().cloned().collect();
                    notifications.push(OrchestratorNotification::ServiceDead {
                        service: state.service_type.clone().to_string(),
                        instance_id: state.instance_id.clone(),
                        last_heartbeat: state.last_heartbeat.to_rfc3339(),
                        assigned_games,
                        timestamp: now.to_rfc3339(),
                    });
                } else if age_secs < timeout_secs && state.status == ServiceStatus::Dead {
                    // Service recovered!
                    info!("Service {} recovered (heartbeat resumed)", state.instance_id);
                    state.status = ServiceStatus::Healthy;

                    // Collect recovery notification
                    notifications.push(OrchestratorNotification::ServiceRecovered {
                        service: state.service_type.clone().to_string(),
                        instance_id: state.instance_id.clone(),
                        was_degraded_for_secs: age_secs,
                        timestamp: now.to_rfc3339(),
                    });
                }

                // Check for degradation
                let operational = self.is_service_operational(state);
                if !operational && state.status == ServiceStatus::Healthy {
                    state.status = ServiceStatus::Degraded;
                    warn!("Service {} degraded", state.instance_id);
                } else if operational && state.status == ServiceStatus::Degraded {
                    state.status = ServiceStatus::Healthy;
                    info!("Service {} recovered from degradation", state.instance_id);
                }
            }
        } // Lock dropped here

        // Publish notifications after lock is released
        for notification in notifications {
            self.publish_notification(&notification).await;
        }
    }

    fn is_service_operational(&self, state: &ServiceState) -> bool {
        match state.service_type {
            ServiceType::GameShard => {
                // Critical: Redis (for pub/sub)
                state.component_status.get("redis_ok").copied().unwrap_or(false)
            }
            ServiceType::PolymarketMonitor => {
                // Critical: Redis, VPN, WebSocket
                state.component_status.get("redis_ok").copied().unwrap_or(false)
                    && state.component_status.get("vpn_ok").copied().unwrap_or(false)
                    && state.component_status.get("ws_ok").copied().unwrap_or(false)
            }
            ServiceType::KalshiMonitor => {
                // Critical: Redis, WebSocket
                state.component_status.get("redis_ok").copied().unwrap_or(false)
                    && state.component_status.get("ws_ok").copied().unwrap_or(false)
            }
            ServiceType::Other(_) => {
                // Generic: Just check overall status
                matches!(state.status, ServiceStatus::Healthy | ServiceStatus::Starting)
            }
        }
    }

    /// Process pending service resyncs
    pub async fn process_pending_resyncs(&self, assignments: Arc<RwLock<HashMap<String, GameAssignment>>>) {
        let instance_key = {
            let mut pending = self.pending_resyncs.write().await;
            pending.pop_front()
        };

        if let Some(key) = instance_key {
            // Debounce: wait a bit for service to settle
            tokio::time::sleep(Duration::from_secs(self.config.resync_debounce_secs)).await;

            let state = {
                let services = self.services.read().await;
                services.get(&key).cloned()
            };

            if let Some(state) = state {
                self.resync_service(&state, assignments).await;
            }
        }
    }

    async fn resync_service(&self, state: &ServiceState, assignments: Arc<RwLock<HashMap<String, GameAssignment>>>) {
        match state.service_type {
            ServiceType::GameShard => {
                self.resync_game_shard(state, assignments).await;
            }
            ServiceType::PolymarketMonitor | ServiceType::KalshiMonitor => {
                self.resync_monitor(state, assignments).await;
            }
            ServiceType::Other(_) => {
                debug!("No resync needed for service type: {:?}", state.service_type);
            }
        }
    }

    async fn resync_game_shard(&self, state: &ServiceState, assignments: Arc<RwLock<HashMap<String, GameAssignment>>>) {
        info!("Resync: game_shard {}", state.instance_id);

        let games_for_shard: Vec<GameAssignment> = {
            let assignments = assignments.read().await;
            assignments
                .values()
                .filter(|a| a.shard_id == state.instance_id)
                .cloned()
                .collect()
        };

        if games_for_shard.is_empty() {
            info!("No games to resync for shard {}", state.instance_id);
            return;
        }

        info!(
            "Resending {} game assignments to shard {}",
            games_for_shard.len(),
            state.instance_id
        );

        let start_time = std::time::Instant::now();

        // Resend assignments with rate limiting
        for (i, assignment) in games_for_shard.iter().enumerate() {
            if i > 0 && i % 5 == 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            if let Err(e) = self.send_game_assignment_to_shard(&state.instance_id, assignment).await {
                warn!("Failed to resend game assignment during resync: {}", e);
            }
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Publish notification
        self.publish_notification(&OrchestratorNotification::ServiceResyncComplete {
            service: "game_shard".to_string(),
            instance_id: state.instance_id.clone(),
            games_resent: games_for_shard.len(),
            duration_ms,
            timestamp: Utc::now().to_rfc3339(),
        })
        .await;
    }

    async fn resync_monitor(&self, state: &ServiceState, assignments: Arc<RwLock<HashMap<String, GameAssignment>>>) {
        info!(
            "Resync: monitor {} (lazy mode - will catch up via broadcast)",
            state.instance_id
        );

        // Re-broadcast recent assignments for faster recovery
        let recent_assignments: Vec<GameAssignment> = {
            let assignments = assignments.read().await;
            assignments
                .values()
                .filter(|a| {
                    let age = Utc::now().signed_duration_since(a.assigned_at).num_minutes();
                    age < 30 // Only recent assignments (game likely still live)
                })
                .take(10)
                .cloned()
                .collect()
        };

        for assignment in recent_assignments {
            if let Err(e) = self.broadcast_market_assignment(&assignment).await {
                warn!("Failed to re-broadcast market assignment: {}", e);
            }
        }
    }

    async fn send_game_assignment_to_shard(&self, shard_id: &str, assignment: &GameAssignment) -> Result<()> {
        let channel = format!("shard:{}:command", shard_id);

        let command = json!({
            "type": "add_game",
            "game_id": assignment.game_id,
            "sport": assignment.sport,
            "kalshi_market_id": assignment.kalshi_market_id,
            "polymarket_market_id": assignment.polymarket_market_id,
            "market_ids_by_type": assignment.market_ids_by_type,
        });

        self.redis.publish(&channel, &command).await?;
        Ok(())
    }

    async fn broadcast_market_assignment(&self, assignment: &GameAssignment) -> Result<()> {
        let channel = "orchestrator:market_assignments";

        // Broadcast for polymarket
        if let Some(polymarket_id) = &assignment.polymarket_market_id {
            let message = json!({
                "type": "polymarket_assign",
                "game_id": assignment.game_id,
                "sport": assignment.sport,
                "markets": [{
                    "market_type": "moneyline",
                    "condition_id": polymarket_id,
                }]
            });
            self.redis.publish(channel, &message).await?;
        }

        // Broadcast for kalshi
        if let Some(kalshi_id) = &assignment.kalshi_market_id {
            let message = json!({
                "type": "kalshi_assign",
                "game_id": assignment.game_id,
                "sport": assignment.sport,
                "markets": [{
                    "market_type": "moneyline",
                    "ticker": kalshi_id,
                }]
            });
            self.redis.publish(channel, &message).await?;
        }

        Ok(())
    }

    /// Get all healthy game shards
    pub async fn get_healthy_shards(&self) -> Vec<ServiceState> {
        let services = self.services.read().await;
        services
            .values()
            .filter(|s| matches!(s.service_type, ServiceType::GameShard))
            .filter(|s| matches!(s.status, ServiceStatus::Healthy))
            .filter(|s| self.is_service_operational(s))
            .filter(|s| s.assignment_circuit.state == CircuitState::Closed)
            .cloned()
            .collect()
    }

    /// Get service status by instance ID
    pub async fn get_service_status(&self, instance_id: &str) -> Option<ServiceStatus> {
        let services = self.services.read().await;
        services
            .values()
            .find(|s| s.instance_id == instance_id)
            .map(|s| s.status)
    }

    async fn publish_notification(&self, notification: &OrchestratorNotification) {
        let channel = match notification {
            OrchestratorNotification::ServiceDegraded { .. }
            | OrchestratorNotification::ServiceRecovered { .. } => CHANNEL_DEGRADATION,
            OrchestratorNotification::ServiceRestarted { .. }
            | OrchestratorNotification::ServiceDead { .. } => CHANNEL_SERVICE_HEALTH,
            OrchestratorNotification::ServiceResyncComplete { .. } => CHANNEL_SERVICE_RESYNC,
            OrchestratorNotification::CircuitBreakerOpened { .. }
            | OrchestratorNotification::CircuitBreakerClosed { .. } => CHANNEL_CIRCUIT_BREAKER,
        };

        if let Ok(payload) = serde_json::to_value(notification) {
            if let Err(e) = self.redis.publish(channel, &payload).await {
                error!("Failed to publish notification: {}", e);
            }

            // Also log at appropriate level
            match notification {
                OrchestratorNotification::ServiceDead { .. } => error!("{:?}", notification),
                OrchestratorNotification::CircuitBreakerOpened { .. } => warn!("{:?}", notification),
                _ => info!("{:?}", notification),
            }
        }
    }
}

impl ToString for ServiceType {
    fn to_string(&self) -> String {
        match self {
            ServiceType::GameShard => "game_shard".to_string(),
            ServiceType::PolymarketMonitor => "polymarket_monitor".to_string(),
            ServiceType::KalshiMonitor => "kalshi_monitor".to_string(),
            ServiceType::Other(s) => s.clone(),
        }
    }
}
