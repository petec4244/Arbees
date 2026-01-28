# Comprehensive Fault Tolerance and Service Recovery System Design

## Executive Summary

Design a robust fault tolerance system for the Arbees orchestrator that detects service restarts, automatically resynchronizes state, monitors degradation, implements circuit breakers, and handles partial failures across heterogeneous services (Rust game_shards, Python monitors).

## Current Architecture Analysis

### Heartbeat Infrastructure
- **Rust game_shard**: Publishes to `shard:{shard_id}:heartbeat` every 10s with `{shard_id, game_count, max_games, games[], timestamp}`
- **Python monitors**: Use `HeartbeatPublisher` class, publish to `health:hb:{service}:{instance}` with `{service, instance_id, status, checks{}, metrics{}, started_at, timestamp}`
- **Orchestrator**: Subscribes to `shard:*:heartbeat`, marks unhealthy after 60s timeout

### Game Assignment Flow
- **game_shard**: Receives via Redis command `shard:{shard_id}:command` with `{"type": "add_game", ...}`
- **monitors**: Broadcast via `orchestrator:market_assignments` with `{"type": "polymarket_assign"|"kalshi_assign", ...}`
- **State**: In-memory only in orchestrator (`assignments: HashMap<game_id, GameAssignment>`)

### Current Gaps
1. **No restart detection** - Service comes back, but orchestrator doesn't know it's a fresh instance
2. **No state resync** - Recovered services sit idle until new games discovered
3. **No degradation alerts** - Can't distinguish "WebSocket down but alive" from "completely dead"
4. **No circuit breaker for assignments** - Continues assigning to unhealthy services
5. **No partial failure handling** - Binary healthy/unhealthy status

---

## 1. Enhanced Heartbeat Protocol

### Problem
Current heartbeat doesn't include restart detection signals or degradation details.

### Solution: Add Process Identity Fields

#### For Rust game_shard (backward-compatible additions):
```json
{
  "shard_id": "shard-1",
  "game_count": 5,
  "max_games": 20,
  "games": ["401618778", "401618779"],
  "timestamp": "2026-01-27T12:00:00Z",
  
  // NEW FIELDS
  "started_at": "2026-01-27T11:55:00Z",  // Process start time (unchanging)
  "process_id": "uuid-v4",                 // Generated at startup (unchanging)
  "version": "git-sha-or-build-tag",       // Optional: deployment version
  "status": "healthy",                     // "starting", "healthy", "degraded", "unhealthy"
  "checks": {                              // Component health checks
    "redis_ok": true,
    "espn_api_ok": true,
    "zmq_ok": true
  },
  "metrics": {                             // Runtime metrics
    "avg_poll_latency_ms": 150,
    "signals_generated_1m": 3
  }
}
```

#### For Python monitors (already using HeartbeatPublisher, no changes needed):
```json
{
  "service": "polymarket_monitor",
  "instance_id": "monitor-1",
  "status": "degraded",                    // Already has this
  "started_at": "2026-01-27T11:55:00Z",   // Already has this
  "timestamp": "2026-01-27T12:00:00Z",
  "checks": {                              // Already has this
    "redis_ok": true,
    "vpn_ok": true,
    "ws_ok": false   // <-- Degradation signal
  },
  "metrics": {                             // Already has this
    "subscriptions_active": 12,
    "prices_published_1m": 45
  },
  "version": "abc123def",
  "hostname": "container-hostname"
}
```

### Rationale
- `started_at` + `process_id`: Detect restarts (compare to last known values)
- `status`: Explicit health state (reduces ambiguity)
- `checks`: Component-level visibility (enables partial failure detection)
- `metrics`: Operational insight (enables performance-based decisions)
- **Backward compatible**: Old orchestrator ignores new fields, new orchestrator tolerates missing fields

---

## 2. Reconnection Detection Strategy

### Algorithm: Track Process Identity

#### Orchestrator State Per Service
```rust
struct ServiceState {
    // Identity
    service_type: ServiceType,  // GameShard, PolymarketMonitor, KalshiMonitor
    instance_id: String,
    
    // Restart detection
    last_process_id: Option<String>,
    last_started_at: Option<DateTime<Utc>>,
    
    // Health tracking
    status: ServiceStatus,
    last_heartbeat: DateTime<Utc>,
    consecutive_heartbeat_failures: u32,
    
    // Component checks
    component_status: HashMap<String, bool>,  // redis_ok, ws_ok, vpn_ok, etc.
    
    // Game assignments (for shards only)
    assigned_games: HashSet<String>,
    
    // Circuit breaker state
    assignment_circuit_state: CircuitState,   // Open/Closed/HalfOpen
    assignment_failures: u32,
    last_assignment_failure: Option<DateTime<Utc>>,
}

enum ServiceType {
    GameShard,
    PolymarketMonitor,
    KalshiMonitor,
    Other(String),
}

enum ServiceStatus {
    Starting,      // Just came online
    Healthy,       // All checks passing
    Degraded,      // Some checks failing but operational
    Unhealthy,     // Cannot perform primary function
    Stopping,      // Graceful shutdown
    Dead,          // No heartbeat for timeout period
}

enum CircuitState {
    Closed,        // Normal operation, assigning games
    HalfOpen,      // Testing recovery, limited assignments
    Open,          // Not assigning games due to failures
}
```

