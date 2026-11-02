//! Frame statistics for spotting wasted redraws: how many frames the UI drew and what asked for
//! them (the `file:line` of each repaint request), read through `ui.inspect` (`framesDrawn`,
//! `repaintCauses`) and reset with `ui.set {resetFrameStats: true}`. An idle window should draw
//! no frames at all.

use std::collections::{BTreeMap, VecDeque};

use serde_json::{Value, json};

/// How many of the most frequent repaint causes `ui.inspect` lists.
const TOP_CAUSES: usize = 12;
const RECENT_FRAMES: usize = 512;

#[derive(Default)]
pub struct FrameStats {
    /// Frames drawn since the app started (or the last reset).
    pub frames: u64,
    /// Repaint requests by call site, counted once per frame they asked for.
    causes: BTreeMap<(&'static str, u32), u64>,
    last_start: Option<web_time::Instant>,
    frame_times_ms: VecDeque<f64>,
    gpu_render_times_ms: VecDeque<f64>,
    frame_intervals_ms: VecDeque<f64>,
    uncovered_pixels: VecDeque<u64>,
    preview_frames: VecDeque<bool>,
}

impl FrameStats {
    /// Count this frame and what asked for it (call once a frame).
    pub fn record(&mut self, ctx: &egui::Context) {
        self.frames += 1;
        let now = web_time::Instant::now();
        if let Some(last) = self.last_start {
            push_recent(&mut self.frame_intervals_ms, now.duration_since(last).as_secs_f64() * 1000.0);
        }
        self.last_start = Some(now);
        for c in ctx.repaint_causes() {
            *self.causes.entry((c.file, c.line)).or_default() += 1;
        }
    }

    pub fn record_timing(&mut self, frame_ms: f64, gpu_render_ms: f64, uncovered_pixels: u64, preview: bool) {
        push_recent(&mut self.frame_times_ms, frame_ms);
        push_recent(&mut self.gpu_render_times_ms, gpu_render_ms);
        push_recent(&mut self.uncovered_pixels, uncovered_pixels);
        push_recent(&mut self.preview_frames, preview);
    }

    pub fn recent_timings(&self) -> Value {
        json!({
            "frameMs": self.frame_times_ms,
            "gpuRenderMs": self.gpu_render_times_ms,
            "intervalMs": self.frame_intervals_ms,
            "uncoveredPixels": self.uncovered_pixels,
            "preview": self.preview_frames,
        })
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The most frequent repaint causes, most frequent first: `[{at: "file:line", count}]`.
    pub fn causes(&self) -> Value {
        let mut top: Vec<(&(&'static str, u32), &u64)> = self.causes.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        top.into_iter().take(TOP_CAUSES).map(|((file, line), n)| json!({"at": format!("{file}:{line}"), "count": n})).collect()
    }
}

fn push_recent<T>(values: &mut VecDeque<T>, value: T) {
    if values.len() == RECENT_FRAMES {
        values.pop_front();
    }
    values.push_back(value);
}
