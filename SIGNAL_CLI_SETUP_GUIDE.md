# Signal CLI Setup Guide - Step by Step

**Goal:** Get signal-cli-rest-api running and registered so you can send notifications

**Time:** 15-20 minutes (one-time setup)

---

## 📋 **Prerequisites**

You need:
1. ✅ A phone number for the Signal bot (can be Google Voice, burner, or your real number)
2. ✅ Docker installed and running
3. ✅ Your personal phone number (to receive notifications)

---

## 🚀 **Step-by-Step Setup**

### **Step 1: Add signal-cli to docker-compose.yml**

**File:** `docker-compose.yml`

```yaml
services:
  # ... your existing services ...

  # Signal CLI REST API
  signal-cli:
    image: bbernhard/signal-cli-rest-api:0.80
    container_name: arbees-signal-cli
    environment:
      MODE: json-rpc  # Use JSON-RPC mode for HTTP API
    ports:
      - "8080:8080"  # Expose API on port 8080
    volumes:
      - signal_data:/home/.local/share/signal-cli  # Persist registration
    restart: unless-stopped
    profiles:
      - full
      - notifications

volumes:
  # ... your existing volumes ...
  signal_data:  # Store Signal registration data
```

---

### **Step 2: Start signal-cli Container**

```bash
# Start just signal-cli
docker-compose up -d signal-cli

# Check it's running
docker ps | grep signal-cli

# Should see:
# arbees-signal-cli ... Up ... 0.0.0.0:8080->8080/tcp
```

---

### **Step 3: Test signal-cli API is Accessible**

```bash
# Test the API
curl http://localhost:8080/v1/about

# Should return JSON like:
# {
#   "versions": {
#     "signal-cli": "0.11.x",
#     "api": "0.80"
#   },
#   ...
# }
```

**✅ If you see JSON response, signal-cli is running!**

---

### **Step 4: Register Your Signal Bot Number**