#### Restart Detection Logic
```rust
fn handle_heartbeat(&mut self, payload: Heartbeat) {
    let instance_key = format!("{}:{}", payload.service, payload.instance_id);
    
    let mut state = self.services.entry(instance_key.clone())
        .or_insert_with(|| ServiceState::new(payload.service.clone(), payload.instance_id.clone()));
    
    // RESTART DETECTION: Check if process_id or started_at changed
    let is_restart = if let (Some(new_pid), Some(old_pid)) = (&payload.process_id, &state.last_process_id) {
        new_pid != old_pid
    } else if let (Some(new_start), Some(old_start)) = (&payload.started_at, &state.last_started_at) {
        new_start != old_start
    } else {
        false
    };
    
    if is_restart {
        warn!("Detected restart: {} (old_start={:?}, new_start={:?})", 
              instance_key, state.last_started_at, payload.started_at);
        
        // Mark for resync
        self.pending_resyncs.push(instance_key.clone());
        
        // Reset assignment circuit breaker (fresh start)
        state.assignment_circuit_state = CircuitState::Closed;
        state.assignment_failures = 0;
        
        // Clear stale game assignments (will resync)
        state.assigned_games.clear();
    }
    
    // Update state
    state.last_process_id = payload.process_id.clone();
    state.last_started_at = payload.started_at;
    state.last_heartbeat = Utc::now();
    state.status = payload.status;
    state.component_status = payload.checks.clone();
    state.consecutive_heartbeat_failures = 0;
}
```

### Rationale
- **Reliable restart detection**: Process identity changes only on restart
- **Graceful degradation**: Distinguish new discovery from restart
- **State consistency**: Clear stale assignments before resync
- **Circuit breaker reset**: Give recovered service a fresh chance

---

## 3. State Resynchronization Strategy

### Immediate vs. Lazy Resync Tradeoff

| Approach | Pros | Cons |
|----------|------|------|
| **Immediate** | Fast recovery, minimizes gap | Thundering herd if many services restart, may resend games already assigned |
| **Lazy** | Smoother load, no duplicates | Slower recovery, recovered service idle until next discovery |
| **Hybrid** | Best of both worlds | More complex |

### Recommendation: Hybrid Approach

#### Resync Timing
1. **Immediate for critical services**: game_shard (holds game state)
2. **Lazy for monitors**: polymarket_monitor, kalshi_monitor (just price subscribers)
3. **Debounced**: Wait 5s after restart detection (allows service to settle)

#### Resync Implementation
```rust
async fn resync_service(&self, instance_key: &str) {
    let state = self.services.read().await.get(instance_key).cloned();
    let Some(state) = state else { return; };
    
    match state.service_type {
        ServiceType::GameShard => {
            self.resync_game_shard(&state).await;
        }
        ServiceType::PolymarketMonitor | ServiceType::KalshiMonitor => {
            self.resync_monitor(&state).await;
        }
        ServiceType::Other(_) => {
            // No resync needed for generic services
        }
    }
}

async fn resync_game_shard(&self, state: &ServiceState) {
    info!("Resync: game_shard {}", state.instance_id);
    
    // Get all games currently assigned to this shard (from orchestrator's master list)
    let games_for_shard: Vec<GameAssignment> = self.assignments.read().await
        .values()
        .filter(|a| a.shard_id == state.instance_id)
        .cloned()
        .collect();
    
    if games_for_shard.is_empty() {
        info!("No games to resync for shard {}", state.instance_id);
        return;
    }
    
    info!("Resending {} game assignments to shard {}", games_for_shard.len(), state.instance_id);
    
    // Resend assignments one by one (with rate limiting to avoid overwhelming)
    for (i, assignment) in games_for_shard.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;  // Rate limit
        }
        
        self.send_game_assignment_to_shard(&state.instance_id, assignment).await;
    }
    
    // Publish notification
    self.publish_notification(&json!({
        "type": "service_resync",
        "service": "game_shard",
        "instance_id": state.instance_id,
        "games_resent": games_for_shard.len(),
        "timestamp": Utc::now().to_rfc3339()
    })).await;
}

async fn resync_monitor(&self, state: &ServiceState) {
    info!("Resync: monitor {} (lazy mode - will catch up via broadcast)", state.instance_id);
    
    // For monitors, we DON'T resend all assignments immediately.
    // Instead, we rely on:
    // 1. Monitors subscribe to "orchestrator:market_assignments" broadcast
    // 2. Orchestrator will naturally broadcast new games as they're discovered
    // 3. If we need faster recovery, we could optionally re-broadcast last N assignments
    
    // Optional: Re-broadcast last 10 assignments for faster recovery
    let recent_assignments: Vec<GameAssignment> = self.assignments.read().await
        .values()
        .filter(|a| {
            let age = Utc::now().signed_duration_since(a.assigned_at).num_minutes();
            age < 30  // Only recent assignments (game likely still live)
        })
        .take(10)
        .cloned()
        .collect();
    
    for assignment in recent_assignments {
        self.broadcast_market_assignments(&assignment).await;
    }
}
```

