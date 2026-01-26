# Signal CLI Docker Compose - Configuration Comparison

## 🔍 **Your Version vs Recommended Version**

### **Your Current Configuration:**
```yaml
signal-cli-rest-api:
  image: bbernhard/signal-cli-rest-api:latest
  container_name: arbees-signal-cli-rest-api
  environment:
    - MODE=native  # ⚠️ Different
  ports:
    - "9922:8080"  # ⚠️ Different port
  volumes:
    - signal_cli_data:/home/.local/share/signal-cli  # ✅ Same
  restart: unless-stopped  # ✅ Same
  profiles:
    - full  # ✅ Same
```

### **Recommended Configuration:**
```yaml
signal-cli:
  image: bbernhard/signal-cli-rest-api:0.80  # ⚠️ Pinned version
  container_name: arbees-signal-cli
  environment:
    - MODE=json-rpc  # ⚠️ Different mode
  ports:
    - "8080:8080"    # ⚠️ Standard port
  volumes:
    - signal_data:/home/.local/share/signal-cli  # ✅ Same concept
  restart: unless-stopped  # ✅ Same
  profiles:
    - full  # ✅ Same
```

---

## 📊 **Differences Explained**

### **1. Service Name**
```yaml
# Yours:
signal-cli-rest-api:

# Recommended:
signal-cli:
```

**Verdict:** Either works, but **`signal-cli`** is shorter/cleaner.

**Impact:** Low - just affects service name in commands
- Yours: `docker-compose logs signal-cli-rest-api`
- Recommended: `docker-compose logs signal-cli`

---

### **2. MODE Setting** ⚠️ **IMPORTANT**

```yaml
# Yours:
MODE=native

# Recommended:
MODE=json-rpc
```

**What's the difference?**

| Mode | Description | API Endpoint |
|------|-------------|--------------|
| `native` | Uses native signal-cli binary | `/v1/send` |
| `json-rpc` | Uses JSON-RPC protocol | `/v2/send` |

**Verdict:** **`json-rpc` is recommended** (newer, better maintained)

**Why json-rpc is better:**
- ✅ Newer API version (v2 vs v1)
- ✅ Better error handling
- ✅ More features (attachments, receipts, etc.)
- ✅ Actively maintained

**But:** `native` mode **still works fine** for basic sending!

**Impact:** Medium - affects which API endpoint you use
- Native mode: `POST /v1/send`
- JSON-RPC mode: `POST /v2/send`

---

### **3. Port Mapping**

```yaml
# Yours:
ports:
  - "9922:8080"

# Recommended:
ports:
  - "8080:8080"
```

**Verdict:** **Your version is actually SAFER!**

**Why 9922 might be better:**
- ✅ Avoids conflict if port 8080 is already used
- ✅ Less common port (security through obscurity)
- ✅ Custom port signals "this is internal"

**Why 8080 is simpler:**
- ✅ Standard port (easier to remember)
- ✅ Common convention

**Impact:** Low - just change the URL
- Yours: `http://localhost:9922/...`
- Recommended: `http://localhost:8080/...`

---

### **4. Image Version**

```yaml
# Yours:
image: bbernhard/signal-cli-rest-api:latest

# Recommended:
image: bbernhard/signal-cli-rest-api:0.80
```

**Verdict:** **Pinned version (0.80) is better for production**

**Why pinned version is better:**
- ✅ Reproducible builds (always same version)
- ✅ Won't break on auto-update
- ✅ Control when to upgrade

**Why `latest` might be okay:**
- ✅ Auto-updates (gets new features)
- ⚠️ Might break unexpectedly

**Impact:** Low for now, Medium for production

---

### **5. Volume Name**

```yaml
# Yours:
signal_cli_data

# Recommended:
signal_data
```

**Verdict:** Either works! Just naming preference.

**Impact:** None (just a name)

---

## 🎯 **Recommendation: Keep Yours with Small Tweaks**

### **Option A: Minimal Changes (RECOMMENDED)**

**Just change MODE to json-rpc:**

```yaml
signal-cli-rest-api:
  image: bbernhard/signal-cli-rest-api:latest
  container_name: arbees-signal-cli-rest-api
  environment:
    - MODE=json-rpc  # ← Changed from native
  ports:
    - "9922:8080"    # ← Keep your port (it's fine!)
  volumes:
    - signal_cli_data:/home/.local/share/signal-cli
  restart: unless-stopped
  profiles:
    - full
```

**Why this is best:**
- ✅ Keeps your custom port (9922)
- ✅ Uses better API mode (json-rpc)
- ✅ Minimal changes

**Adjust Rust code to use your port:**
```rust
// In config.rs or main.rs
let signal_cli_url = std::env::var("SIGNAL_CLI_URL")
    .unwrap_or_else(|_| "http://signal-cli-rest-api:8080".to_string());
```

**And in .env:**
```bash
SIGNAL_CLI_URL=http://signal-cli-rest-api:9922  # Note: internal Docker uses 8080
```

