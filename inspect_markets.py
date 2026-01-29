import asyncio
import os
import asyncpg
from dotenv import load_dotenv

async def inspect_markets():
    # Load .env explicitly
    load_dotenv("p:\\petes_code\\ClaudeCode\\Arbees\\.env")
    
    url = os.environ.get("DATABASE_URL")
    if not url:
        print("DATABASE_URL not found")
        return

    # Check and fix hostname for local execution
    from urllib.parse import urlparse
    
    try:
        parsed = urlparse(url)
        if parsed.hostname and parsed.hostname not in ('localhost', '127.0.0.1'):
            print(f"Replacing hostname '{parsed.hostname}' with 'localhost'")
            # Simple string replacement to preserve auth info
            url = url.replace(f"@{parsed.hostname}", "@localhost")
    except Exception as e:
        print(f"Error parsing URL: {e}")

    # print(f"Connecting to: {url}") # Debug only

    try:
        conn = await asyncpg.connect(url)
        
        print("\n--- Distinct Sports in Games ---")
        rows = await conn.fetch("SELECT DISTINCT sport FROM games")
        if not rows:
            print("No rows found in games")
        for r in rows:
            print(r['sport'])

        print("\n--- Distinct Sports in Game States ---")
        rows = await conn.fetch("SELECT DISTINCT sport FROM game_states")
        if not rows:
            print("No rows found in game_states")
        for r in rows:
            print(r['sport'])

        await conn.close()
    except Exception as e:
        print(f"Error: {e}")

if __name__ == "__main__":
    asyncio.run(inspect_markets())
