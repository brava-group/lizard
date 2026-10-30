#!/usr/bin/env python3
"""Drive repeatable pan and wheel-zoom events through a running Lizard control port.

This measures control replies and UI counters, not display-photon latency. Keep the
window visible and avoid concurrent builds when comparing renderer configurations.
"""

import argparse
import json
import math
import socket
import statistics
import subprocess
import time


def summary(values):
    ordered = sorted(values)
    return {
        "medianMs": round(statistics.median(ordered), 2),
        "p95Ms": round(ordered[math.ceil(len(ordered) * 0.95) - 1], 2),
        "maxMs": round(ordered[-1], 2),
        "over50": sum(value > 50 for value in ordered),
    }


def frontmost_pid():
    result = subprocess.run(
        ["swift", "-e", "import AppKit; print(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1)"],
        check=True, capture_output=True, text=True,
    )
    return int(result.stdout.strip())


def ensure_frontmost(pid):
    subprocess.run(
        ["osascript", "-e", f'tell application "System Events" to set frontmost of (first process whose unix id is {pid}) to true'],
        check=True, capture_output=True, text=True,
    )
    active = frontmost_pid()
    if active != pid:
        raise RuntimeError(f"Lizard is not frontmost: expected {pid}, found {active}")
    return active


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--positions", type=int, default=60)
    parser.add_argument("--rate", type=float, default=75)
    parser.add_argument("--warm-seconds", type=float, default=4)
    parser.add_argument("--activate-pid", type=int, help="bring this macOS process to the current Space before input")
    args = parser.parse_args()
    if args.positions < 2 or args.rate <= 0:
        parser.error("positions must be at least 2 and rate must be positive")

    with socket.create_connection(("127.0.0.1", args.port), timeout=10) as connection:
        connection.settimeout(30)
        stream = connection.makefile("rwb")
        request_id = 0

        def call(method, params=None):
            nonlocal request_id
            request_id += 1
            before = time.monotonic()
            stream.write((json.dumps({"id": request_id, "method": method, "params": params or {}}) + "\n").encode())
            stream.flush()
            reply = json.loads(stream.readline())
            if not reply.get("ok"):
                raise RuntimeError(reply)
            return reply["result"], (time.monotonic() - before) * 1000

        call("engine.execute", {"command": "view.zoomFit", "params": {}})
        time.sleep(args.warm_seconds)
        initial, _ = call("ui.inspect")
        x0, y0, x1, y1 = initial["canvas"]["rect"]
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        ax, ay = min((x1 - x0) * 0.24, 200), min((y1 - y0) * 0.16, 140)
        call("ui.set", {"resetFrameStats": True})
        call("engine.execute", {"command": "tool.hand", "params": {}})
        before_pan = ensure_frontmost(args.activate_pid) if args.activate_pid is not None else None
        call("ui.input", {"events": [{"move": [cx, cy]}, {"down": [cx, cy]}]})

        pan = []
        deadline = time.monotonic()
        for i in range(args.positions):
            x = cx + ax * math.sin(i * 0.22)
            y = cy + ay * math.sin(i * 0.17)
            _, elapsed = call("ui.input", {"events": [{"move": [x, y]}]})
            pan.append(elapsed)
            deadline += 1 / args.rate
            time.sleep(max(0, deadline - time.monotonic()))
        call("ui.input", {"events": [{"up": [cx, cy]}, {"move": [cx, cy]}]})

        before_zoom = ensure_frontmost(args.activate_pid) if args.activate_pid is not None else None

        zoom = []
        deadline = time.monotonic()
        for i in range(args.positions):
            dy = 5 if i < args.positions // 2 else -5
            _, elapsed = call("ui.input", {"events": [{"scroll": [0, dy], "cmd": True}]})
            zoom.append(elapsed)
            deadline += 1 / args.rate
            time.sleep(max(0, deadline - time.monotonic()))
        final, _ = call("ui.inspect")
        after_zoom = frontmost_pid() if args.activate_pid is not None else None

    print(json.dumps({
        "port": args.port,
        "positionsPerPhase": args.positions,
        "rateHz": args.rate,
        "warmSeconds": args.warm_seconds,
        "frontmostBeforePan": before_pan,
        "frontmostBeforeZoom": before_zoom,
        "frontmostAfterZoom": after_zoom,
        "panControl": summary(pan),
        "zoomControl": summary(zoom),
        "framesDrawn": final["framesDrawn"],
        "finalCanvasQuality": final["canvas"]["quality"],
        "finalFrameMs": final["frameMs"],
    }, indent=2))


if __name__ == "__main__":
    main()
