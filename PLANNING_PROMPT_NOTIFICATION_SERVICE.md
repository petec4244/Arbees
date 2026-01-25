# Notification Service Implementation Plan - Signal Integration

**Context:** Need real-time alerts for trading activity (entries, exits, P&L, errors) via Signal messenger

**Goal:** Highly configurable notification system with quiet hours, priority levels, and rate limiting

**Timeline:** 3-4 hours implementation

---

## 🎯 **Why Signal?**

### **Advantages:**
- ✅ **End-to-end encrypted** (secure trading alerts)
- ✅ **No phone number exposure** (uses Signal API)
- ✅ **Rich formatting** (markdown, emojis)
- ✅ **Free API** (no costs like Twilio)
- ✅ **Mobile + Desktop** (get alerts anywhere)
- ✅ **Group support** (can alert team if needed)

### **Alternatives Considered:**
- ❌ Slack: Webhook rate limits, requires workspace
- ❌ Discord: Gaming-focused, less professional
- ❌ Telegram: Not E2E encrypted by default
- ❌ Email: Gets lost in inbox, slow
- ❌ SMS: Costs money, character limits

---

## 🏗️ **Architecture**

### **Component Structure:**

```
services/notification_service/
  ├── main.py (FastAPI service)
  ├── notifier.py (Signal client wrapper)
  ├── config.py (Configuration management)
  ├── filters.py (Priority, quiet hours, rate limiting)
  ├── formatters.py (Message templates)
  ├── Dockerfile
  └── requirements.txt
```

### **Data Flow:**

```
Trading Event
    ↓
Redis Pub/Sub (notification:events)
    ↓
Notification Service
    ↓
Apply Filters (priority, quiet hours, rate limit)
    ↓
Format Message (template + emoji)
    ↓
Signal API
    ↓
Your Phone 📱
```

---

## 📋 **Implementation Plan**

### **Phase 1: Signal API Setup (30 min)**

#### **Step 1.1: Install signal-cli**

Signal requires `signal-cli` to send messages from command line/API.

**Docker approach (recommended):**

```dockerfile
# File: services/notification_service/Dockerfile
FROM python:3.11-slim

# Install signal-cli dependencies
RUN apt-get update && apt-get install -y \
    openjdk-17-jre-headless \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Install signal-cli
ENV SIGNAL_CLI_VERSION=0.12.8
RUN wget https://github.com/AsamK/signal-cli/releases/download/v${SIGNAL_CLI_VERSION}/signal-cli-${SIGNAL_CLI_VERSION}.tar.gz \
    && tar xf signal-cli-${SIGNAL_CLI_VERSION}.tar.gz -C /opt \
    && ln -sf /opt/signal-cli-${SIGNAL_CLI_VERSION}/bin/signal-cli /usr/local/bin/signal-cli \
    && rm signal-cli-${SIGNAL_CLI_VERSION}.tar.gz

WORKDIR /app

# Install Python dependencies
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY . .

CMD ["python", "main.py"]
```

---

#### **Step 1.2: Register Signal Account**

**You need a phone number for Signal registration** (can use Google Voice, burner number, etc.)

```bash
# Inside container (or locally):
signal-cli -a +1234567890 register

# You'll receive SMS verification code
signal-cli -a +1234567890 verify CODE_FROM_SMS

# Link to your main Signal account (optional):
signal-cli -a +1234567890 link

# Test send:
signal-cli -a +1234567890 send -m "Test message" +YOUR_REAL_PHONE
```

**Store credentials:**
```bash
# Signal stores config in ~/.local/share/signal-cli/
# Mount this as volume to persist
```

---

### **Phase 2: Python Signal Client (1 hour)**

#### **File:** `services/notification_service/requirements.txt`

```txt
fastapi==0.109.0
uvicorn[standard]==0.27.0
pydantic==2.5.0
pydantic-settings==2.1.0
redis==5.0.1
python-dotenv==1.0.0
```

---

#### **File:** `services/notification_service/config.py`

