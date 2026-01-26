# Signal CAPTCHA - Desktop App Interference Fix

## 🚨 **Problem: Signal Desktop App Intercepts CAPTCHA**

When you have Signal Desktop installed, clicking the CAPTCHA link opens Signal app instead of showing the token.

---

## ✅ **Solution 1: Use Voice Call (EASIEST)**

**Skip CAPTCHA entirely:**

```bash
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 register --voice

# Answer the phone call, get the 6-digit code
# Then verify:
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 verify 123456
```

**This completely avoids the CAPTCHA issue!** ⭐

---

## ✅ **Solution 2: Get Token Without Clicking**

The CAPTCHA token is **in the page HTML** - you don't need to click!

### **Method A: Inspect Element**

1. Open https://signalcaptchas.org/registration/generate.html
2. Complete the CAPTCHA (check "I'm not a robot")
3. **DON'T CLICK** the "Open Signal" button
4. **Right-click on the button** → Inspect Element
5. Look for the `href` attribute containing `signalcaptcha://`
6. The token is after `signalcaptcha://` in the URL

**Example:**
```html
<a href="signalcaptcha://signal-recaptcha-v2.03.AHdjaX9234...">
```

The token is: `signal-recaptcha-v2.03.AHdjaX9234...`

---

### **Method B: View Page Source**

1. Open https://signalcaptchas.org/registration/generate.html
2. Complete CAPTCHA
3. **Right-click anywhere** → View Page Source (or press Ctrl+U)
4. **Search for** `signalcaptcha://` (Ctrl+F)
5. Copy everything after `signalcaptcha://` up to the quote

**It will look like:**
```html
href="signalcaptcha://signal-recaptcha-v2.03.ABC123XYZ789..."
                     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                     This is your token!
```

---

### **Method C: Browser Console**

1. Open https://signalcaptchas.org/registration/generate.html
2. Complete CAPTCHA
3. **Open browser console** (F12 or right-click → Inspect)
4. Go to **Console** tab
5. Type this JavaScript:
   ```javascript
   document.querySelector('a[href^="signalcaptcha://"]').href.replace('signalcaptcha://', '')
   ```