**Wait, correction:**

Inside Docker network, containers talk on their **internal ports**, not the mapped ports!

```bash
# From host (your machine):
SIGNAL_CLI_URL=http://localhost:9922  # Use mapped port

# From inside Docker (notification_service):
SIGNAL_CLI_URL=http://signal-cli-rest-api:8080  # Use internal port!
```

So your config should be:

```yaml
# docker-compose.yml
signal-cli-rest-api:
  ports:
    - "9922:8080"  # External:Internal

notification_service:
  environment:
    SIGNAL_CLI_URL: http://signal-cli-rest-api:8080  # Use internal port!
```

---

### **Option B: Full Recommended (If Starting Fresh)**

```yaml
signal-cli:
  image: bbernhard/signal-cli-rest-api:0.80
  container_name: arbees-signal-cli
  environment:
    - MODE=json-rpc
  ports:
    - "8080:8080"
  volumes:
    - signal_data:/home/.local/share/signal-cli
  restart: unless-stopped
  profiles:
    - full
```

**Only if:** You want to match the plan exactly.

---

## 🔧 **Updated Setup Instructions for Your Config**

### **Step 1: Update MODE (if you want json-rpc)**

```yaml
environment:
  - MODE=json-rpc  # Change from native
```

---

### **Step 2: Start signal-cli**

```bash
docker-compose up -d signal-cli-rest-api

# Test (using YOUR port 9922)
curl http://localhost:9922/v1/about
```

---

### **Step 3: Register**

```bash
# Same commands, just use your container name
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 register --captcha "TOKEN"
```

---

### **Step 4: Verify**

```bash
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 verify 123456
```

---

### **Step 5: Test Send**

**If MODE=native (your current):**
```bash
curl -X POST http://localhost:9922/v1/send \
  -H "Content-Type: application/json" \
  -d '{
    "number": "+1234567890",
    "recipients": ["+10987654321"],
    "message": "Test!"
  }'
```

**If MODE=json-rpc (recommended):**
```bash
curl -X POST http://localhost:9922/v2/send \
  -H "Content-Type: application/json" \
  -d '{
    "number": "+1234567890",
    "recipients": ["+10987654321"],
    "message": "Test!"
  }'
```

---

### **Step 6: Update Rust Code**

**In notification_service_rust/src/signal_client.rs:**

```rust
impl SignalClient {
    pub async fn send_message(&self, recipients: &[String], message: &str) -> Result<()> {
        // Use v2 API if MODE=json-rpc, v1 if MODE=native
        let endpoint = if self.base_url.contains("json-rpc") {
            "v2/send"
        } else {
            "v1/send"  // For native mode
        };
        
        let url = format!("{}/{}", self.base_url, endpoint);
        
        // ... rest of implementation
    }
}
```

**Or simpler - just always use v2 after changing MODE to json-rpc:**

```rust
let url = format!("{}/v2/send", self.base_url);
```

---

### **Step 7: Update .env**

```bash
# Note: Inside Docker, use container name + internal port
SIGNAL_CLI_URL=http://signal-cli-rest-api:8080

# NOT: http://localhost:9922 (that's only from host machine)
```

---

## 📊 **Final Recommendation**

### **Best Configuration (Keep Yours, Small Tweak):**

```yaml
signal-cli-rest-api:
  image: bbernhard/signal-cli-rest-api:0.80  # ← Pin version
  container_name: arbees-signal-cli-rest-api
  environment:
    - MODE=json-rpc  # ← Use json-rpc
  ports:
    - "9922:8080"    # ← Keep your custom port
  volumes:
    - signal_cli_data:/home/.local/share/signal-cli
  restart: unless-stopped
  profiles:
    - full
    - notifications  # ← Add this profile too
```

**Why:**
- ✅ Uses better API mode (json-rpc)
- ✅ Pins version (reproducible)
- ✅ Keeps your port (9922 is fine)
- ✅ Adds notifications profile

---

## ✅ **Summary**

| Setting | Yours | Recommended | Keep/Change? |
|---------|-------|-------------|--------------|
| Service name | `signal-cli-rest-api` | `signal-cli` | **Keep** (either works) |
| MODE | `native` | `json-rpc` | **Change to json-rpc** |
| Port | `9922:8080` | `8080:8080` | **Keep 9922** (it's safer) |
| Image | `latest` | `0.80` | **Change to 0.80** (production) |
| Volume | `signal_cli_data` | `signal_data` | **Keep** (just a name) |

**Bottom line:** Your config is 95% correct! Just update MODE to `json-rpc` and optionally pin the version.

---

## 🚀 **Quick Fix**

```bash
# 1. Edit docker-compose.yml - change MODE to json-rpc
# 2. Rebuild
docker-compose up -d signal-cli-rest-api

# 3. Test with v2 API
curl http://localhost:9922/v2/send ...

# 4. Update Rust code to use v2/send endpoint

# Done!
```

**Your config is fine - just use `MODE=json-rpc` and you're golden!** ✅