```python
from pydantic_settings import BaseSettings
from typing import List, Optional
from datetime import time

class NotificationConfig(BaseSettings):
    # Signal settings
    signal_phone: str  # Bot's phone number (e.g., "+1234567890")
    signal_recipients: List[str]  # Your phone number(s) to receive alerts
    
    # Quiet hours (no notifications during these times)
    quiet_hours_enabled: bool = True
    quiet_hours_start: time = time(22, 0)  # 10 PM
    quiet_hours_end: time = time(7, 0)     # 7 AM
    quiet_hours_timezone: str = "America/New_York"
    
    # Priority levels (what gets through quiet hours)
    quiet_hours_min_priority: str = "CRITICAL"  # Only CRITICAL during quiet hours
    
    # Rate limiting (prevent spam)
    rate_limit_enabled: bool = True
    rate_limit_window_seconds: int = 60
    rate_limit_max_messages: int = 10
    
    # Message batching (combine multiple alerts)
    batching_enabled: bool = True
    batching_window_seconds: int = 5
    
    # Redis connection
    redis_url: str = "redis://redis:6379"
    
    class Config:
        env_file = ".env"
        env_prefix = "NOTIFICATION_"

config = NotificationConfig()
```

---

#### **File:** `services/notification_service/notifier.py`

```python
import subprocess
import json
import logging
from typing import Optional, List

logger = logging.getLogger(__name__)

class SignalNotifier:
    """Wrapper for signal-cli command line tool"""
    
    def __init__(self, phone_number: str):
        self.phone_number = phone_number
        
    def send_message(
        self,
        recipient: str,
        message: str,
        attachments: Optional[List[str]] = None
    ) -> bool:
        """
        Send a Signal message
        
        Args:
            recipient: Phone number to send to (e.g., "+1234567890")
            message: Message text (supports markdown-style formatting)
            attachments: Optional list of file paths to attach
            
        Returns:
            True if sent successfully, False otherwise
        """
        try:
            cmd = [
                "signal-cli",
                "-a", self.phone_number,
                "send",
                "-m", message,
                recipient
            ]
            
            # Add attachments if provided
            if attachments:
                for attachment in attachments:
                    cmd.extend(["-a", attachment])
            
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=10
            )
            
            if result.returncode == 0:
                logger.info(f"✅ Signal sent to {recipient}")
                return True
            else:
                logger.error(f"❌ Signal failed: {result.stderr}")
                return False
                
        except subprocess.TimeoutExpired:
            logger.error("⏱️  Signal send timeout")
            return False
        except Exception as e:
            logger.error(f"💥 Signal error: {e}")
            return False
    
    def send_to_multiple(
        self,
        recipients: List[str],
        message: str,
        attachments: Optional[List[str]] = None
    ) -> dict:
        """Send to multiple recipients, return success status for each"""
        results = {}
        for recipient in recipients:
            results[recipient] = self.send_message(recipient, message, attachments)
        return results
```

---

#### **File:** `services/notification_service/formatters.py`