6. Press Enter
7. **Copy the output** (that's your token!)

---

## ✅ **Solution 3: Temporarily Uninstall Signal Desktop**

If you really need the CAPTCHA method:

### **Windows:**
1. Uninstall Signal Desktop (Settings → Apps)
2. Get CAPTCHA token from https://signalcaptchas.org/registration/generate.html
3. Reinstall Signal Desktop

### **macOS:**
1. Quit Signal Desktop
2. Move Signal.app to Trash
3. Get CAPTCHA token
4. Restore Signal.app from Trash

### **Linux:**
```bash
# Temporarily remove Signal Desktop
sudo apt remove signal-desktop  # or snap remove signal-desktop

# Get CAPTCHA token
# Then reinstall:
sudo apt install signal-desktop
```

**Not recommended** - voice call is easier!

---

## ✅ **Solution 4: Use Different Browser**

If Signal Desktop is associated with your default browser, use a different one:

1. **Copy the URL:** https://signalcaptchas.org/registration/generate.html
2. **Open in different browser** (if you normally use Chrome, try Firefox)
3. Complete CAPTCHA
4. Get token

**Signal Desktop usually only intercepts in one browser.**

---

## ✅ **Solution 5: Incognito/Private Mode**

Sometimes private browsing prevents URL scheme interception:

1. **Open Incognito/Private window** (Ctrl+Shift+N in Chrome)
2. Go to https://signalcaptchas.org/registration/generate.html
3. Complete CAPTCHA
4. Try to get token

**May or may not work** depending on OS configuration.

---

## 🎯 **Recommended Solutions (In Order)**

### **1. Voice Call** ⭐ **BEST**
```bash
register --voice
```
- No CAPTCHA needed
- No browser issues
- Works every time

---

### **2. Inspect Element** ⭐⭐ **GOOD**
1. Complete CAPTCHA
2. Right-click button → Inspect
3. Copy token from `href` attribute

---

### **3. Browser Console** ⭐⭐ **GOOD**
```javascript
document.querySelector('a[href^="signalcaptcha://"]').href.replace('signalcaptcha://', '')
```

---

### **4. Different Browser**
- Try browser that Signal Desktop doesn't intercept

---

### **5. Uninstall Signal Desktop** (Last Resort)
- Only if you really need CAPTCHA method

---

## 📋 **Step-by-Step: Inspect Element Method**

**Detailed walkthrough:**

1. Open https://signalcaptchas.org/registration/generate.html

2. Complete the reCAPTCHA checkbox

3. After CAPTCHA completes, you'll see "Open Signal" button

4. **DON'T CLICK IT!** Instead:
   - Right-click the button
   - Select "Inspect" or "Inspect Element"

5. In the DevTools, you'll see HTML like:
   ```html
   <a href="signalcaptcha://signal-recaptcha-v2.03.AHdjaX..." 
      class="btn btn-primary">
     Open Signal
   </a>
   ```

6. **Click on the `href` value** in DevTools

7. It will highlight the entire URL

8. **Copy everything AFTER** `signalcaptcha://`

9. That's your token! Use it:
   ```bash
   docker exec -it arbees-signal-cli-rest-api \
     signal-cli -a +1234567890 register \
     --captcha "signal-recaptcha-v2.03.AHdjaX..."
   ```

---

## 🔍 **What You're Looking For**

The token is a **long string** (~200 characters) that starts with:
```
signal-recaptcha-v2.03.
```

Followed by random characters:
```
AHdjaX9lkj234lkjsdf234lkjsdf...
```

**Full token example:**
```
signal-recaptcha-v2.03.AHdjaX9lkj234lkjsdf234lkjsdfkljsdflkjsdf234lkjsdf234lkjsdflkjsdf234lkjsdf...
```

---

## ⚠️ **Common Mistakes**

### **Mistake 1: Copying "signalcaptcha://"**
```bash
# ❌ WRONG (includes protocol)
--captcha "signalcaptcha://signal-recaptcha-v2.03.ABC..."

# ✅ CORRECT (no protocol)
--captcha "signal-recaptcha-v2.03.ABC..."
```

### **Mistake 2: Adding Extra Quotes**
```bash
# ❌ WRONG (quotes inside quotes)
--captcha "'signal-recaptcha-v2.03.ABC...'"

# ✅ CORRECT (single quotes around entire token)
--captcha "signal-recaptcha-v2.03.ABC..."
```

### **Mistake 3: Incomplete Token**
```bash
# ❌ WRONG (token cut off)
--captcha "signal-recaptcha-v2.03.ABC"

# ✅ CORRECT (full ~200 character token)
--captcha "signal-recaptcha-v2.03.ABC123XYZ789...long string..."
```

---

## 🧪 **Test Your Setup**

After getting token (any method), test registration:

```bash
# 1. Register
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 register \
  --captcha "YOUR_TOKEN_HERE"

# Should output: "Verification code sent to +1234567890"

# 2. Check for SMS (wait ~30 seconds)

# 3. Verify with code from SMS
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 verify 123456

# Should output: Registration successful

# 4. Test send
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 send \
  -m "It works! 🎉" \
  +YOUR_PERSONAL_NUMBER

# Check your phone!
```

---

## 💡 **Pro Tip: Browser Console Method**

**If you're comfortable with browser console:**

```javascript
// After completing CAPTCHA, run this in console (F12):
copy(document.querySelector('a[href^="signalcaptcha://"]').href.replace('signalcaptcha://', ''))

// Token is now in your clipboard!
```

This **automatically copies** the token to clipboard!

---

## 🎯 **Bottom Line**

**You have 3 easy options:**

1. **Voice call** - No CAPTCHA needed ⭐
   ```bash
   register --voice
   ```

2. **Inspect Element** - Right-click button, copy from HTML ⭐⭐
   
3. **Browser Console** - JavaScript to extract token ⭐⭐

**All work with Signal Desktop installed!**

---

## ✅ **Recommended: Just Use Voice**

Seriously, voice call is **way easier**:

```bash
# One command:
docker exec -it arbees-signal-cli-rest-api \
  signal-cli -a +1234567890 register --voice

# Answer phone, get code, done!
```

**No CAPTCHA, no browser, no Signal Desktop issues!** 🎉

---

## 📞 **If Voice Doesn't Work**

Some carriers block automated calls. If voice call fails:

1. **Use Inspect Element method** (see above)
2. **Or use different phone number** (Google Voice, Twilio)
3. **Or temporarily close Signal Desktop**, get CAPTCHA, reopen

---

**Summary: Voice call is easiest, Inspect Element is backup!** ✅
