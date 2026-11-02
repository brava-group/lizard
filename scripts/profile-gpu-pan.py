#!/usr/bin/env python3
"""Exercise native Lizard pan/zoom through the control channel without editing the document."""

import argparse
import json
import math
import random
import socket
import subprocess
import time


def percentile(values, fraction):
    if not values:
        return None
    ordered = sorted(values)
    return round(ordered[min(len(ordered) - 1, max(0, int(len(ordered) * fraction + 0.999999) - 1))], 2)


def summarize(values):
    return {
        "p50": percentile(values, 0.50),
        "p95": percentile(values, 0.95),
        "p99": percentile(values, 0.99),
        "max": round(max(values), 2) if values else None,
    }


def scroll_events(zoom, steps, rng, pointer):
    for step in range(steps):
        direction = 1 if (step // 6) % 2 == 0 else -1
        dx = direction * zoom * 200 * rng.uniform(0.85, 1.15)
        dy = (-direction if (step // 3) % 2 else direction) * zoom * 65 * rng.uniform(0.85, 1.15)
        yield True, [{"move": pointer}, {"scroll": [dx, dy]}]


def middle_drag_events(zoom, canvas_rect, rng):
    x0, y0, x1, y1 = canvas_rect
    left, right = x0 + 40, x1 - 40
    span = right - left
    middle_y = (y0 + y1) * 0.5
    segments = max(1, math.ceil(1200 * zoom / span))
    for leg in (1, -1):
        for segment in range(segments):
            # Moving the pointer left reveals document content farther right.
            start_x, end_x = (right, left) if leg == 1 else (left, right)
            tilt = 95 if segment % 2 else -95
            start = [start_x, middle_y + tilt]
            end = [end_x, middle_y - tilt]
            yield False, [{"move": start}, {"down": start, "button": "middle"}]
            for part in range(1, 7):
                fraction = part / 6
                point = [start_x + (end_x - start_x) * fraction, start[1] + (end[1] - start[1]) * fraction + rng.uniform(-3, 3)]
                yield True, [{"move": point}]
            yield False, [{"up": end, "button": "middle"}]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=7994)
    parser.add_argument("--activate-pid", type=int, help="bring this macOS app process to the front after starting the harness")
    parser.add_argument("--seed", type=int, default=240113)
    parser.add_argument("--steps", type=int, default=36)
    parser.add_argument("--high-zoom", type=float, default=5.2)
    parser.add_argument("--cadence-hz", type=float, default=60.0)
    parser.add_argument("--settle-ms", type=float, default=0.0, help="wait after each zoom so background page cache can complete")
    parser.add_argument("--inspect-each", action="store_true")
    parser.add_argument("--verify-pan", action="store_true", help="sample view.get after every move; use as a separate validation run")
    parser.add_argument("--pan-mode", choices=("scroll", "middle"), default="scroll")
    parser.add_argument("--replay", action="store_true", help="repeat each exact pan route at the same zoom")
    parser.add_argument("--output", default="/tmp/lizard-gpu-zigzag.json")
    args = parser.parse_args()
    rng = random.Random(args.seed)
    zooms = [round(rng.uniform(2.2, 3.4), 2), round(rng.uniform(0.55, 0.85), 2), round(rng.uniform(1.8, 2.8), 2), args.high_zoom]
    report = {"port": args.port, "seed": args.seed, "zooms": zooms, "panMode": args.pan_mode, "stages": [], "replays": []}
    with socket.create_connection(("127.0.0.1", args.port), timeout=75) as connection:
        if args.activate_pid:
            subprocess.run([
                "osascript", "-e",
                f'tell application "System Events" to set frontmost of (first process whose unix id is {args.activate_pid}) to true',
            ], check=True)
        stream = connection.makefile("r")
        request_id = 0

        def call(method, params):
            nonlocal request_id
            request_id += 1
            payload = {"id": request_id, "method": method, "params": params}
            start = time.perf_counter()
            connection.sendall((json.dumps(payload) + "\n").encode())
            line = stream.readline()
            if not line:
                raise RuntimeError(f"control connection closed during {method}")
            reply = json.loads(line)
            if not reply.get("ok"):
                raise RuntimeError(f"{method}: {reply.get('error')}")
            return reply.get("result", {}), (time.perf_counter() - start) * 1000.0

        if args.activate_pid:
            first_shot, _ = call("ui.screenshot", {"path": "/tmp/lizard-gpu-harness-initial.png"})
            report["windowPixels"] = [first_shot.get("width"), first_shot.get("height")]
        initial, _ = call("ui.inspect", {})
        report["canvasRect"] = initial["canvas"]["rect"]
        x0, y0, x1, y1 = initial["canvas"]["rect"]
        pointer = [(x0 + x1) * 0.5, (y0 + y1) * 0.5]
        call("ui.input", {"events": [{"move": pointer}]})

        for stage_number, zoom in enumerate(zooms, 1):
            call("engine.execute", {"command": "view.set", "params": {"zoom": zoom, "cx": 0 if args.pan_mode == "middle" else 600, "cy": 380}})
            if args.settle_ms > 0:
                time.sleep(args.settle_ms / 1000.0)
            start_view, _ = call("engine.execute", {"command": "view.get", "params": {}})
            call("ui.set", {"resetFrameStats": True})
            stage_start = time.perf_counter()
            records = []
            inspected = initial
            events = list(scroll_events(zoom, args.steps, rng, pointer) if args.pan_mode == "scroll" else middle_drag_events(zoom, initial["canvas"]["rect"], rng))
            previous_camera = (start_view["cx"], start_view["cy"])
            sampled_travel = 0.0
            for action_index, (is_move, action_events) in enumerate(events):
                due = stage_start + action_index / args.cadence_hz
                if due > time.perf_counter():
                    time.sleep(due - time.perf_counter())
                _, input_ms = call("ui.input", {"events": action_events})
                if args.verify_pan and is_move:
                    current_view, _ = call("engine.execute", {"command": "view.get", "params": {}})
                    camera = (current_view["cx"], current_view["cy"])
                    sampled_travel += math.dist(previous_camera, camera)
                    previous_camera = camera
                if args.inspect_each:
                    inspected, inspect_ms = call("ui.inspect", {})
                else:
                    inspect_ms = 0.0
                if not is_move:
                    continue
                canvas = inspected.get("canvas", {})
                records.append({
                    "step": len(records),
                    "inputMs": round(input_ms, 2),
                    "inspectMs": round(inspect_ms, 2),
                    "frameMs": inspected.get("frameMs"),
                    "gpuRenderMs": canvas.get("gpuRenderMs"),
                    "uncoveredPixels": canvas.get("uncoveredPixels"),
                    "quality": canvas.get("quality"),
                    "framesDrawn": inspected.get("framesDrawn"),
                })
            inspected, _ = call("ui.inspect", {})
            elapsed = time.perf_counter() - stage_start
            screenshot = f"/tmp/lizard-gpu-zigzag-{stage_number}.png"
            shot_info, _ = call("ui.screenshot", {"path": screenshot})
            end_view, _ = call("engine.execute", {"command": "view.get", "params": {}})
            timings = inspected.get("recentFrameTimings", {})
            frames = timings.get("frameMs", [row["frameMs"] for row in records if isinstance(row["frameMs"], (int, float))])
            renders = timings.get("gpuRenderMs", [row["gpuRenderMs"] for row in records if isinstance(row["gpuRenderMs"], (int, float))])
            intervals = timings.get("intervalMs", [])
            uncovered = timings.get("uncoveredPixels", [])
            preview = timings.get("preview", [])
            input_times = [row["inputMs"] for row in records]
            report["stages"].append({
                "zoom": zoom,
                "cameraStart": [start_view["cx"], start_view["cy"]],
                "cameraEnd": [end_view["cx"], end_view["cy"]],
                "sampledPanTravel": round(sampled_travel, 2) if args.verify_pan else None,
                "steps": len(records),
                "elapsedSeconds": round(elapsed, 2),
                "throughputEventsPerSecond": round(len(records) / elapsed, 2),
                "frameMs": summarize(frames),
                "gpuRenderMs": summarize(renders),
                "frameIntervalMs": summarize(intervals),
                "inputAckMs": summarize(input_times),
                "framesDrawn": inspected.get("framesDrawn", 0),
                "framesOver16_7Ms": sum(value > 16.7 for value in frames),
                "uncoveredFrames": sum(value > 0 for value in uncovered),
                "previewFrames": sum(preview),
                "sampledUncoveredSteps": sum((row["uncoveredPixels"] or 0) > 0 for row in records) if args.inspect_each else None,
                "sampledPreviewSteps": sum(row["quality"] != "refined" for row in records) if args.inspect_each else None,
                "screenshot": screenshot,
                "screenshotPixels": [shot_info.get("width"), shot_info.get("height")],
                "canvasRect": inspected.get("canvas", {}).get("rect"),
                "records": records,
            })
            if args.replay:
                call("engine.execute", {"command": "view.set", "params": {"zoom": zoom, "cx": 0 if args.pan_mode == "middle" else 600, "cy": 380}})
                call("ui.set", {"resetFrameStats": True})
                replay_start = time.perf_counter()
                replay_acks = []
                for _, (is_move, action_events) in enumerate(events):
                    _, input_ms = call("ui.input", {"events": action_events})
                    if is_move:
                        replay_acks.append(input_ms)
                replay_state, _ = call("ui.inspect", {})
                replay_timings = replay_state.get("recentFrameTimings", {})
                replay_shot = f"/tmp/lizard-gpu-replay-{stage_number}.png"
                call("ui.screenshot", {"path": replay_shot})
                report["replays"].append({
                    "zoom": zoom,
                    "steps": len(replay_acks),
                    "elapsedSeconds": round(time.perf_counter() - replay_start, 2),
                    "frameMs": summarize(replay_timings.get("frameMs", [])),
                    "gpuRenderMs": summarize(replay_timings.get("gpuRenderMs", [])),
                    "frameIntervalMs": summarize(replay_timings.get("intervalMs", [])),
                    "inputAckMs": summarize(replay_acks),
                    "uncoveredFrames": sum(value > 0 for value in replay_timings.get("uncoveredPixels", [])),
                    "previewFrames": sum(replay_timings.get("preview", [])),
                    "screenshot": replay_shot,
                })
    with open(args.output, "w", encoding="utf-8") as destination:
        json.dump(report, destination, indent=2)
    print(json.dumps({key: value for key, value in report.items() if key != "stages"}))
    for stage in report["stages"]:
        print(json.dumps({key: value for key, value in stage.items() if key != "records"}))
    for stage in report["replays"]:
        print("replay " + json.dumps(stage))
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
