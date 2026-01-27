#!/usr/bin/env python3
"""
Memory Profiling Script for Arbees Services

Monitors Docker container memory usage during live game loads.
Outputs CSV data for analysis and alerts on high memory usage.

Usage:
    python scripts/profile_memory.py --duration 3600 --interval 10
    python scripts/profile_memory.py --containers game_shard orchestrator --threshold 80

Output:
    - Real-time memory stats to stdout
    - CSV file: reports/memory_profile_{timestamp}.csv
    - Alert when memory exceeds threshold
"""

import argparse
import csv
import subprocess
import time
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional


def get_container_stats(container_filter: Optional[List[str]] = None) -> List[Dict]:
    """Get memory stats for running containers."""
    try:
        # Use docker stats with --no-stream for a single snapshot
        cmd = [
            "docker", "stats", "--no-stream",
            "--format", "{{.Name}},{{.MemUsage}},{{.MemPerc}},{{.CPUPerc}}"
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)

        if result.returncode != 0:
            print(f"Error running docker stats: {result.stderr}")
            return []

        stats = []
        for line in result.stdout.strip().split("\n"):
            if not line:
                continue

            parts = line.split(",")
            if len(parts) != 4:
                continue

            name, mem_usage, mem_pct, cpu_pct = parts

            # Filter containers if specified
            if container_filter:
                if not any(f in name for f in container_filter):
                    continue

            # Only include arbees containers
            if "arbees" not in name.lower():
                continue

            # Parse memory percentage (remove % sign)
            try:
                mem_pct_float = float(mem_pct.rstrip("%"))
            except ValueError:
                mem_pct_float = 0.0

            # Parse CPU percentage
            try:
                cpu_pct_float = float(cpu_pct.rstrip("%"))
            except ValueError:
                cpu_pct_float = 0.0

            stats.append({
                "name": name,
                "mem_usage": mem_usage,
                "mem_pct": mem_pct_float,
                "cpu_pct": cpu_pct_float,
            })

        return stats

    except subprocess.TimeoutExpired:
        print("Timeout getting docker stats")
        return []
    except Exception as e:
        print(f"Error: {e}")
        return []


def format_stats_table(stats: List[Dict], threshold: float) -> str:
    """Format stats as a table for display."""
    if not stats:
        return "No containers found"

    # Sort by memory percentage descending
    stats = sorted(stats, key=lambda x: x["mem_pct"], reverse=True)

    lines = []
    lines.append(f"{'Container':<35} {'Memory':<20} {'Mem %':<10} {'CPU %':<10} {'Status':<10}")
    lines.append("-" * 95)

    for stat in stats:
        status = "⚠️ HIGH" if stat["mem_pct"] > threshold else "OK"
        lines.append(
            f"{stat['name']:<35} {stat['mem_usage']:<20} {stat['mem_pct']:<10.1f} "
            f"{stat['cpu_pct']:<10.1f} {status:<10}"
        )

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Profile Arbees container memory usage")
    parser.add_argument(
        "--duration", type=int, default=3600,
        help="Duration to profile in seconds (default: 3600 = 1 hour)"
    )
    parser.add_argument(
        "--interval", type=int, default=10,
        help="Sampling interval in seconds (default: 10)"
    )
    parser.add_argument(
        "--threshold", type=float, default=80.0,
        help="Memory percentage threshold for alerts (default: 80)"
    )
    parser.add_argument(
        "--containers", nargs="*",
        help="Filter to specific containers (partial name match)"
    )
    parser.add_argument(
        "--output-dir", type=str, default="reports",
        help="Output directory for CSV files (default: reports)"
    )
    parser.add_argument(
        "--quiet", action="store_true",
        help="Only output alerts, not regular stats"
    )

    args = parser.parse_args()

    # Create output directory
    output_dir = Path(args.output_dir)
    output_dir.mkdir(exist_ok=True)

    # Create CSV file
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    csv_path = output_dir / f"memory_profile_{timestamp}.csv"

    print(f"Memory Profiling Started")
    print(f"========================")
    print(f"Duration: {args.duration}s, Interval: {args.interval}s, Threshold: {args.threshold}%")
    print(f"Output: {csv_path}")
    print()

    start_time = time.time()
    samples = 0
    high_memory_events = []

    with open(csv_path, "w", newline="") as csvfile:
        writer = csv.writer(csvfile)
        writer.writerow(["timestamp", "container", "mem_usage", "mem_pct", "cpu_pct"])

        while time.time() - start_time < args.duration:
            now = datetime.now()
            stats = get_container_stats(args.containers)

            if stats:
                samples += 1

                # Write to CSV
                for stat in stats:
                    writer.writerow([
                        now.isoformat(),
                        stat["name"],
                        stat["mem_usage"],
                        stat["mem_pct"],
                        stat["cpu_pct"],
                    ])

                # Check for high memory
                for stat in stats:
                    if stat["mem_pct"] > args.threshold:
                        event = f"{now.isoformat()} - {stat['name']}: {stat['mem_pct']:.1f}%"
                        high_memory_events.append(event)
                        print(f"⚠️  HIGH MEMORY: {stat['name']} at {stat['mem_pct']:.1f}%")

                # Display table
                if not args.quiet:
                    # Clear screen and show stats
                    print(f"\n[{now.strftime('%H:%M:%S')}] Sample {samples}")
                    print(format_stats_table(stats, args.threshold))

                csvfile.flush()

            time.sleep(args.interval)

    # Summary
    print("\n" + "=" * 50)
    print("PROFILING COMPLETE")
    print("=" * 50)
    print(f"Total samples: {samples}")
    print(f"CSV output: {csv_path}")

    if high_memory_events:
        print(f"\n⚠️  High memory events ({len(high_memory_events)}):")
        for event in high_memory_events[-10:]:  # Show last 10
            print(f"  {event}")
    else:
        print("\n✅ No high memory events detected")


if __name__ == "__main__":
    main()