### Rationale
- **Immediate shard resync**: Game shards hold critical state, need full context
- **Lazy monitor resync**: Monitors are stateless subscribers, catch up naturally
- **Rate limiting**: Prevent overwhelming recovered service or Redis
- **Recency filter**: Only resend active games (not completed ones)

---

## 4. Degradation Detection

### Problem
Current system only tracks "alive" or "dead" (heartbeat presence), not degraded states.

### Solution: Multi-Level Health Status

#### Status Determination Logic
```rust
fn determine_service_status(
    heartbeat_status: ServiceStatus,
    component_checks: &HashMap<String, bool>,
    last_heartbeat_age_secs: i64,
    timeout_secs: i64,
) -> ServiceStatus {
    // Dead = no heartbeat for timeout period
    if last_heartbeat_age_secs >= timeout_secs {
        return ServiceStatus::Dead;
    }
    
    // If heartbeat explicitly says unhealthy/stopping, believe it
    if matches!(heartbeat_status, ServiceStatus::Unhealthy | ServiceStatus::Stopping) {
        return heartbeat_status;
    }
    
    // Check component health
    let critical_checks_ok = match component_checks.get("redis_ok") {
        Some(&ok) => ok,
        None => true,  // Assume ok if not reported
    };
    
    if !critical_checks_ok {
        return ServiceStatus::Unhealthy;
    }
    
    // If any non-critical check fails, mark degraded
    let all_checks_ok = component_checks.values().all(|&ok| ok);
    if !all_checks_ok {
        return ServiceStatus::Degraded;
    }
    
    // Otherwise, use reported status
    heartbeat_status
}
```

#### Service-Specific Critical Checks
```rust
fn is_service_operational(&self, state: &ServiceState) -> bool {
    match state.service_type {
        ServiceType::GameShard => {
            // Critical: Redis (for pub/sub), ESPN API (for game state)
            state.component_status.get("redis_ok").copied().unwrap_or(false) &&
            state.component_status.get("espn_api_ok").copied().unwrap_or(true)
        }
        ServiceType::PolymarketMonitor => {
            // Critical: Redis (for pub), VPN (for CLOB access), WebSocket
            state.component_status.get("redis_ok").copied().unwrap_or(false) &&
            state.component_status.get("vpn_ok").copied().unwrap_or(false) &&
            state.component_status.get("ws_ok").copied().unwrap_or(false)
        }
        ServiceType::KalshiMonitor => {
            // Critical: Redis, WebSocket (no VPN needed)
            state.component_status.get("redis_ok").copied().unwrap_or(false) &&
            state.component_status.get("ws_ok").copied().unwrap_or(false)
        }
        ServiceType::Other(_) => {
            // Generic: Just check overall status
            matches!(state.status, ServiceStatus::Healthy | ServiceStatus::Starting)
        }
    }
}
```

