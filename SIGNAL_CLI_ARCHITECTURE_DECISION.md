# Signal CLI Integration Architecture Decision

**Context:** How should notification_service_rust communicate with signal-cli?

**Options:**
1. HTTP to sidecar container (signal-cli-rest-api)
2. Subprocess spawning from Rust
3. Direct D-Bus communication
4. Native Rust Signal library

---

## 📊 **Option Comparison**

### **Option 1: HTTP Sidecar (RECOMMENDED)**

**Architecture:**
```
notification_service_rust → HTTP → signal-cli-rest-api container → Signal Protocol
```

**Pros:**
- ✅ **Clean separation** (Rust service doesn't need Java/signal-cli)
- ✅ **Small Rust image** (~20MB vs ~400MB with Java)
- ✅ **Independent scaling** (can restart Rust without losing Signal state)
- ✅ **Signal registration persists** (stored in signal-cli volume)
- ✅ **Easy testing** (can curl the API directly)
- ✅ **Well-maintained** (bbernhard/signal-cli-rest-api is popular)
- ✅ **Multiple services can share** (other services could send notifications)
- ✅ **Simpler error handling** (HTTP errors are standard)
- ✅ **No process management** (Docker handles lifecycle)

**Cons:**
- ⚠️ Tiny latency overhead (~1-5ms for HTTP)
- ⚠️ One more container to manage (but minimal overhead)

**Implementation:**
```rust
// Already in the plan - super simple
pub struct SignalClient {
    client: reqwest::Client,
    base_url: String, // "http://signal-cli:8080"
}

impl SignalClient {
    pub async fn send_message(&self, recipients: &[String], msg: &str) -> Result<()> {
        let resp = self.client
            .post(format!("{}/v2/send", self.base_url))
            .json(&json!({
                "number": self.sender_number,
                "recipients": recipients,
                "message": msg,
            }))
            .send()
            .await?;
        
        resp.error_for_status()?;
        Ok(())
    }
}
```

**Docker Compose:**
```yaml
services:
  signal-cli:
    image: bbernhard/signal-cli-rest-api:latest
    volumes:
      - signal_data:/home/.local/share/signal-cli
    ports:
      - "8080:8080"
    restart: unless-stopped

  notification_service:
    environment:
      SIGNAL_CLI_URL: http://signal-cli:8080
    depends_on:
      - signal-cli
```

**Resource Usage:**
- Rust service: ~5MB RAM
- signal-cli sidecar: ~50MB RAM (Java)
- **Total: ~55MB RAM**

---

### **Option 2: Subprocess Spawning**

**Architecture:**
```
notification_service_rust → spawn signal-cli process → Signal Protocol
```

**Pros:**
- ✅ Single container (one less service)
- ✅ Slightly lower latency (no HTTP)

**Cons:**
- ❌ **HUGE Docker image** (~400MB - needs Java + signal-cli)
- ❌ **Complex process management** (handle signal-cli crashes)
- ❌ **Slower startup** (spawn Java process each send = ~500ms)
- ❌ **More memory** (~100MB+ for Java process)
- ❌ **Harder to debug** (no API to test directly)
- ❌ **Signal state in Rust container** (lose on Rust restart)
- ❌ **Error handling messy** (parse stdout/stderr)

**Implementation:**
```rust
use tokio::process::Command;

pub async fn send_message(&self, recipient: &str, msg: &str) -> Result<()> {
    let output = Command::new("signal-cli")
        .args(&["-u", &self.sender_number, "send", "-m", msg, recipient])
        .output()
        .await?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("signal-cli failed: {}", stderr);
    }
    
    Ok(())
}
```

**Dockerfile:**
```dockerfile
FROM rust:1.75-slim as builder
# ... build Rust binary ...

FROM debian:bookworm-slim
# Install Java (HUGE!)
RUN apt-get update && apt-get install -y \
    openjdk-17-jre-headless \  # +300MB
    wget \
    && wget signal-cli.tar.gz \  # +50MB
    && tar xf signal-cli.tar.gz

COPY --from=builder /app/target/release/notification_service /usr/local/bin/

# Final image: ~400MB (vs 20MB with HTTP sidecar!)
```

**Resource Usage:**
- Container: ~120MB RAM (Rust + Java process)
- **Total: ~120MB RAM**

---

### **Option 3: D-Bus Communication**

**Architecture:**
```
notification_service_rust → D-Bus → signal-cli daemon → Signal Protocol
```

**Pros:**
- ✅ Low latency (IPC)
- ✅ signal-cli runs as daemon (not spawned per-send)

**Cons:**
- ❌ **Very complex** (need D-Bus Rust bindings)
- ❌ **Still needs Java** (signal-cli is Java)
- ❌ **Platform-specific** (D-Bus primarily Linux)
- ❌ **Harder to debug** (no simple API)
- ❌ **Still large image** (~400MB)

**Implementation complexity: HIGH**

---

### **Option 4: Native Rust Signal Library**

**Architecture:**
```
notification_service_rust → libsignal-client (Rust) → Signal Protocol
```

**Pros:**
- ✅ Pure Rust (no Java!)
- ✅ Small image (~30MB)
- ✅ Full control

**Cons:**
- ❌ **VERY COMPLEX** (implement Signal protocol yourself)
- ❌ **Lots of code** (registration, encryption, key management)
- ❌ **Maintenance burden** (Signal protocol changes)
- ❌ **Security risk** (crypto is hard to get right)
- ❌ **Weeks of work** (vs hours for HTTP)

**Libraries:**
- `libsignal-client` - Official Signal library (C/Rust bindings)
- But: Still need to implement full registration flow, device linking, message sending, etc.

**Implementation complexity: VERY HIGH**

---

## 🎯 **Recommendation: HTTP Sidecar (Option 1)**

### **Why HTTP Sidecar Wins:**

1. **Simplicity:**
   - 5 lines of Rust code to send message
   - No process management
   - No Java in Rust container

2. **Maintainability:**
   - Well-tested Docker image (`bbernhard/signal-cli-rest-api`)
   - 3.5K+ GitHub stars, actively maintained
   - Many users, bugs already found/fixed

3. **Separation of Concerns:**
   - Rust service: business logic
   - signal-cli: Signal protocol complexity
   - Can update/restart independently

4. **Resource Efficiency:**
   - Rust container: 20MB image, 5MB RAM
   - signal-cli sidecar: 50MB RAM (shared if multiple services use it)
   - vs 400MB image + 120MB RAM for subprocess

5. **Operational Benefits:**
   - Can test Signal separately: `curl http://signal-cli:8080/v2/send`
   - Signal registration persists in volume (survives Rust restarts)
   - Easy to add features (webhooks, group messages, attachments)

6. **Future-Proof:**
   - Other services can send notifications (analytics_service, cron jobs, etc.)
   - Can add rate limiting in signal-cli
   - Can monitor signal-cli health separately

---

## 📊 **Resource Comparison**

| Approach | Rust Image | Rust RAM | signal-cli RAM | Total RAM | Complexity |
|----------|------------|----------|----------------|-----------|------------|
| **HTTP Sidecar** | **20MB** | **5MB** | **50MB** | **55MB** | **Low** |
| Subprocess | 400MB | 120MB | - | 120MB | Medium |
| D-Bus | 400MB | 100MB | 50MB | 150MB | High |
| Native Rust | 30MB | 10MB | - | 10MB | Very High |

**HTTP Sidecar: Best balance of simplicity, maintainability, and resources.**

---

## 🏗️ **Recommended Architecture**

### **Docker Compose:**

```yaml
services:
  # Signal CLI - Shared by all services that need notifications
  signal-cli:
    image: bbernhard/signal-cli-rest-api:0.80
    container_name: arbees-signal-cli
    environment:
      MODE: json-rpc
    ports:
      - "8080:8080"
    volumes:
      - signal_data:/home/.local/share/signal-cli
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/v1/about"]
      interval: 30s
      timeout: 10s
      retries: 3
    restart: unless-stopped
    profiles:
      - full
      - notifications

  # Notification Service - Lightweight Rust daemon
  notification_service:
    build:
      context: .
      dockerfile: services/notification_service_rust/Dockerfile
    container_name: arbees-notifications
    depends_on:
      signal-cli:
        condition: service_healthy
    environment:
      SIGNAL_CLI_URL: http://signal-cli:8080
      SIGNAL_PHONE: ${SIGNAL_PHONE}
      # ... other config ...
    restart: unless-stopped
    profiles:
      - full
      - notifications

volumes:
  signal_data:  # Persists Signal registration
```

---

## 🔧 **Implementation**

### **Rust Client (Already in Plan):**

```rust
pub struct SignalClient {
    client: reqwest::Client,
    base_url: String,
    sender_number: String,
}

impl SignalClient {
    pub fn new(base_url: String, sender_number: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            base_url,
            sender_number,
        }
    }

    pub async fn send_message(
        &self,
        recipients: &[String],
        message: &str,
    ) -> Result<()> {
        let url = format!("{}/v2/send", self.base_url);

        let body = json!({
            "number": self.sender_number,
            "recipients": recipients,
            "message": message,
        });

        let resp = self.client
            .post(&url)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Signal send failed: {} - {}", status, text);
        }

        Ok(())
    }
}
```

**That's it! Super simple.**

---

## 🧪 **Testing**

### **Test Signal CLI Directly:**
```bash
# Check if alive
curl http://localhost:8080/v1/about

# Send test message
curl -X POST http://localhost:8080/v2/send \
  -H "Content-Type: application/json" \
  -d '{
    "number": "+1234567890",
    "recipients": ["+10987654321"],
    "message": "Test from curl!"
  }'
```

### **Test from Rust:**
```rust
#[tokio::test]
async fn test_signal_send() {
    let client = SignalClient::new(
        "http://localhost:8080".to_string(),
        "+1234567890".to_string(),
    );

    client.send_message(
        &["+10987654321".to_string()],
        "Test from Rust!"
    ).await.unwrap();
}
```

---

## 🚀 **Alternative: If You REALLY Want Single Container**

If you absolutely need single container (e.g., for AWS Lambda), use subprocess BUT with optimizations:

### **Optimized Subprocess Approach:**

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::process::Command;

pub struct SignalClient {
    sender_number: String,
    // Keep signal-cli process alive between sends
    daemon_process: Arc<Mutex<Option<tokio::process::Child>>>,
}

impl SignalClient {
    pub async fn start_daemon(&self) -> Result<()> {
        let mut process = self.daemon_process.lock().await;
        
        if process.is_none() {
            let child = Command::new("signal-cli")
                .args(&["-u", &self.sender_number, "daemon"])
                .spawn()?;
            
            *process = Some(child);
        }
        
        Ok(())
    }

    pub async fn send_message(&self, recipient: &str, msg: &str) -> Result<()> {
        // Use daemon (faster than spawning each time)
        let output = Command::new("signal-cli")
            .args(&[
                "-u", &self.sender_number,
                "send",
                "-m", msg,
                recipient
            ])
            .output()
            .await?;
        
        // ... error handling ...
        Ok(())
    }
}
```

**But:** Still need Java in Docker image (~400MB), still complex process management.

**Verdict:** Not worth it compared to HTTP sidecar.

---

## 💡 **Other Considerations**

### **What if signal-cli crashes?**

**HTTP Sidecar:**
- Docker restarts it automatically (`restart: unless-stopped`)
- Rust service just gets HTTP error, retries
- Signal registration persists in volume

**Subprocess:**
- Rust service must detect crash
- Rust service must respawn process
- More complex error handling

### **What about latency?**

**HTTP overhead:** ~1-5ms
**Signal protocol overhead:** ~500-2000ms (network to Signal servers)

**1-5ms is negligible compared to 500-2000ms.**

---

## 🎯 **Final Recommendation**

### **Use HTTP Sidecar (Option 1)**

**Why:**
1. ✅ Simplest implementation (5 lines of Rust)
2. ✅ Smallest Rust container (20MB)
3. ✅ Best separation of concerns
4. ✅ Easy to test and debug
5. ✅ Well-maintained library
6. ✅ Future-proof (multiple services can use it)

**Updated Plan:**
- Already written in `PLANNING_PROMPT_NOTIFICATION_SERVICE_RUST.md`
- Uses `bbernhard/signal-cli-rest-api` Docker image
- Rust sends HTTP POST to `http://signal-cli:8080/v2/send`
- Total overhead: ~55MB RAM (5MB Rust + 50MB signal-cli)

**This is the right choice!** 🎯

---

## 📋 **Comparison Summary**

| Criteria | HTTP Sidecar | Subprocess | Native Rust |
|----------|--------------|------------|-------------|
| **Simplicity** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐ |
| **Rust Image Size** | ⭐⭐⭐⭐⭐ (20MB) | ⭐ (400MB) | ⭐⭐⭐⭐ (30MB) |
| **Total RAM** | ⭐⭐⭐⭐ (55MB) | ⭐⭐⭐ (120MB) | ⭐⭐⭐⭐⭐ (10MB) |
| **Maintainability** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐ |
| **Development Time** | ⭐⭐⭐⭐⭐ (1hr) | ⭐⭐⭐ (4hr) | ⭐ (weeks) |
| **Debuggability** | ⭐⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐ |
| **Error Handling** | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐⭐ |

**Winner: HTTP Sidecar** 🏆

---

**Stick with the HTTP sidecar approach in the plan - it's the right choice!** ✅