```python
from datetime import datetime
from typing import Dict, Any

class MessageFormatter:
    """Format trading events into readable Signal messages"""
    
    # Emoji mapping for event types
    EMOJIS = {
        "trade_entry": "🟢",
        "trade_exit": "🔴",
        "profit": "💰",
        "loss": "📉",
        "error": "⚠️",
        "critical": "🚨",
        "info": "ℹ️",
        "risk_rejection": "🛑",
        "daily_summary": "📊",
    }
    
    @staticmethod
    def format_trade_entry(data: Dict[str, Any]) -> str:
        """Format trade entry notification"""
        emoji = MessageFormatter.EMOJIS["trade_entry"]
        
        return f"""
{emoji} **TRADE ENTRY**

**Game:** {data['game_id']} - {data['sport'].upper()}
**Team:** {data['team']}
**Side:** {data['side'].upper()} @ {data['price']:.2%}

**Position:**
• Size: ${data['size']:.2f}
• Edge: {data['edge_pct']:.1f}%
• Platform: {data['platform']}

**Market:** {data['market_id']}
**Time:** {datetime.now().strftime('%I:%M %p')}
""".strip()
    
    @staticmethod
    def format_trade_exit(data: Dict[str, Any]) -> str:
        """Format trade exit notification"""
        is_profit = data['pnl'] > 0
        emoji = MessageFormatter.EMOJIS["profit"] if is_profit else MessageFormatter.EMOJIS["loss"]
        
        return f"""
{emoji} **TRADE EXIT**

**P&L:** ${data['pnl']:.2f} ({data['pnl_pct']:+.1f}%)
**Game:** {data['game_id']} - {data['sport'].upper()}
**Team:** {data['team']}

**Details:**
• Entry: {data['entry_price']:.2%} @ ${data['entry_size']:.2f}
• Exit: {data['exit_price']:.2%}
• Duration: {data['duration_minutes']} min
• Platform: {data['platform']}

**Time:** {datetime.now().strftime('%I:%M %p')}
""".strip()
    
    @staticmethod
    def format_risk_rejection(data: Dict[str, Any]) -> str:
        """Format risk rejection notification"""
        emoji = MessageFormatter.EMOJIS["risk_rejection"]
        
        return f"""
{emoji} **RISK REJECTION**

**Reason:** {data['rejection_reason']}

**Signal:**
• Game: {data['game_id']}
• Team: {data['team']}
• Edge: {data['edge_pct']:.1f}%
• Size: ${data['size']:.2f}

**Current Exposure:**
• Game: ${data.get('game_exposure', 0):.2f}
• Sport: ${data.get('sport_exposure', 0):.2f}

**Time:** {datetime.now().strftime('%I:%M %p')}
""".strip()
    
    @staticmethod
    def format_error(data: Dict[str, Any]) -> str:
        """Format error notification"""
        emoji = MessageFormatter.EMOJIS["error"]
        severity = data.get('severity', 'ERROR')
        
        if severity == 'CRITICAL':
            emoji = MessageFormatter.EMOJIS["critical"]
        
        return f"""
{emoji} **{severity}**

**Service:** {data['service']}
**Error:** {data['message']}

**Details:**
{data.get('details', 'No additional details')}

**Time:** {datetime.now().strftime('%I:%M %p')}
""".strip()
    
    @staticmethod
    def format_daily_summary(data: Dict[str, Any]) -> str:
        """Format daily summary notification"""
        emoji = MessageFormatter.EMOJIS["daily_summary"]
        
        total_pnl = data['total_pnl']
        pnl_emoji = "💰" if total_pnl > 0 else "📉"
        
        return f"""
{emoji} **DAILY SUMMARY**

**P&L:** {pnl_emoji} ${total_pnl:.2f}

**Trades:**
• Total: {data['total_trades']}
• Wins: {data['winning_trades']} ({data['win_rate']:.1f}%)
• Losses: {data['losing_trades']}

**Performance:**
• Avg Win: ${data['avg_win']:.2f}
• Avg Loss: ${data['avg_loss']:.2f}
• Profit Factor: {data['profit_factor']:.1f}x

**Exposure:**
• Max: ${data['max_exposure']:.2f}
• Current: ${data['current_exposure']:.2f}

**Date:** {datetime.now().strftime('%Y-%m-%d')}
""".strip()
    
    @staticmethod
    def format_message(event_type: str, data: Dict[str, Any]) -> str:
        """Route to appropriate formatter"""
        formatters = {
            "trade_entry": MessageFormatter.format_trade_entry,
            "trade_exit": MessageFormatter.format_trade_exit,
            "risk_rejection": MessageFormatter.format_risk_rejection,
            "error": MessageFormatter.format_error,
            "daily_summary": MessageFormatter.format_daily_summary,
        }
        
        formatter = formatters.get(event_type)
        if formatter:
            return formatter(data)
        else:
            # Fallback for unknown event types
            return f"ℹ️ **{event_type.upper()}**\n\n{json.dumps(data, indent=2)}"
```

---

#### **File:** `services/notification_service/filters.py`