### Rationale
- **Service-specific criteria**: Different services have different critical dependencies
- **Graceful degradation**: Distinguish "limping along" from "completely dead"
- **Actionable signals**: Orchestrator can make smarter decisions (e.g., don't assign to degraded shard)

---

## 5. Circuit Breaker Logic

### Existing Infrastructure
- Already have `ApiCircuitBreaker` for ESPN API resilience (in `rust_core/src/circuit_breaker.rs`)
- Can reuse pattern for game assignment decisions

### New: Assignment Circuit Breaker

#### Per-Service Circuit Breaker
```rust
struct AssignmentCircuitBreaker {
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    last_state_change: DateTime<Utc>,
    config: AssignmentCircuitConfig,
}

struct AssignmentCircuitConfig {
    failure_threshold: u32,        // Open after this many failures (default: 3)
    success_threshold: u32,        // Close after this many successes in half-open (default: 2)
    half_open_timeout: Duration,   // Wait before trying half-open (default: 30s)
}

impl AssignmentCircuitBreaker {
    fn can_assign(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if timeout elapsed, transition to half-open
                if self.last_state_change.elapsed_secs() >= self.config.half_open_timeout.as_secs() {
                    self.state = CircuitState::HalfOpen;
                    self.success_count = 0;
                    self.failure_count = 0;
                    info!("Assignment circuit breaker -> HalfOpen");
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,  // Allow limited assignments
        }
    }
    
    fn record_assignment_success(&mut self) {
        self.failure_count = 0;
        
        match self.state {
            CircuitState::HalfOpen => {
                self.success_count += 1;
                if self.success_count >= self.config.success_threshold {
                    self.state = CircuitState::Closed;
                    self.last_state_change = Utc::now();
                    info!("Assignment circuit breaker -> Closed (recovered)");
                }
            }
            _ => {
                self.state = CircuitState::Closed;  // Ensure closed on success
            }
        }
    }
    
    fn record_assignment_failure(&mut self) {
        self.failure_count += 1;
        
        match self.state {
            CircuitState::Closed => {
                if self.failure_count >= self.config.failure_threshold {
                    self.state = CircuitState::Open;
                    self.last_state_change = Utc::now();
                    warn!("Assignment circuit breaker -> Open (too many failures)");
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open -> back to open
                self.state = CircuitState::Open;
                self.last_state_change = Utc::now();
                warn!("Assignment circuit breaker -> Open (half-open test failed)");
            }
            CircuitState::Open => {
                // Already open, stay open
            }
        }
    }
}
```

#### Assignment Failure Detection
```rust
async fn assign_game_to_shard(&mut self, game: &GameInfo, shard_id: &str) -> Result<()> {
    let state = self.services.read().await.get(shard_id).cloned();
    let Some(mut state) = state else {
        return Err(anyhow!("Shard {} not found", shard_id));
    };
    
    // Check circuit breaker
    if !state.assignment_circuit.can_assign() {
        return Err(anyhow!("Assignment circuit breaker open for shard {}", shard_id));
    }
    
    // Check service health
    if !self.is_service_operational(&state) {
        state.assignment_circuit.record_assignment_failure();
        return Err(anyhow!("Shard {} not operational", shard_id));
    }
    
    // Send assignment command
    let result = self.send_game_command_to_shard(shard_id, game).await;
    
    match result {
        Ok(_) => {
            state.assignment_circuit.record_assignment_success();
            state.assigned_games.insert(game.game_id.clone());
            Ok(())
        }
        Err(e) => {
            state.assignment_circuit.record_assignment_failure();
            Err(e)
        }
    }
}
```

#### Implicit Assignment Verification
```rust
// In handle_shard_heartbeat (already exists):
async fn handle_shard_heartbeat(&mut self, payload: Heartbeat) {
    let shard_id = payload.instance_id.clone();
    let reported_games: HashSet<String> = payload.games.into_iter().collect();
    
    let state = self.services.read().await.get(&shard_id).cloned();
    let Some(mut state) = state else { return; };
    
    // Check for missing games (assignments that didn't stick)
    for assigned_game in &state.assigned_games {
        if !reported_games.contains(assigned_game) {
            warn!("Game {} missing from shard {} heartbeat report", assigned_game, shard_id);
            
            // This is a soft failure - assignment didn't work
            state.assignment_circuit.record_assignment_failure();
            
            // Remove from assigned set (will be retried in next discovery cycle)
            state.assigned_games.remove(assigned_game);
            self.assignments.write().await.remove(assigned_game);
        }
    }
}
```

### Rationale
- **Prevent thrashing**: Stop assigning to services that consistently fail
- **Auto-recovery**: Test recovered services gradually (half-open state)
- **Implicit verification**: Use heartbeat game lists as confirmation
- **Fail-fast**: Detect problems early, avoid wasting discovery resources

---

## 6. Notification System

### Notification Channels
```rust
// Redis channels for different notification types
const CHANNEL_SERVICE_HEALTH: &str = "notifications:service_health";
const CHANNEL_SERVICE_RESYNC: &str = "notifications:service_resync";
const CHANNEL_CIRCUIT_BREAKER: &str = "notifications:circuit_breaker";
const CHANNEL_DEGRADATION: &str = "notifications:degradation";
```

### Notification Types
```rust
#[derive(Serialize)]
enum OrchestratorNotification {
    ServiceRestarted {
        service: String,
        instance_id: String,
        old_started_at: DateTime<Utc>,
        new_started_at: DateTime<Utc>,
        timestamp: DateTime<Utc>,
    },
    
    ServiceResyncComplete {
        service: String,
        instance_id: String,
        games_resent: usize,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    
    ServiceDegraded {
        service: String,
        instance_id: String,
        failed_checks: Vec<String>,
        severity: DegradationSeverity,
        timestamp: DateTime<Utc>,
    },
    
    ServiceRecovered {
        service: String,
        instance_id: String,
        was_degraded_for_secs: i64,
        timestamp: DateTime<Utc>,
    },
    
    CircuitBreakerOpened {
        service: String,
        instance_id: String,
        reason: String,
        failure_count: u32,
        timestamp: DateTime<Utc>,
    },
    
    CircuitBreakerClosed {
        service: String,
        instance_id: String,
        was_open_for_secs: i64,
        timestamp: DateTime<Utc>,
    },
    
    ServiceDead {
        service: String,
        instance_id: String,
        last_heartbeat: DateTime<Utc>,
        assigned_games: Vec<String>,  // Games now orphaned
        timestamp: DateTime<Utc>,
    },
}

#[derive(Serialize)]
enum DegradationSeverity {
    Warning,   // Non-critical check failed
    Critical,  // Critical check failed, service limping
}
```

### Notification Publishing
```rust
async fn publish_notification(&self, notification: OrchestratorNotification) {
    let channel = match &notification {
        OrchestratorNotification::ServiceDegraded { .. } |
        OrchestratorNotification::ServiceRecovered { .. } => CHANNEL_DEGRADATION,
        
        OrchestratorNotification::ServiceRestarted { .. } |
        OrchestratorNotification::ServiceDead { .. } => CHANNEL_SERVICE_HEALTH,
        
        OrchestratorNotification::ServiceResyncComplete { .. } => CHANNEL_SERVICE_RESYNC,
        
        OrchestratorNotification::CircuitBreakerOpened { .. } |
        OrchestratorNotification::CircuitBreakerClosed { .. } => CHANNEL_CIRCUIT_BREAKER,
    };
    
    let payload = serde_json::to_string(&notification).unwrap();
    
    if let Err(e) = self.redis.publish(channel, &payload).await {
        error!("Failed to publish notification: {}", e);
    }
    
    // Also log at appropriate level
    match &notification {
        OrchestratorNotification::ServiceDegraded { severity, .. } => {
            match severity {
                DegradationSeverity::Warning => warn!("{}", payload),
                DegradationSeverity::Critical => error!("{}", payload),
            }
        }
        OrchestratorNotification::ServiceDead { .. } => error!("{}", payload),
        OrchestratorNotification::CircuitBreakerOpened { .. } => warn!("{}", payload),
        _ => info!("{}", payload),
    }
}
```

### Integration with Existing notification_service_rust
```rust
// notification_service_rust subscribes to these channels and forwards to Signal
// (Already has adaptive throttling to prevent alert fatigue)

// Example: Forward critical degradation to Signal
async fn handle_degradation_notification(&self, payload: Value) {
    let severity = payload.get("severity").and_then(|v| v.as_str()).unwrap_or("Warning");
    
    if severity == "Critical" {
        let service = payload.get("service").and_then(|v| v.as_str()).unwrap_or("unknown");
        let instance = payload.get("instance_id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let failed_checks = payload.get("failed_checks")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        
        let message = format!(
            "🚨 Service Degraded: {}:{}\nFailed checks: {}",
            service, instance, failed_checks
        );
        
        self.send_signal_notification(&message, NotificationPriority::High).await;
    }
}
```

### Rationale
- **Structured notifications**: Easy to consume by monitoring systems
- **Severity-based routing**: Critical issues get immediate alerts
- **Channel separation**: Consumers subscribe to relevant topics
- **Integration ready**: Works with existing notification_service_rust

---

## 7. Implementation Architecture

### New Components

#### 1. ServiceRegistry (orchestrator_rust)
```rust
// File: services/orchestrator_rust/src/managers/service_registry.rs

pub struct ServiceRegistry {
    services: Arc<RwLock<HashMap<String, ServiceState>>>,
    pending_resyncs: Arc<RwLock<VecDeque<String>>>,
    redis: redis::Client,
    config: ServiceRegistryConfig,
}

impl ServiceRegistry {
    // Core methods
    pub async fn handle_heartbeat(&self, payload: Heartbeat);
    pub async fn check_health(&self);
    pub async fn process_pending_resyncs(&self);
    
    // Query methods
    pub async fn get_healthy_shards(&self) -> Vec<ServiceState>;
    pub async fn get_service_status(&self, instance_id: &str) -> Option<ServiceStatus>;
    pub async fn get_all_services_summary(&self) -> ServiceHealthSummary;
    
    // Helper methods
    fn detect_restart(&self, state: &ServiceState, payload: &Heartbeat) -> bool;
    fn determine_service_status(&self, state: &ServiceState) -> ServiceStatus;
    async fn resync_service(&self, instance_key: &str);
}
```

#### 2. Update ShardManager (orchestrator_rust)
```rust
// File: services/orchestrator_rust/src/managers/shard_manager.rs

// REPLACE ShardManager with call to ServiceRegistry
// ShardManager becomes a facade/adapter over ServiceRegistry

pub struct ShardManager {
    service_registry: Arc<ServiceRegistry>,
    config: Config,
}

impl ShardManager {
    pub async fn get_best_shard(&self) -> Option<ShardInfo> {
        // Filter services by type=GameShard, status=Healthy, circuit=Closed
        let healthy_shards = self.service_registry.get_healthy_shards().await;
        
        healthy_shards.into_iter()
            .filter(|s| s.assignment_circuit.can_assign())
            .max_by_key(|s| s.available_capacity())
    }
}
```

#### 3. Update game_shard Heartbeat (game_shard_rust)
```rust
// File: services/game_shard_rust/src/shard.rs

// In heartbeat_loop(), add new fields:
async fn heartbeat_loop(&self) -> Result<()> {
    let channel = format!("shard:{}:heartbeat", self.shard_id);
    let process_id = Uuid::new_v4().to_string();  // Generate once at startup
    let started_at = self.started_at;  // Store in struct
    
    loop {
        let (game_ids, count) = {
            let games = self.games.lock().await;
            let ids = games.keys().cloned().collect::<Vec<_>>();
            (ids, games.len())
        };
        
        // Determine status based on health checks
        let redis_ok = self.redis.ping().await.is_ok();
        let espn_ok = self.espn_circuit_breaker.state() == ApiCircuitState::Closed;
        let zmq_ok = self.zmq_pub.is_some();
        
        let status = if redis_ok && espn_ok {
            "healthy"
        } else if redis_ok {
            "degraded"
        } else {
            "unhealthy"
        };
        
        let payload = json!({
            "shard_id": self.shard_id,
            "game_count": count,
            "max_games": self.max_games,
            "games": game_ids,
            "timestamp": Utc::now().to_rfc3339(),
            
            // NEW FIELDS
            "started_at": started_at.to_rfc3339(),
            "process_id": process_id,
            "version": env::var("BUILD_VERSION").unwrap_or_else(|_| "dev".to_string()),
            "status": status,
            "checks": {
                "redis_ok": redis_ok,
                "espn_api_ok": espn_ok,
                "zmq_ok": zmq_ok,
            },
            "metrics": {
                "avg_poll_latency_ms": self.get_avg_poll_latency(),
            },
        });
        
        if let Err(e) = self.redis.publish(&channel, &payload).await {
            warn!("Heartbeat publish error: {}", e);
        }
        
        tokio::time::sleep(self.heartbeat_interval).await;
    }
}
```

### Data Flow Diagram
```
┌────────────────────────────────────────────────────────────────┐
│                         Orchestrator                           │
│                                                                │
│  ┌─────────────────┐         ┌─────────────────┐             │
│  │ ServiceRegistry │◄────────┤ Heartbeat       │             │
│  │                 │         │ Listener        │             │
│  │ - Track process │         └─────────────────┘             │
│  │   identity      │                                          │
│  │ - Detect restart│         ┌─────────────────┐             │
│  │ - Track health  │◄────────┤ Health Check    │             │
│  │ - Circuit       │         │ Loop (15s)      │             │
│  │   breaker       │         └─────────────────┘             │
│  └────────┬────────┘                                          │
│           │                                                   │
│           │ Resync Trigger                                    │
│           ▼                                                   │
│  ┌─────────────────┐         ┌─────────────────┐             │
│  │ Resync Worker   │────────►│ Notification    │             │
│  │                 │         │ Publisher       │             │
│  │ - Debounced     │         └────────┬────────┘             │
│  │ - Rate limited  │                  │                      │
│  │ - Service-aware │                  │                      │
│  └────────┬────────┘                  │                      │
│           │                           │                      │
└───────────┼───────────────────────────┼──────────────────────┘
            │                           │
            │ Commands                  │ Notifications
            ▼                           ▼
     ┌─────────────┐           ┌──────────────────┐
     │ game_shard  │           │ notification_    │
     │ monitors    │           │ service_rust     │
     └─────────────┘           └──────────────────┘
```

### File Structure
```
services/orchestrator_rust/src/
├── main.rs                         # Add resync loop, update heartbeat subscription
├── config.rs                       # Add resync config (debounce, rate limits)
├── managers/
│   ├── service_registry.rs         # NEW: Core service tracking + restart detection
│   ├── shard_manager.rs            # UPDATE: Use ServiceRegistry
│   ├── game_manager.rs             # UPDATE: Use circuit breaker checks
│   └── notification_manager.rs     # NEW: Publish orchestrator notifications
└── state.rs                        # UPDATE: Add ServiceState, CircuitState enums

services/game_shard_rust/src/
└── shard.rs                        # UPDATE: Enhanced heartbeat format

shared/arbees_shared/models/
└── health.py                       # ALREADY EXISTS: Heartbeat, ServiceStatus models
```

---

## 8. Performance Impact Analysis

### Memory Overhead
- **Per-service tracking**: ~500 bytes per service (includes HashMaps for checks/metrics)
- **20 services**: ~10 KB total (negligible)

### CPU Overhead
- **Heartbeat processing**: +5% per heartbeat (parse new fields, restart detection logic)
- **Health check loop**: +20% (status determination logic)
- **Overall impact**: <1% of orchestrator CPU (heartbeats are 10s interval, health checks 15s)

### Redis Overhead
- **Heartbeat size increase**: ~200 bytes per heartbeat (new fields)
- **At 10s interval, 20 services**: 20 * 200 bytes / 10s = 400 bytes/s = negligible
- **Notification channels**: ~10 messages/hour expected (low volume)

### Network Overhead
- **Resync storm**: Up to 100 game assignments * 500 bytes = 50 KB burst
- **Mitigation**: Rate limited to 5 games/100ms = 50ms worst case
- **Impact**: Minimal (Redis handles 100K+ ops/sec)

---

## 9. Testing Strategy

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_restart_detection_via_process_id() {
        let registry = ServiceRegistry::new(...);
        
        // First heartbeat
        let hb1 = Heartbeat {
            instance_id: "shard-1".into(),
            process_id: Some("pid-1".into()),
            started_at: Some(Utc::now()),
            ..Default::default()
        };
        registry.handle_heartbeat(hb1).await;
        
        // Second heartbeat with new process_id (restart)
        let hb2 = Heartbeat {
            instance_id: "shard-1".into(),
            process_id: Some("pid-2".into()),  // Changed!
            started_at: Some(Utc::now()),
            ..Default::default()
        };
        registry.handle_heartbeat(hb2).await;
        
        // Should have triggered resync
        assert_eq!(registry.pending_resyncs.read().await.len(), 1);
    }
    
    #[tokio::test]
    async fn test_degradation_detection() {
        let registry = ServiceRegistry::new(...);
        
        let hb = Heartbeat {
            instance_id: "monitor-1".into(),
            service: "polymarket_monitor".into(),
            checks: hashmap! {
                "redis_ok".into() => true,
                "vpn_ok".into() => true,
                "ws_ok".into() => false,  // WebSocket down!
            },
            ..Default::default()
        };
        registry.handle_heartbeat(hb).await;
        
        let status = registry.get_service_status("monitor-1").await.unwrap();
        assert_eq!(status, ServiceStatus::Degraded);
    }
    
    #[tokio::test]
    async fn test_assignment_circuit_breaker() {
        let mut circuit = AssignmentCircuitBreaker::new(...);
        
        // Closed -> Open after 3 failures
        circuit.record_assignment_failure();
        circuit.record_assignment_failure();
        assert!(circuit.can_assign());
        
        circuit.record_assignment_failure();
        assert!(!circuit.can_assign());  // Now open
        
        // Wait for timeout, transitions to half-open
        tokio::time::sleep(Duration::from_secs(31)).await;
        assert!(circuit.can_assign());  // Half-open
        
        // Success in half-open -> closed
        circuit.record_assignment_success();
        circuit.record_assignment_success();
        assert_eq!(circuit.state, CircuitState::Closed);
    }
}
```

### Integration Tests
```bash
# Test restart detection end-to-end
docker-compose up -d orchestrator_rust game_shard
docker stop arbees-game-shard
# Wait 5s
docker start arbees-game-shard
# Check logs for "Detected restart" and "Resync complete"
docker logs arbees-orchestrator-rust | grep "Resync"

