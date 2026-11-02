#!/usr/bin/env python3
"""Replay a real macOS middle-button zigzag on a visible Lizard window.

Requires macOS Accessibility/Input Monitoring access. The document is never edited.
"""

import argparse
import ctypes
import json
import math
from pathlib import Path
import random
import socket
import subprocess
import sys
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--port', type=int, required=True)
parser.add_argument('--pid', type=int, required=True)
parser.add_argument('--document-path', required=True, help='switch to and verify this already-open file')
parser.add_argument('--zooms', type=float, nargs='+', help='override seeded zooms')
parser.add_argument('--seed', type=int, default=240113)
parser.add_argument('--high-zoom', type=float, default=5.2)
parser.add_argument('--immediate', action='store_true', help='start pan right after view.set; include zoom frames and skip the pre-pan screenshot')
parser.add_argument('--output', type=Path, default=Path('/tmp/lizard-physical-benchmark.json'))
parser.add_argument('--window-x', type=int)
parser.add_argument('--window-y', type=int)
parser.add_argument('--window-width', type=int)
parser.add_argument('--window-height', type=int)
args = parser.parse_args()
if sys.platform != 'darwin':
    parser.error('physical CGEvent replay requires macOS')
screen_query = '''import AppKit
guard let screen = NSScreen.screens.first else { exit(1) }
let frame = screen.frame
let visible = screen.visibleFrame
print("\\(Int(frame.origin.x)),\\(Int(frame.origin.y)),\\(Int(frame.width)),\\(Int(frame.height));\\(Int(visible.origin.x)),\\(Int(visible.origin.y)),\\(Int(visible.width)),\\(Int(visible.height));\\(screen.backingScaleFactor)")
'''
screen_result = subprocess.check_output(['swift', '-e', screen_query], text=True).strip().split(';')
if len(screen_result) != 3:
    raise RuntimeError(f'could not read main display frame: {screen_result}')
display_frame = [int(value) for value in screen_result[0].split(',')]
visible_frame = [int(value) for value in screen_result[1].split(',')]
screen_dpr = float(screen_result[2])
visible_top_left = [visible_frame[0], display_frame[1] + display_frame[3] - visible_frame[1] - visible_frame[3],
                    visible_frame[2], visible_frame[3]]
window_args = ['window_x', 'window_y', 'window_width', 'window_height']
for name, value in zip(window_args, visible_top_left):
    if getattr(args, name) is None:
        setattr(args, name, value)
requested_window = [getattr(args, name) for name in window_args]
if requested_window != visible_top_left:
    raise RuntimeError(f'benchmark requires the full usable main display: requested={requested_window}, visible={visible_top_left}')
rng = random.Random(args.seed)
zooms = args.zooms or [round(rng.uniform(2.2, 3.4), 2), round(rng.uniform(0.55, 0.85), 2),
                       round(rng.uniform(1.8, 2.8), 2), args.high_zoom]

class CGPoint(ctypes.Structure):
    _fields_ = [('x', ctypes.c_double), ('y', ctypes.c_double)]

app = ctypes.CDLL('/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices')
cf = ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
app.CGEventCreateMouseEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint32, CGPoint, ctypes.c_uint32]
app.CGEventCreateMouseEvent.restype = ctypes.c_void_p
app.CGEventPost.argtypes = [ctypes.c_uint32, ctypes.c_void_p]
cf.CFRelease.argtypes = [ctypes.c_void_p]

def post(kind, point):
    event = app.CGEventCreateMouseEvent(None, kind, point, 2)
    if not event:
        raise RuntimeError('CGEventCreateMouseEvent failed')
    app.CGEventPost(0, event)
    cf.CFRelease(event)

def summary(values):
    if not values:
        return {}
    values = sorted(values)
    return {
        'count': len(values),
        'p50': values[math.ceil(len(values)*.50)-1],
        'p95': values[math.ceil(len(values)*.95)-1],
        'p99': values[math.ceil(len(values)*.99)-1],
        'max': values[-1],
        'over33': sum(v > 33.333 for v in values),
        'over50': sum(v > 50 for v in values),
        'over100': sum(v > 100 for v in values),
    }