```python
from datetime import datetime, time
from typing import Optional
import pytz
from collections import defaultdict
import time as time_module

class NotificationFilter:
    """Filter notifications based on priority, quiet hours, and rate limits"""
    
    PRIORITY_LEVELS = {
        "INFO": 0,
        "WARNING": 1,
        "ERROR": 2,
        "CRITICAL": 3,
    }
    
    def __init__(self, config):
        self.config = config
        self.message_timestamps = defaultdict(list)  # For rate limiting
        
    def should_notify(self, priority: str, event_type: str) -> tuple[bool, Optional[str]]:
        """
        Determine if notification should be sent
        
        Returns:
            (should_send, reason_if_filtered)
        """
        # Check quiet hours
        if self.config.quiet_hours_enabled:
            if self._is_quiet_hours():
                min_priority = self.PRIORITY_LEVELS.get(
                    self.config.quiet_hours_min_priority,
                    3  # Default to CRITICAL
                )
                current_priority = self.PRIORITY_LEVELS.get(priority, 0)
                
                if current_priority < min_priority:
                    return False, f"Quiet hours (priority {priority} < {self.config.quiet_hours_min_priority})"
        
        # Check rate limit
        if self.config.rate_limit_enabled:
            if not self._check_rate_limit():
                return False, "Rate limit exceeded"
        
        return True, None
    
    def _is_quiet_hours(self) -> bool:
        """Check if current time is within quiet hours"""
        tz = pytz.timezone(self.config.quiet_hours_timezone)
        now = datetime.now(tz).time()
        
        start = self.config.quiet_hours_start
        end = self.config.quiet_hours_end
        
        # Handle overnight quiet hours (e.g., 10 PM - 7 AM)
        if start > end:
            return now >= start or now < end
        else:
            return start <= now < end
    
    def _check_rate_limit(self) -> bool:
        """Check if we're within rate limits"""
        now = time_module.time()
        window = self.config.rate_limit_window_seconds
        max_messages = self.config.rate_limit_max_messages
        
        # Clean old timestamps
        self.message_timestamps["default"] = [
            ts for ts in self.message_timestamps["default"]
            if now - ts < window
        ]
        
        # Check if under limit
        if len(self.message_timestamps["default"]) >= max_messages:
            return False
        
        # Add current timestamp
        self.message_timestamps["default"].append(now)
        return True
```

---

#### **File:** `services/notification_service/main.py`

```python
import asyncio
import json
import logging
from typing import Dict, Any
import redis.asyncio as redis
from notifier import SignalNotifier
from config import config
from filters import NotificationFilter
from formatters import MessageFormatter

logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

class NotificationService:
    def __init__(self):
        self.config = config
        self.notifier = SignalNotifier(config.signal_phone)
        self.filter = NotificationFilter(config)
        self.redis_client = None
        
    async def start(self):
        """Start the notification service"""
        logger.info("🔔 Starting Notification Service...")
        
        # Connect to Redis
        self.redis_client = await redis.from_url(
            self.config.redis_url,
            decode_responses=True
        )
        
        # Subscribe to notification events
        pubsub = self.redis_client.pubsub()
        await pubsub.subscribe("notification:events")
        
        logger.info("✅ Subscribed to notification:events")
        logger.info(f"📱 Recipients: {self.config.signal_recipients}")
        
        # Listen for events
        async for message in pubsub.listen():
            if message['type'] == 'message':
                await self.handle_event(message['data'])
    
    async def handle_event(self, data: str):
        """Handle incoming notification event"""
        try:
            event = json.loads(data)
            
            event_type = event.get('type')
            priority = event.get('priority', 'INFO')
            payload = event.get('data', {})
            
            logger.info(f"📨 Event: {event_type} (priority: {priority})")
            
            # Apply filters
            should_send, filter_reason = self.filter.should_notify(priority, event_type)
            
            if not should_send:
                logger.info(f"🔇 Filtered: {filter_reason}")
                return
            
            # Format message
            message = MessageFormatter.format_message(event_type, payload)
            
            # Send to all recipients
            for recipient in self.config.signal_recipients:
                success = self.notifier.send_message(recipient, message)
                
                if success:
                    logger.info(f"✅ Sent to {recipient}")
                else:
                    logger.error(f"❌ Failed to send to {recipient}")
                    
        except json.JSONDecodeError:
            logger.error(f"Invalid JSON: {data}")
        except Exception as e:
            logger.error(f"Error handling event: {e}", exc_info=True)

async def main():
    service = NotificationService()
    await service.start()

if __name__ == "__main__":
    asyncio.run(main())
```

---

