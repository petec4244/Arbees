import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def read_text(rel_path: str) -> str:
    path = ROOT / rel_path
    return path.read_text(encoding="utf-8", errors="replace")


def check_env_example(contents: str) -> list[str]:
    missing = []
    for key in [
        "POLYMARKET_WS_RECONNECT_BASE",
        "POLYMARKET_WS_RECONNECT_MAX",
        "POLYMARKET_WS_RECONNECT_JITTER",
        "LIQUIDITY_MIN_THRESHOLD",
        "LIQUIDITY_MIN_THRESHOLD_KALSHI",
        "LIQUIDITY_MIN_THRESHOLD_POLYMARKET",
        "LIQUIDITY_MIN_THRESHOLD_SPORT",
        "LIQUIDITY_MIN_THRESHOLD_CRYPTO",
        "LIQUIDITY_MIN_THRESHOLD_ECONOMICS",
        "LIQUIDITY_MIN_THRESHOLD_POLITICS",
        "LIQUIDITY_MIN_THRESHOLD_ENTERTAINMENT",
    ]:
        if key not in contents:
            missing.append(key)
    return missing


def rest_poll_has_zmq(contents: str) -> bool:
    start = contents.find("async def _rest_poll_loop")
    if start == -1:
        return False
    next_def = contents.find("async def", start + 1)
    block = contents[start:next_def if next_def != -1 else None]
    return "_publish_zmq_price" in block


def main() -> int:
    failures = []

    env_example = read_text(".env.example")
    missing_env = check_env_example(env_example)
    if missing_env:
        failures.append(f".env.example missing: {', '.join(missing_env)}")

    ws_client = read_text("markets/polymarket/websocket/ws_client.py")
    for key in [
        "get_polymarket_ws_reconnect_base",
        "get_polymarket_ws_reconnect_max",
        "get_polymarket_ws_reconnect_jitter",
    ]:
        if key not in ws_client:
            failures.append(f"ws_client missing config hook: {key}")

    monitor = read_text("services/polymarket_monitor/monitor.py")
    if not rest_poll_has_zmq(monitor):
        failures.append("REST poll loop missing ZMQ publish call")

    signal_processor = read_text("services/signal_processor_rust/src/main.rs")
    if "LIQUIDITY_REJECTED" not in signal_processor:
        failures.append("signal_processor missing LIQUIDITY_REJECTED log")
    if "signals_rejected_insufficient_liquidity" not in signal_processor:
        failures.append("signal_processor missing liquidity rejection metric")

    if failures:
        print("Sprint 1/2 validation: FAILED")
        for failure in failures:
            print(f"- {failure}")
        return 1

    print("Sprint 1/2 validation: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