# Test degradation handling
docker exec arbees-polymarket-monitor killall -STOP polymarket_clob_api  # Simulate hang
# Wait 30s
docker logs arbees-orchestrator-rust | grep "degraded"
docker exec arbees-polymarket-monitor killall -CONT polymarket_clob_api  # Resume
# Wait 30s
docker logs arbees-orchestrator-rust | grep "recovered"
```

### Load Tests
- **Restart storm**: Restart all 10 game shards simultaneously, measure resync time
- **Degradation cascade**: Simulate 50% of services degraded, verify orchestrator behavior
- **Heartbeat flood**: Send 1000 heartbeats/sec, verify orchestrator CPU stays <10%

---

## 10. Rollout Plan

### Phase 1: Foundation (Week 1)
1. Implement `ServiceRegistry` with basic restart detection
2. Update `game_shard_rust` heartbeat format (backward-compatible)
3. Add unit tests
4. Deploy to staging, verify restart detection works

### Phase 2: Resynchronization (Week 2)
1. Implement resync logic (immediate for shards, lazy for monitors)
2. Add rate limiting and debouncing
3. Test with manual service restarts
4. Deploy to staging, measure resync performance

### Phase 3: Circuit Breaker (Week 3)
1. Implement `AssignmentCircuitBreaker`
2. Integrate with game assignment flow
3. Add implicit verification via heartbeat game lists
4. Test with flaky service scenarios
5. Deploy to staging

### Phase 4: Notifications (Week 4)
1. Implement notification publishing
2. Update `notification_service_rust` to subscribe to new channels
3. Add throttling to prevent alert fatigue
4. Test notification flow end-to-end
5. Deploy to production

### Phase 5: Monitoring & Tuning (Ongoing)
1. Add Grafana dashboards for service health
2. Tune circuit breaker thresholds based on production data
3. Add alerts for critical degradation events
4. Document runbooks for common failure scenarios

---

## 11. Configuration

### Environment Variables
```bash
# Orchestrator config
RESYNC_DEBOUNCE_SECS=5                   # Wait after restart before resync
RESYNC_RATE_LIMIT_GAMES_PER_SEC=10       # Max games/sec during resync
SERVICE_TIMEOUT_SECS=60                   # Mark dead after this timeout
ASSIGNMENT_CIRCUIT_FAILURE_THRESHOLD=3    # Open circuit after N failures
ASSIGNMENT_CIRCUIT_HALF_OPEN_TIMEOUT_SECS=30  # Try recovery after this
ASSIGNMENT_CIRCUIT_SUCCESS_THRESHOLD=2    # Close circuit after N successes