### **Phase 3: Integration with Trading System (30 min)**

#### **Update Trading Services to Publish Events**

**File:** `services/signal_processor_rust/src/main.rs` (example)

```rust
// After successful trade execution
async fn publish_trade_entry_notification(
    &self,
    signal: &TradingSignal,
    execution: &ExecutionResult,
) -> Result<()> {
    let event = serde_json::json!({
        "type": "trade_entry",
        "priority": "INFO",
        "data": {
            "game_id": signal.game_id,
            "sport": signal.sport,
            "team": signal.team,
            "side": execution.side,
            "price": execution.avg_price,
            "size": execution.filled_qty,
            "edge_pct": signal.edge_pct,
            "platform": execution.platform,
            "market_id": execution.market_id,
        }
    });
    
    self.redis
        .publish("notification:events", event.to_string())
        .await?;
    
    Ok(())
}

// After risk rejection
async fn publish_risk_rejection_notification(
    &self,
    signal: &TradingSignal,
    rejection_reason: &str,
    exposure_data: ExposureData,
) -> Result<()> {
    let event = serde_json::json!({
        "type": "risk_rejection",
        "priority": "WARNING",
        "data": {
            "game_id": signal.game_id,
            "team": signal.team,
            "edge_pct": signal.edge_pct,
            "size": signal.size,
            "rejection_reason": rejection_reason,
            "game_exposure": exposure_data.game,
            "sport_exposure": exposure_data.sport,
        }
    });
    
    self.redis
        .publish("notification:events", event.to_string())
        .await?;
    
    Ok(())
}
```

---

### **Phase 4: Configuration (15 min)**

#### **File:** `docker-compose.yml`

```yaml
  notification_service:
    build:
      context: .
      dockerfile: services/notification_service/Dockerfile
    container_name: arbees-notifications
    depends_on:
      redis:
        condition: service_healthy
    env_file:
      - .env
    environment:
      NOTIFICATION_SIGNAL_PHONE: "${SIGNAL_PHONE:?SIGNAL_PHONE required}"
      NOTIFICATION_SIGNAL_RECIPIENTS: "${SIGNAL_RECIPIENTS:?SIGNAL_RECIPIENTS required}"
      NOTIFICATION_QUIET_HOURS_ENABLED: "true"
      NOTIFICATION_QUIET_HOURS_START: "22:00"
      NOTIFICATION_QUIET_HOURS_END: "07:00"
      NOTIFICATION_QUIET_HOURS_TIMEZONE: "America/New_York"
      NOTIFICATION_QUIET_HOURS_MIN_PRIORITY: "CRITICAL"
      NOTIFICATION_RATE_LIMIT_ENABLED: "true"
      NOTIFICATION_RATE_LIMIT_WINDOW_SECONDS: "60"
      NOTIFICATION_RATE_LIMIT_MAX_MESSAGES: "10"
      REDIS_URL: "redis://redis:6379"
    volumes:
      - signal_data:/root/.local/share/signal-cli
    restart: unless-stopped
    profiles:
      - full

volumes:
  signal_data:  # Persist Signal registration
```

---

#### **File:** `.env`

```bash
# Signal Configuration
SIGNAL_PHONE=+1234567890  # Bot's phone number (register with signal-cli)
SIGNAL_RECIPIENTS=+10987654321,+11234567890  # Comma-separated recipient numbers

# Notification Settings
NOTIFICATION_QUIET_HOURS_ENABLED=true
NOTIFICATION_QUIET_HOURS_START=22:00
NOTIFICATION_QUIET_HOURS_END=07:00
NOTIFICATION_QUIET_HOURS_TIMEZONE=America/New_York
NOTIFICATION_QUIET_HOURS_MIN_PRIORITY=CRITICAL  # Only CRITICAL during quiet hours
NOTIFICATION_RATE_LIMIT_ENABLED=true
NOTIFICATION_RATE_LIMIT_WINDOW_SECONDS=60
NOTIFICATION_RATE_LIMIT_MAX_MESSAGES=10
```

---

### **Phase 5: Testing & Validation (30 min)**

#### **Test 1: Send Test Message**

