# Getting Signal CAPTCHA Token - Mobile vs Desktop

## 🚨 **Problem: CAPTCHA Opens Signal App Instead of Showing Token**

This happens when you open https://signalcaptchas.org/registration/generate.html on **mobile**.

---

## ✅ **Solution 1: Use Desktop Browser (RECOMMENDED)**

The CAPTCHA page works best on desktop.

### **Steps:**

1. **Open on desktop computer:** https://signalcaptchas.org/registration/generate.html
2. **Complete the CAPTCHA** (check "I'm not a robot")
3. **Copy the token** that appears (long string starting with `signal-recaptcha-v2...`)
4. **Use token in Docker command** on your server

**This is the easiest way!**

---

## ✅ **Solution 2: Use Mobile Browser in Desktop Mode**

If you must use mobile:

### **On iPhone (Safari):**

1. Open Safari
2. Go to https://signalcaptchas.org/registration/generate.html
3. Tap **AA** button in address bar
4. Select **"Request Desktop Website"**
5. Complete CAPTCHA
6. Token should appear (long string)
7. **Long-press to select** → Copy

### **On Android (Chrome):**

1. Open Chrome
2. Go to https://signalcaptchas.org/registration/generate.html
3. Tap **⋮** (three dots menu)
4. Check **"Desktop site"**
5. Complete CAPTCHA
6. Token should appear
7. **Long-press to select** → Copy

---

## ✅ **Solution 3: Alternative CAPTCHA Method**

Signal has an alternative registration method that **doesn't require CAPTCHA** (but requires voice call):

### **Register with Voice Call:**

```bash
# Skip CAPTCHA entirely - use voice verification
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 register --voice

# You'll receive a PHONE CALL with the verification code
# (Robot voice will speak the 6-digit code)

# Then verify:
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 verify 123456
```

**This works great and avoids the CAPTCHA issue entirely!**

---

## ✅ **Solution 4: Get Token from Desktop, Text to Yourself**

If you have access to a desktop computer but need to run commands from mobile:

1. **On desktop:** Get CAPTCHA token from https://signalcaptchas.org/registration/generate.html
2. **Copy token** (will be ~200 characters long)
3. **Email or text yourself** the token
4. **Use token on mobile/server**

---

## 🎯 **What the Token Looks Like**

The CAPTCHA token is a **very long string** (200+ characters) that looks like:

```
signal-recaptcha-v2.03.AHdjaX9234lkj23lkjsdfkljsdf...
(continues for ~200 characters)
```

**Common mistakes:**
- ❌ Copying only part of the token (must be complete!)
- ❌ Adding quotes around token (don't do this in command)
- ❌ Line breaks in token (should be one continuous string)

---

## 📱 **Why Mobile Opens Signal App**

The CAPTCHA page has a **"signalcaptcha://"** link that:
- On **desktop:** Shows token as text
- On **mobile:** Tries to open Signal app

This is intentional (for regular Signal users), but annoying for developers!

---

## 🔧 **Complete Registration Flow (Voice Method - EASIEST)**

**Recommended if CAPTCHA is problematic:**

```bash
# 1. Start signal-cli
docker-compose up -d signal-cli-rest-api

# 2. Register with VOICE (no CAPTCHA needed!)
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +YOUR_BOT_NUMBER register --voice

# 3. Answer the phone call
# Robot will say: "Your Signal verification code is 1-2-3-4-5-6"

# 4. Verify with the code
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +YOUR_BOT_NUMBER verify 123456

# 5. Test send
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +YOUR_BOT_NUMBER send \
  -m "Registration successful!" \
  +YOUR_PERSONAL_NUMBER

# Done! ✅
```

**This is actually EASIER than the CAPTCHA method!**

---

## 🆚 **CAPTCHA vs Voice Call**

| Method | Pros | Cons |
|--------|------|------|
| **CAPTCHA** | No phone call needed | Mobile browser issues |
| **Voice Call** | Works from anywhere | Need to answer phone |

**For developers:** Voice call is often easier!

---

## 🐛 **Troubleshooting**

### **"I don't see any token after CAPTCHA"**

**Cause:** Mobile browser opening Signal app

**Solution:**
1. Use desktop browser, OR
2. Use "Desktop mode" in mobile browser, OR
3. Use voice call method instead

---

### **"Token is too long to copy on mobile"**

**Cause:** Token is ~200 characters

**Solution:**
1. **Triple-tap** the token text to select all
2. Or use desktop browser
3. Or use voice call method

---

### **"Voice call didn't arrive"**

**Possible causes:**
1. Wrong number format (must include country code: `+1234567890`)
2. Number already registered
3. Carrier blocking automated calls

**Solutions:**
```bash
# Try SMS method instead:
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 register --captcha "TOKEN"

# Or use different phone number (Google Voice, etc.)
```

---

### **"CAPTCHA token expired"**

**Cause:** Tokens expire after ~10 minutes

**Solution:** Get a fresh token from https://signalcaptchas.org/registration/generate.html

---

## 📋 **Quick Decision Tree**

```
Do you have access to desktop computer?
├─ YES → Use desktop browser for CAPTCHA ✅
└─ NO → Use voice call method ✅

Did voice call work?
├─ YES → Great, you're done! ✅
└─ NO → Use desktop mode in mobile browser for CAPTCHA
```

---

## 🎯 **Recommended Approach**

**Best method for most people:**

```bash
# Just use voice call - it's easiest!
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 register --voice

# Answer phone, get code, verify:
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 verify CODE

# Done!
```

**No CAPTCHA hassles, works from anywhere!** ✅

---

## 💡 **Pro Tip**

If you're setting up signal-cli on a server (SSH), you can:

1. **Get CAPTCHA token on your desktop**
2. **SSH into server**
3. **Paste token in register command**

```bash
# On your server via SSH:
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 register \
  --captcha "signal-recaptcha-v2.03.ABC123..."
```

---

## ✅ **Summary**

**You have 3 options:**

1. ⭐ **Voice call** (easiest, no CAPTCHA)
   ```bash
   register --voice
   ```

2. 🖥️ **Desktop browser** (if you have desktop)
   - Open CAPTCHA page on desktop
   - Get token
   - Use in command

3. 📱 **Mobile desktop mode** (if stuck on mobile)
   - Request desktop site
   - Get token
   - Use in command

**Recommended: Just use voice call - it's simpler!** 🎉
