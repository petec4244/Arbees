use anyhow::Result;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, oneshot};
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamMatchResult {
    pub is_match: bool,
    pub confidence: f64,
    pub method: String,
    pub reason: String,
}

#[derive(Serialize)]
struct MatchRequest {
    request_id: String,
    target_team: String,
    candidate_team: String,
    sport: String,
}

#[derive(Deserialize)]
struct MatchResponse {
    request_id: String,
    is_match: bool,
    confidence: f64,
    method: String,
    reason: String,
}

const REQUEST_CHANNEL: &str = "team:match:request";
const RESPONSE_PATTERN: &str = "team:match:response:*";

#[derive(Clone)]
pub struct TeamMatchingClient {
    client: redis::Client,
    pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<TeamMatchResult>>>>,
    cache: Arc<RwLock<HashMap<(String, String, String), TeamMatchResult>>>,
}

impl TeamMatchingClient {
    pub async fn new(redis_url: &str) -> Result<Self> {
        let client = redis::Client::open(redis_url)?;
        
        // Test connection
        let mut conn = client.get_async_connection().await?;
        let _: String = redis::cmd("PING").query_async(&mut conn).await?;
        
        let instance = Self {
            client: client.clone(),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            cache: Arc::new(RwLock::new(HashMap::new())),
        };
        
        // Spawn listener
        let listener = instance.clone();
        tokio::spawn(async move {
            listener.listen_loop().await;
        });
        
        Ok(instance)
    }

    async fn listen_loop(&self) {
        loop {
            match self.client.get_async_connection().await {
                Ok(conn) => {
                    let mut pubsub = conn.into_pubsub();
                    if let Err(e) = pubsub.psubscribe(RESPONSE_PATTERN).await {
                        error!("Failed to subscribe to team match responses: {}", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                    
                    info!("TeamMatchingClient listening for responses");
                    
                    use futures_util::StreamExt;
                    let mut stream = pubsub.on_message();
                    
                    while let Some(msg) = stream.next().await {
                        if let Ok(payload_str) = msg.get_payload::<String>() {
                            if let Ok(response) = serde_json::from_str::<MatchResponse>(&payload_str) {
                                let mut pending = self.pending_requests.lock().await;
                                if let Some(tx) = pending.remove(&response.request_id) {
                                    let result = TeamMatchResult {
                                        is_match: response.is_match,
                                        confidence: response.confidence,
                                        method: response.method,
                                        reason: response.reason,
                                    };
                                    let _ = tx.send(result);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("TeamMatchingClient Redis error: {}", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    pub async fn match_teams(&self, target: &str, candidate: &str, sport: &str) -> Result<Option<TeamMatchResult>> {
        let key = (target.to_lowercase(), candidate.to_lowercase(), sport.to_lowercase());
        
        // Check cache
        {
            let cache = self.cache.read().await;
            if let Some(res) = cache.get(&key) {
                return Ok(Some(res.clone()));
            }
        }
        
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        
        // Register pending request
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id.clone(), tx);
        }
        
        // Send request
        let request = MatchRequest {
            request_id,
            target_team: target.to_string(),
            candidate_team: candidate.to_string(),
            sport: sport.to_string(),
        };
        
        let mut conn = self.client.get_async_connection().await?;
        conn.publish::<_, _, ()>(REQUEST_CHANNEL, serde_json::to_string(&request)?).await?;
        
        // Wait for response with timeout
        match tokio::time::timeout(Duration::from_secs(2), rx).await {
            Ok(Ok(result)) => {
                // Update cache
                let mut cache = self.cache.write().await;
                cache.insert(key, result.clone());
                Ok(Some(result))
            }
            Ok(Err(_)) => {
                // Sender dropped
                Ok(None)
            }
            Err(_) => {
                // Timeout
                let mut pending = self.pending_requests.lock().await;
                pending.remove(&request.request_id);
                warn!("Team match timeout for {} vs {}", target, candidate);
                Ok(None)
            }
        }
    }
}