```bash
# Inside container
docker exec arbees-notifications python -c "
from notifier import SignalNotifier
from config import config

notifier = SignalNotifier(config.signal_phone)
notifier.send_message(
    config.signal_recipients[0],
    '🧪 Test notification from Arbees!'
)
"
```

---

#### **Test 2: Publish Test Event**

```bash
# Publish test trade entry
docker exec arbees-redis redis-cli PUBLISH notification:events '{
  "type": "trade_entry",
  "priority": "INFO",
  "data": {
    "game_id": "401810502",
    "sport": "nba",
    "team": "Lakers",
    "side": "buy",
    "price": 0.55,
    "size": 25.50,
    "edge_pct": 8.5,
    "platform": "Polymarket",
    "market_id": "token_123"
  }
}'
```

**Check your phone - you should receive a formatted message!**

---

#### **Test 3: Test Quiet Hours**

```bash
# Set quiet hours to NOW
docker exec arbees-notifications python -c "
from filters import NotificationFilter
from config import config
from datetime import datetime

# Override quiet hours to current time
config.quiet_hours_start = datetime.now().time()
config.quiet_hours_end = datetime.now().time()

filter = NotificationFilter(config)
should_send, reason = filter.should_notify('INFO', 'test')
print(f'Should send: {should_send}, Reason: {reason}')
"
```

---

## 🎛️ **Configuration Options**

### **Quiet Hours:**

```bash
# Disable notifications 10 PM - 7 AM
NOTIFICATION_QUIET_HOURS_ENABLED=true
NOTIFICATION_QUIET_HOURS_START=22:00
NOTIFICATION_QUIET_HOURS_END=07:00

# Still get CRITICAL alerts during quiet hours
NOTIFICATION_QUIET_HOURS_MIN_PRIORITY=CRITICAL

# Set timezone
NOTIFICATION_QUIET_HOURS_TIMEZONE=America/New_York
```

---

### **Rate Limiting:**

```bash
# Max 10 messages per minute
NOTIFICATION_RATE_LIMIT_ENABLED=true
NOTIFICATION_RATE_LIMIT_WINDOW_SECONDS=60
NOTIFICATION_RATE_LIMIT_MAX_MESSAGES=10
```

---

### **Priority Levels:**

```python
# In your trading services, set priority:
"priority": "INFO"      # Regular trades
"priority": "WARNING"   # Risk rejections
"priority": "ERROR"     # Execution failures
"priority": "CRITICAL"  # System crashes, circuit breakers
```

---

## 📱 **Example Notifications**

### **Trade Entry:**
```
🟢 TRADE ENTRY

Game: 401810502 - NBA
Team: Lakers
Side: BUY @ 55.00%

Position:
• Size: $25.50
• Edge: 8.5%
• Platform: Polymarket

Market: token_abc123
Time: 02:15 PM
```

---

### **Trade Exit:**
```
💰 TRADE EXIT

P&L: $2.50 (+9.8%)
Game: 401810502 - NBA
Team: Lakers

Details:
• Entry: 55.00% @ $25.50
• Exit: 65.00%
• Duration: 12 min
• Platform: Polymarket

Time: 02:27 PM
```

---

### **Risk Rejection:**
```
🛑 RISK REJECTION

Reason: MAX_GAME_EXPOSURE: $45.00 + $30.00 > $50.00

Signal:
• Game: 401810503
• Team: Celtics
• Edge: 7.2%
• Size: $30.00

Current Exposure:
• Game: $45.00
• Sport: $180.00

Time: 02:30 PM
```

---

## 🚀 **Deployment**

### **Step 1: Register Signal Bot**

```bash
# Start notification service
docker-compose up -d notification_service

# Register Signal number
docker exec -it arbees-notifications signal-cli -a +1234567890 register

# Verify with code from SMS
docker exec -it arbees-notifications signal-cli -a +1234567890 verify CODE

# Test send
docker exec -it arbees-notifications signal-cli -a +1234567890 send \
  -m "Registration successful!" +YOUR_PHONE
```

---

### **Step 2: Enable in Trading Services**

Add notification publishing to:
- ✅ signal_processor (trade entries, risk rejections)
- ✅ position_tracker (trade exits)
- ✅ execution_service (errors)
- ✅ orchestrator (critical errors)