with socket.create_connection(('127.0.0.1', args.port), timeout=120) as sock:
    stream = sock.makefile('r')
    request_id = 0
    def call(method, params):
        global request_id
        request_id += 1
        sock.sendall((json.dumps({'id':request_id,'method':method,'params':params})+'\n').encode())
        reply = json.loads(stream.readline())
        if not reply.get('ok'):
            raise RuntimeError(f'{method}: {reply.get("error")}')
        return reply.get('result',{})

    expected_path = str(Path(args.document_path).resolve())
    files = call('window.list', {})[0]['files']
    for index in range(len(files)):
        call('engine.execute', {'command':'doc.switch','params':{'index':index}})
        candidate_path = call('document.inspect', {}).get('path')
        if candidate_path and str(Path(candidate_path).resolve()) == expected_path:
            break
    else:
        raise RuntimeError(f'document path is not open: {expected_path}; tabs={files}')
    process = f'(first process whose unix id is {args.pid})'
    subprocess.run([
        'osascript',
        '-e', f'tell application "System Events" to tell {process} to set position of window 1 to {{{args.window_x}, {args.window_y}}}',
        '-e', f'tell application "System Events" to tell {process} to set size of window 1 to {{{args.window_width}, {args.window_height}}}',
        '-e', f'tell application "System Events" to set frontmost of {process} to true',
    ], check=True)
    actual_text = subprocess.check_output([
        'osascript', '-e', f'tell application "System Events" to tell {process} to get {{position, size}} of window 1'
    ], text=True).strip()
    actual_window = [int(value.strip()) for value in actual_text.split(',')]
    if actual_window != requested_window:
        raise RuntimeError(f'Lizard window did not fill usable display: OS={actual_window}, expected={requested_window}')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    screenshot_base = args.output.with_suffix('')
    shot = call('ui.screenshot', {'path':f'{screenshot_base}-initial.png'})
    expected_pixels = [round(args.window_width*screen_dpr), round(args.window_height*screen_dpr)]
    if [shot.get('width'), shot.get('height')] != expected_pixels:
        raise RuntimeError(f'wrong screenshot size: {shot}; expected {expected_pixels}')
    canvas = call('ui.inspect', {})['canvas']['rect']
    if canvas[2] <= canvas[0] or canvas[3] <= canvas[1]:
        raise RuntimeError(f'invalid canvas rect: {canvas}')
    def point(u,v):
        return CGPoint(args.window_x+canvas[0]+(canvas[2]-canvas[0])*u, args.window_y+canvas[1]+(canvas[3]-canvas[1])*v)

    results = []
    for stage, zoom in enumerate(zooms):
        if args.immediate:
            call('ui.set', {'resetFrameStats':True})
        zoom_started = time.monotonic()
        call('engine.execute', {'command':'view.set','params':{'zoom':zoom,'cx':600,'cy':380}})
        zoom_ack = time.monotonic()
        screenshot = None
        sync_shot_ms = None
        if not args.immediate:
            screenshot = f'{screenshot_base}-start-{stage}.png'
            shot_started = time.monotonic()
            shot = call('ui.screenshot', {'path':screenshot})
            sync_shot_ms = (time.monotonic()-shot_started)*1000
            if [shot.get('width'), shot.get('height')] != expected_pixels:
                raise RuntimeError(f'window changed size at zoom {zoom}: {shot}')
            before = call('engine.execute', {'command':'view.get','params':{}})
            call('ui.set', {'resetFrameStats':True})
        else:
            before = {'cx':600.0,'cy':380.0}
        start = point(.18, .2)
        post(5, start)
        time.sleep(.03)
        post(25, start)
        drag_start = time.monotonic()
        for step in range(36):
            row = step//6
            progress = (step%6+1)/6
            u = .18+.64*progress if row%2==0 else .82-.64*progress
            v = .2+.6*(row/5) if row%2==0 else .8-.6*(row/5)
            post(27, point(u,v))
            time.sleep(1/60)
            if step == 17:
                middle = call('engine.execute', {'command':'view.get','params':{}})
        post(26, point(.18,.2))
        drag_elapsed = time.monotonic()-drag_start
        after = call('engine.execute', {'command':'view.get','params':{}})
        stats = call('ui.inspect',{})
        timings = stats['recentFrameTimings']
        travel = math.dist((before['cx'], before['cy']), (middle['cx'], middle['cy']))
        final_travel = math.dist((before['cx'], before['cy']), (after['cx'], after['cy']))
        if max(travel, final_travel) < 1.0:
            raise RuntimeError(f'camera did not pan at zoom {zoom}: start={before}, middle={middle}, end={after}')
        result = {
            'zoom': zoom,
            'cameraBefore': [before.get('cx'),before.get('cy')],
            'cameraMiddle': [middle.get('cx'),middle.get('cy')],
            'cameraAfter': [after.get('cx'),after.get('cy')],
            'sampledTravel': travel,
            'finalTravel':final_travel,
            'midpointStalled':travel < 1.0,
            'dragSeconds':drag_elapsed,
            'zoomAckMs':(zoom_ack-zoom_started)*1000,
            'syncShotMs':sync_shot_ms,
            'zoomToDragMs':(drag_start-zoom_ack)*1000,
            'framesDrawn':stats.get('framesDrawn'),
            'frameMs':summary(timings.get('frameMs',[])),
            'gpuRenderMs':summary(timings.get('gpuRenderMs',[])),
            'intervalMs':summary(timings.get('intervalMs',[])),
            # This is the canvas coverage proxy, not a pixel readback from the screen.
            'uncoveredProxyMax':max(timings.get('uncoveredPixels',[]) or [0]),
            'previewFrames':sum(timings.get('preview',[])),
            'timings':timings,
            'screenshot':screenshot,
        }
        post(5, CGPoint(100.0, 100.0))
        time.sleep(.03)
        end_screenshot = f'{screenshot_base}-end-{stage}.png'
        end_shot = call('ui.screenshot', {'path':end_screenshot})
        if [end_shot.get('width'), end_shot.get('height')] != expected_pixels:
            raise RuntimeError(f'window changed size after pan at zoom {zoom}: {end_shot}')
        result['endScreenshot'] = end_screenshot
        results.append(result)
        print(json.dumps({key:value for key,value in result.items() if key != 'timings'}),flush=True)
    document = call('document.inspect',{})
    if not document.get('path') or str(Path(document['path']).resolve()) != expected_path:
        raise RuntimeError(f'active document changed during replay: {document.get("path")}')
    report = {'port':args.port,'pid':args.pid,'seed':args.seed,'zooms':zooms,'document':document.get('name'),
              'documentPath':document.get('path'),
              'displayFramePoints':display_frame,'displayVisiblePoints':visible_frame,
              'windowActualPoints':actual_window,'windowPixels':expected_pixels,'screenDpr':screen_dpr,
              'canvasRect':canvas,'stages':results}
    args.output.write_text(json.dumps(report,indent=2),encoding='utf-8')
    print(f'wrote {args.output}')
