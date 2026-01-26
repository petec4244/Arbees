#!/usr/bin/env python3
"""Test notification script - sends properly formatted JSON to Redis"""
import redis
import json
import sys

# Connect to Redis
r = redis.Redis(host='localhost', port=6379, decode_responses=False)

# Create a properly formatted NotificationEvent
event = {
    "type": "trade_entry",
    "priority": "CRITICAL",
    "data": {
        "message": "Test notification from Python script"
    }
}

# Serialize to JSON string
json_str = json.dumps(event)

# Publish to Redis
result = r.publish("notification:events", json_str.encode('utf-8'))
print(f"Published notification (subscribers: {result})")
print(f"JSON payload: {json_str}")