---

### **Step 3: Monitor**

```bash
# Check notification service logs
docker-compose logs -f notification_service

# Should see:
# ✅ Subscribed to notification:events
# 📱 Recipients: ['+1234567890']
# 📨 Event: trade_entry (priority: INFO)
# ✅ Sent to +1234567890
```

---

## 🎯 **Advanced Features (Optional)**

### **Feature 1: Daily Summary (Auto-sent at 11 PM)**

Add to `notification_service`:

```python
async def send_daily_summary(self):
    """Send daily summary at configured time"""
    # Query database for today's stats
    summary = await self.get_daily_stats()
    
    event = {
        "type": "daily_summary",
        "priority": "INFO",
        "data": summary
    }
    
    await self.handle_event(json.dumps(event))

# Schedule daily at 11 PM
async def schedule_daily_summary(self):
    while True:
        now = datetime.now()
        target = now.replace(hour=23, minute=0, second=0)
        
        if now > target:
            target += timedelta(days=1)
        
        wait_seconds = (target - now).total_seconds()
        await asyncio.sleep(wait_seconds)
        
        await self.send_daily_summary()
```

---

### **Feature 2: Multiple Recipients with Different Rules**

```python
# In config.py
signal_recipients: List[Dict[str, Any]] = [
    {
        "phone": "+1234567890",
        "name": "Pete",
        "min_priority": "INFO",
        "quiet_hours": True
    },
    {
        "phone": "+10987654321",
        "name": "Partner",
        "min_priority": "CRITICAL",  # Only critical alerts
        "quiet_hours": False  # Always notify
    }
]
```

---

### **Feature 3: Image Attachments (Charts)**

```python
# Generate P&L chart and send
import matplotlib.pyplot as plt

def generate_pnl_chart(trades):
    plt.figure(figsize=(10, 6))
    plt.plot(trades['timestamp'], trades['cumulative_pnl'])
    plt.title('Cumulative P&L')
    plt.savefig('/tmp/pnl_chart.png')
    return '/tmp/pnl_chart.png'

# Send with attachment
chart_path = generate_pnl_chart(trades)
notifier.send_message(
    recipient,
    "📊 Daily P&L Chart",
    attachments=[chart_path]
)
```

---

## ✅ **Success Checklist**

- [ ] Signal bot registered and verified
- [ ] Test message received on phone
- [ ] Quiet hours working (no INFO during sleep)
- [ ] Rate limiting working (max 10/min)
- [ ] Trade entry notifications formatted correctly
- [ ] Trade exit notifications show P&L
- [ ] Risk rejection notifications clear
- [ ] Multiple recipients working
- [ ] Volume persisted (Signal config survives restart)

---

## 💡 **Tips & Best Practices**

### **Keep Messages Concise:**
- Mobile screens are small
- Most important info first
- Use emojis for quick visual scanning

### **Don't Over-Notify:**
- Use rate limiting
- Batch similar events
- Respect quiet hours

### **Test Thoroughly:**
- Verify quiet hours in your timezone
- Test with real trading events
- Make sure critical alerts always get through

### **Security:**
- Never include sensitive data (API keys, passwords)
- Use separate Signal number for bot
- Keep private key secure

---

## 📊 **Expected Results**

### **Notification Frequency:**

**With Risk Controls:**
```
Trade entries: ~5-10/day
Trade exits: ~5-10/day
Risk rejections: ~2-5/day
Errors: ~0-2/day
Daily summary: 1/day

Total: ~15-30 notifications/day
```

**During Quiet Hours (10 PM - 7 AM):**
```
Only CRITICAL alerts (system crashes, circuit breakers)
Expected: 0-1/day
```

---

## 🎯 **Final Recommendation**

1. ✅ **Use Signal** (secure, free, works great)
2. ✅ **Configure quiet hours** (10 PM - 7 AM)
3. ✅ **Enable rate limiting** (max 10/min)
4. ✅ **Set priorities correctly** (CRITICAL for emergencies only)
5. ✅ **Test before enabling** (verify quiet hours work)

---

**Get instant alerts on your phone for every trade, profit, and issue - while you sleep peacefully!** 📱💤✅
