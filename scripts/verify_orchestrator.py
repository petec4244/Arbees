import redis
import time
import json
import threading
import sys
import os

# Configuration
REDIS_URL = os.environ.get("REDIS_URL", "redis://localhost:6379")
SHARD_ID = "test-shard-rust-verification"
HEARTBEAT_CHANNEL = f"shard:{SHARD_ID}:heartbeat"
COMMAND_CHANNEL = f"shard:{SHARD_ID}:command"

def run_heartbeat(stop_event):
    """Publishes heartbeats periodically."""
    try:
        r = redis.from_url(REDIS_URL, decode_responses=True)
        print(f"[SimShard] Connected to Redis at {REDIS_URL}")
        
        while not stop_event.is_set():
            payload = {
                "shard_id": SHARD_ID,
                "game_count": 0,
                "max_games": 10,
                "games": [] # No games held initially
            }
            r.publish(HEARTBEAT_CHANNEL, json.dumps(payload))
            # print(f"[SimShard] Published heartbeat")
            time.sleep(1.0)
    except Exception as e:
        print(f"[SimShard] Heartbeat error: {e}")

def listen_for_commands():
    """Listens for game assignments."""
    try:
        r = redis.from_url(REDIS_URL, decode_responses=True)
        pubsub = r.pubsub()
        pubsub.subscribe(COMMAND_CHANNEL)
        
        print(f"[SimShard] Listening on {COMMAND_CHANNEL}...")
        
        start_time = time.time()
        timeout = 60 # wait up to 60 seconds for an assignment
        
        while time.time() - start_time < timeout:
            message = pubsub.get_message(ignore_subscribe_messages=True, timeout=1.0)
            if message:
                data = message['data']
                print(f"[SimShard] RECEIVED COMMAND: {data}")
                return True
            time.sleep(0.1)
            
        print("[SimShard] Timed out waiting for assignment.")
        return False
    except Exception as e:
        print(f"[SimShard] Listener error: {e}")
        return False

if __name__ == "__main__":
    print("Starting Orchestrator Verification (Simulated Shard)")
    
    stop_event = threading.Event()
    hb_thread = threading.Thread(target=run_heartbeat, args=(stop_event,))
    hb_thread.daemon = True
    hb_thread.start()
    
    success = listen_for_commands()
    
    stop_event.set()
    hb_thread.join(timeout=2)
    
    if success:
        print("VERIFICATION SUCCESS: Received assignment command.")
        sys.exit(0)
    else:
        print("VERIFICATION FAILURE: No assignment received.")
        sys.exit(1)