# game_shard config
HEARTBEAT_INTERVAL_SECS=10               # Existing
BUILD_VERSION=abc123def                  # Git SHA for version tracking
```

### Redis Key Patterns
```
health:hb:{service}:{instance}           # Heartbeat SETEX key (TTL=35s)
health:heartbeats                        # Heartbeat pub/sub channel
notifications:service_health             # Service health notifications
notifications:service_resync             # Resync notifications
notifications:circuit_breaker            # Circuit breaker notifications
notifications:degradation                # Degradation notifications
```

---

## 12. Trade-offs and Considerations

### Design Decisions

| Decision | Rationale | Trade-off |
|----------|-----------|-----------|
| **Hybrid resync** | Balance recovery speed vs. load | More complex than pure immediate/lazy |
| **Process ID vs. PID** | PID reuse risk, UUID safer | Requires generating UUID at startup |
| **Implicit verification** | Use existing heartbeat data | Slower failure detection than ACKs |
| **Service-specific health** | Accurate operational status | More complex status determination |
| **Circuit breaker per-service** | Isolate failures | More state to track |
| **Debounced resync** | Avoid resync thrash | Slightly slower recovery |

### Known Limitations
1. **No cross-orchestrator coordination**: If running multiple orchestrators (future), need distributed locking
2. **No persistent assignment log**: Assignments lost if orchestrator crashes (acceptable, will rediscover)
3. **No manual override**: Can't force-assign game to specific shard (future feature)
4. **No assignment ACKs**: Relies on heartbeat game lists for verification (implicit, works but slower)

### Future Enhancements
1. **Assignment ACKs**: Services explicitly ACK assignments (faster failure detection)
2. **Assignment persistence**: Store assignments in DB for orchestrator restart recovery
3. **Load shedding**: Gracefully reduce load when services degraded
4. **Predictive circuit breaking**: Open circuit preemptively based on latency trends
5. **Multi-orchestrator support**: Distributed coordination via Redis locks

---

## Summary

This design provides a comprehensive fault tolerance system with:

1. **Restart Detection**: Process identity tracking (process_id + started_at)
2. **Automatic Resync**: Hybrid immediate/lazy strategy with rate limiting
3. **Degradation Alerts**: Service-specific component health checks
4. **Circuit Breakers**: Per-service assignment circuit breakers with half-open recovery
5. **Partial Failure Handling**: Multi-level service status (Starting, Healthy, Degraded, Unhealthy, Dead)
6. **Notification System**: Structured notifications on dedicated Redis channels

**Implementation Effort**: ~4 weeks
**Risk**: Low (backward-compatible, incremental rollout)
**Impact**: High (significantly improves system reliability and observability)

---

### Critical Files for Implementation

- **P:\petes_code\ClaudeCode\Arbees\services\orchestrator_rust\src\managers\service_registry.rs** - NEW: Core service tracking, restart detection, resync orchestration. This is the central brain of the fault tolerance system.

- **P:\petes_code\ClaudeCode\Arbees\services\orchestrator_rust\src\managers\shard_manager.rs** - UPDATE: Refactor to use ServiceRegistry, add circuit breaker checks in get_best_shard()

- **P:\petes_code\ClaudeCode\Arbees\services\game_shard_rust\src\shard.rs** - UPDATE: Enhanced heartbeat format with process_id, started_at, status, checks, metrics (lines 827-850)

- **P:\petes_code\ClaudeCode\Arbees\services\orchestrator_rust\src\state.rs** - UPDATE: Add ServiceState struct, CircuitState enum, AssignmentCircuitBreaker struct

- **P:\petes_code\ClaudeCode\Arbees\services\orchestrator_rust\src\main.rs** - UPDATE: Wire up ServiceRegistry, add resync loop task, update heartbeat subscription to handle both formats

ENDOFPLAN