**IMPORTANT:** You need a phone number that can receive SMS. Options:
- Google Voice (free, US only)
- Burner app ($5/month)
- Twilio number ($1/month)
- Your actual phone number (works, but you'll get bot messages mixed with personal)

**Registration:**

```bash
# Replace +1234567890 with your bot's phone number
docker exec -it arbees-signal-cli \
  signal-cli -a +1234567890 register

# You should see:
# Captcha required for verification, use --captcha CAPTCHA
# To get the token, go to https://signalcaptchas.org/registration/generate.html
```

---

### **Step 5: Get CAPTCHA Token**

Signal requires a CAPTCHA to prevent spam bots.

1. **Open in browser:** https://signalcaptchas.org/registration/generate.html
2. **Complete the CAPTCHA** (check "I'm not a robot")
3. **Copy the token** (looks like: `signal-recaptcha-v2.03.Abc123XyzDef456...`)

---

### **Step 6: Register with CAPTCHA**

```bash
# Replace CAPTCHA_TOKEN with the token you copied
docker exec -it arbees-signal-cli \
  signal-cli -a +1234567890 register --captcha "CAPTCHA_TOKEN_HERE"

# You should see:
# Verification code sent to +1234567890
```

**📱 Check your phone/Google Voice - you should receive an SMS with a 6-digit code!**

---

### **Step 7: Verify with SMS Code**

```bash
# Replace 123456 with the code from SMS
docker exec -it arbees-signal-cli \
  signal-cli -a +1234567890 verify 123456

# You should see:
# Registration successful
```

**✅ Your Signal bot is now registered!**

---

### **Step 8: Test Sending a Message**

```bash
# Send test message to YOUR phone
# Replace +10987654321 with YOUR personal phone number
docker exec -it arbees-signal-cli \
  signal-cli -a +1234567890 send \
  -m "🎉 Signal bot is working!" \
  +10987654321

# Should see:
# (no error message = success)
```

**📱 Check your phone - you should receive the message on Signal!**

---

### **Step 9: Test via HTTP API**

```bash
# Test the REST API
curl -X POST http://localhost:8080/v2/send \
  -H "Content-Type: application/json" \
  -d '{
    "number": "+1234567890",
    "recipients": ["+10987654321"],
    "message": "🚀 HTTP API works too!"
  }'

# Should return:
# {"timestamp": ...}
```

**📱 Check your phone again - another message!**

---

### **Step 10: Configure .env**

**File:** `.env`

```bash
# Signal Configuration
SIGNAL_PHONE=+1234567890  # Your bot's number (the one you registered)
SIGNAL_RECIPIENTS=+10987654321  # Your personal number (comma-separated for multiple)

# Quiet Hours
QUIET_HOURS_ENABLED=true
QUIET_HOURS_START=22:00
QUIET_HOURS_END=07:00
QUIET_HOURS_TIMEZONE=America/New_York
QUIET_HOURS_MIN_PRIORITY=CRITICAL

# Rate Limiting
RATE_LIMIT_MAX_PER_MINUTE=10
RATE_LIMIT_MAX_PER_HOUR=100
RATE_LIMIT_MAX_PER_DAY=500
```

---

## ✅ **Verification Checklist**

- [ ] signal-cli container running (`docker ps`)
- [ ] API accessible (`curl http://localhost:8080/v1/about`)
- [ ] Bot number registered (no error from `register`)
- [ ] SMS code verified (no error from `verify`)
- [ ] Test message sent via CLI (`signal-cli send`)
- [ ] Test message sent via HTTP (`curl /v2/send`)
- [ ] Received both messages on your phone
- [ ] `.env` configured with both phone numbers

**If all checked, you're done!** ✅

---

## 🔧 **Troubleshooting**

### **Problem: "Captcha required for verification"**

**Solution:**
1. Go to https://signalcaptchas.org/registration/generate.html
2. Complete CAPTCHA
3. Copy token
4. Use `--captcha "token"` flag in register command

---

### **Problem: "Account is already registered"**

**Solution:**
Your number is already registered. You have two options:

**Option A: Unregister and start fresh**
```bash
docker exec -it arbees-signal-cli \
  signal-cli -a +1234567890 unregister

# Then start from Step 4 again
```

**Option B: Use existing registration**
```bash
# Just test sending a message
docker exec -it arbees-signal-cli \
  signal-cli -a +1234567890 send -m "Test" +YOUR_NUMBER
```

---

### **Problem: "User +1234567890 is not registered"**

**Cause:** Registration didn't complete or was cleared.

**Solution:** Go back to Step 4 and re-register.

---

### **Problem: No SMS received**

**Possible causes:**
1. Wrong phone number format (must include country code: `+1` for US)
2. Number already used with Signal
3. SMS delay (wait 2-3 minutes)
4. Invalid CAPTCHA token (get new one)

**Solution:**
```bash
# Try registering with voice call instead
docker exec -it arbees-signal-cli \
  signal-cli -a +1234567890 register --voice
```

---

### **Problem: "Connection refused" when curling localhost:8080**

**Cause:** signal-cli container not running or port not exposed.

**Solution:**
```bash
# Check container status
docker ps | grep signal-cli

# Check logs
docker logs arbees-signal-cli

# Restart container
docker-compose restart signal-cli
```

---

### **Problem: Message sent but not received**

**Possible causes:**
1. Wrong recipient number format
2. Recipient doesn't have Signal installed
3. Recipient blocked the bot number

**Solution:**
1. Verify number format: `+1234567890` (country code required)
2. Install Signal on recipient's phone
3. Check Signal privacy settings

---

## 🎯 **Alternative: Using Your Personal Number**

**You CAN use your personal Signal number for the bot, but:**

**Pros:**
- ✅ No need for second phone number
- ✅ Immediate setup

**Cons:**
- ❌ Bot messages mixed with personal messages
- ❌ Can't use Signal on your phone and bot simultaneously
- ❌ Might confuse friends who see "typing..." from you

**How to do it:**

```bash
# Instead of registering, link to existing account
docker exec -it arbees-signal-cli \
  signal-cli -a +YOUR_PERSONAL_NUMBER link \
  -n "Arbees Bot"

# Signal will show a QR code in terminal
# Scan it with Signal app: Settings > Linked Devices > Link New Device
```

**NOT RECOMMENDED** - better to use Google Voice or burner number.

---

## 💡 **Getting a Phone Number**

### **Option 1: Google Voice (FREE, US only)**

1. Go to https://voice.google.com
2. Sign in with Google account
3. Choose a phone number (free)
4. Use this number for Signal bot

**Pros:** Free, reliable, can receive SMS
**Cons:** US only, requires Google account

---

### **Option 2: Burner App ($5/month)**

1. Download Burner app (iOS/Android)
2. Subscribe ($5/month)
3. Get temporary number
4. Use for Signal bot

**Pros:** Works internationally, disposable
**Cons:** Costs money, number expires if you don't renew

---

### **Option 3: Twilio ($1/month)**

1. Sign up at https://twilio.com
2. Buy phone number ($1/month)
3. Use for Signal bot

**Pros:** Cheap, programmable (can forward SMS to email)
**Cons:** Requires Twilio account, some setup

---

## 📱 **Recommended Setup**

**For Development/Testing:**
```
Bot Number: Google Voice (free)
Recipient: Your personal phone
```

**For Production:**
```
Bot Number: Dedicated number (Burner or Twilio)
Recipients: Your personal phone + team members
```

---

## 🔄 **What Gets Persisted**

Signal registration data is stored in Docker volume `signal_data`.

**This includes:**
- ✅ Phone number registration
- ✅ Encryption keys
- ✅ Device credentials
- ✅ Contact list

**What this means:**
- Survives container restarts
- Survives Docker Compose down/up
- Does NOT survive `docker volume rm signal_data`

**To backup registration:**
```bash
# Backup
docker run --rm -v arbees_signal_data:/data -v $(pwd):/backup \
  ubuntu tar czf /backup/signal-backup.tar.gz /data

# Restore (if needed)
docker run --rm -v arbees_signal_data:/data -v $(pwd):/backup \
  ubuntu tar xzf /backup/signal-backup.tar.gz -C /
```

---

## ✅ **Quick Setup Summary**

```bash
# 1. Start signal-cli
docker-compose up -d signal-cli

# 2. Register bot number (with CAPTCHA)
docker exec -it arbees-signal-cli \
  signal-cli -a +BOT_NUMBER register --captcha "CAPTCHA_TOKEN"

# 3. Verify with SMS code
docker exec -it arbees-signal-cli \
  signal-cli -a +BOT_NUMBER verify SMS_CODE

# 4. Test send
docker exec -it arbees-signal-cli \
  signal-cli -a +BOT_NUMBER send -m "Test!" +YOUR_NUMBER

# 5. Test HTTP API
curl -X POST http://localhost:8080/v2/send \
  -H "Content-Type: application/json" \
  -d '{
    "number": "+BOT_NUMBER",
    "recipients": ["+YOUR_NUMBER"],
    "message": "HTTP test!"
  }'

# 6. Configure .env
SIGNAL_PHONE=+BOT_NUMBER
SIGNAL_RECIPIENTS=+YOUR_NUMBER

# Done! ✅
```

---

## 🎯 **Next Steps**

After signal-cli is set up:

1. ✅ Build notification_service_rust
2. ✅ Start notification_service
3. ✅ Publish test event to Redis
4. ✅ Receive notification on phone!

**See `PLANNING_PROMPT_NOTIFICATION_SERVICE_RUST.md` for Rust service implementation.**

---

## 📞 **Support**

If you get stuck:

1. **Check logs:**
   ```bash
   docker logs arbees-signal-cli
   ```

2. **Verify registration:**
   ```bash
   docker exec arbees-signal-cli \
     signal-cli -a +BOT_NUMBER listAccounts
   ```

3. **Test API:**
   ```bash
   curl http://localhost:8080/v1/about
   curl http://localhost:8080/v1/accounts
   ```

4. **signal-cli-rest-api docs:**
   - GitHub: https://github.com/bbernhard/signal-cli-rest-api
   - Issues: Check if your problem is reported

---

**That's it! Signal CLI is ready to send notifications!** 📱✨
