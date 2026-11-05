//! Temporary agent cursor and node activity overlay. It never changes the document.

use std::time::Duration;

use web_time::Instant;

use egui::{Align2, CornerRadius, Painter, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::LizardApp;
use crate::theme::Tokens;

const CURSOR_HOLD: Duration = Duration::from_secs(5);
const CURSOR_FADE: Duration = Duration::from_millis(1_500);
const WORK_ZONE_DWELL: Duration = Duration::from_millis(1_600);
const WORK_ZONE_RADIUS: f64 = 170.0;
const WORK_ZONE_REST: f64 = 90.0;

fn cursor_alpha(active: bool, idle: Duration) -> f32 {
    if active || idle <= CURSOR_HOLD { 1.0 } else { (1.0 - (idle - CURSOR_HOLD).as_secs_f32() / CURSOR_FADE.as_secs_f32()).clamp(0.0, 1.0) }
}

fn advance_cursor(from: (f64, f64), to: (f64, f64), dt: Duration, zoom: f64, reduce_motion: bool) -> (f64, f64) {
    if reduce_motion {
        return to;
    }
    let seconds = dt.as_secs_f64().clamp(1.0 / 60.0, 0.1);
    let progress = 1.0 - (-1.4 * seconds).exp();
    let dx = (to.0 - from.0) * progress;
    let dy = (to.1 - from.1) * progress;
    let distance = dx.hypot(dy);
    let limit = 320.0 * seconds / zoom.max(0.01);
    let scale = if distance > limit { limit / distance } else { 1.0 };
    (from.0 + dx * scale, from.1 + dy * scale)
}

fn work_zone(current: Option<(f64, f64)>, work: (f64, f64), zoom: f64, elapsed: Duration) -> ((f64, f64), bool) {
    let zoom = zoom.max(0.01);
    let Some(current) = current else {
        return ((work.0 - 28.0 / zoom, work.1 - 22.0 / zoom), true);
    };
    let dx = work.0 - current.0;
    let dy = work.1 - current.1;
    let distance = dx.hypot(dy) * zoom;
    if distance <= WORK_ZONE_RADIUS || elapsed < WORK_ZONE_DWELL {
        return (current, false);
    }
    let fraction = (distance - WORK_ZONE_REST) / distance;
    ((current.0 + dx * fraction, current.1 + dy * fraction), true)
}

fn idle_offset(idle: Duration, strength: f32) -> egui::Vec2 {
    if strength <= 0.0 {
        return egui::Vec2::ZERO;
    }
    let seconds = idle.as_secs_f32();
    vec2((seconds * 0.8).sin() * 2.0 * strength, (seconds * 0.6).cos() * 1.5 * strength)
}

fn advance_idle_strength(current: f32, idle: bool, dt: Duration, reduce_motion: bool) -> f32 {
    if reduce_motion {
        return 0.0;
    }
    let progress = 1.0 - (-4.0 * dt.as_secs_f32().clamp(1.0 / 60.0, 0.1)).exp();
    current + ((if idle { 1.0 } else { 0.0 }) - current) * progress
}

pub fn draw(app: &mut LizardApp, painter: &Painter, ctx: &egui::Context) {
    let brand = Tokens::get(ctx).accent_text;
    let now = Instant::now();
    let Some(document) = app.session.ds().ok().map(|ds| ds.uid) else { return };
    let presence = &app.ui.agent.presence;
    if presence.document != Some(document) || presence.conversation != app.ui.agent.runtime_conversation {
        return;
    }
    let active = app.ui.agent.busy && presence.active;
    let idle_age = presence.updated.map_or(Duration::MAX, |updated| now.saturating_duration_since(updated));
    let fade = cursor_alpha(active, idle_age);
    let work = presence
        .point
        .or_else(|| {
            presence.target.and_then(|id| app.session.ds().ok()?.doc.world_bounds(id)).map(|bounds| {
                let x = bounds.x0 + bounds.width() * 0.35;
                let y = bounds.y0 + bounds.height() * 0.35;
                (x, y)
            })
        });
    let zoom = app.session.ds().map_or(1.0, |ds| ds.view.zoom).max(0.01);
    let zone_elapsed = presence.destination_updated.map_or(Duration::MAX, |at| now.saturating_duration_since(at));
    let (destination, zone_changed) = if active {
        work.map_or((presence.destination, false), |work| {
            let (zone, changed) = work_zone(presence.destination, work, zoom, zone_elapsed);
            (Some(zone), changed)
        })
    } else {
        (presence.destination, false)
    };
    let target = destination.or(presence.shown).or_else(|| {
        active.then(|| app.session.ds().ok().map(|ds| ds.view.screen_to_page(f64::from(app.canvas.rect.width()) * 0.5, f64::from(app.canvas.rect.height()) * 0.5))).flatten()
    });
    let reduce_motion = app.reduce_motion();
    // Smooth only in page space. A pan or zoom then reprojects the agent with its frame in
    // the same UI frame, without the cursor trailing behind the camera.
    let motion_dt = presence.motion_updated.map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
    let shown = target.map(|target| advance_cursor(presence.shown.unwrap_or(target), target, motion_dt, zoom, reduce_motion));
    let moving = shown.zip(target).is_some_and(|(shown, target)| (shown.0 - target.0).abs() + (shown.1 - target.1).abs() > 0.5);
    let idle_goal = active && !moving && idle_age >= Duration::from_millis(750) && !reduce_motion;
    let idle_strength = advance_idle_strength(presence.idle_strength, idle_goal, motion_dt, reduce_motion);
    app.ui.agent.presence.shown = shown;
    app.ui.agent.presence.destination = destination;
    if zone_changed {
        app.ui.agent.presence.destination_updated = Some(now);
    }
    app.ui.agent.presence.motion_updated = Some(now);
    app.ui.agent.presence.idle_strength = idle_strength;
    app.ui
        .agent
        .presence
        .pulses
        .retain(|pulse| now.saturating_duration_since(pulse.started).as_secs_f32() < if pulse.inspection { 2.0 } else { 0.9 });
    for pulse in &app.ui.agent.presence.pulses {
        let Some(bounds) = app.session.ds().ok().and_then(|ds| ds.doc.world_bounds(pulse.id)) else { continue };
        let rect = Rect::from_two_pos(app.to_screen(bounds.x0, bounds.y0), app.to_screen(bounds.x1, bounds.y1));
        if !rect.is_positive() || !rect.intersects(app.canvas.rect) {
            continue;
        }
        let length = if pulse.inspection { 2.0 } else { 0.9 };
        let t = (now.saturating_duration_since(pulse.started).as_secs_f32() / length).clamp(0.0, 1.0);
        let alpha = (1.0 - t).min(t * 8.0).clamp(0.0, 1.0);
        painter.rect_stroke(rect, CornerRadius::same(5), Stroke::new(2.0, brand.gamma_multiply(alpha)), StrokeKind::Outside);
        if !reduce_motion && rect.width() > 12.0 && rect.height() > 12.0 {
            let y = rect.top() + rect.height() * t;
            let visible = rect.intersect(app.canvas.rect);
            if y >= visible.top() && y <= visible.bottom() {
                let clipped = painter.with_clip_rect(visible);
                clipped.line_segment([pos2(visible.left(), y), pos2(visible.right(), y)], Stroke::new(10.0, brand.gamma_multiply(alpha * 0.12)));
            }
        }
    }
    app.ui.agent.presence.shimmers.retain(|shimmer| {
        now.saturating_duration_since(shimmer.started) < if shimmer.ongoing { Duration::from_secs(20) } else { Duration::from_secs(2) }
    });
    for shimmer in &app.ui.agent.presence.shimmers {
        let Some(bounds) = app.session.ds().ok().and_then(|ds| ds.doc.world_bounds(shimmer.id)) else { continue };
        let rect = Rect::from_two_pos(app.to_screen(bounds.x0, bounds.y0), app.to_screen(bounds.x1, bounds.y1));
        if rect.is_positive() && rect.intersects(app.canvas.rect) {
            let elapsed = now.saturating_duration_since(shimmer.started).as_secs_f32();
            let fade = if shimmer.ongoing { 1.0 } else { (1.0 - elapsed / 2.0).clamp(0.0, 1.0) };
            painter.with_clip_rect(app.canvas.rect).rect_filled(rect, CornerRadius::same(6), brand.gamma_multiply(0.20 * fade));
            painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(2.0, brand.gamma_multiply(0.65 * fade)), StrokeKind::Outside);
            if !reduce_motion {
                let visible = rect.intersect(app.canvas.rect);
                let clipped = painter.with_clip_rect(visible);
                let band = visible.width().clamp(64.0, 240.0) * 0.7;
                let progress = (elapsed / 1.1).fract();
                let center = visible.left() - band + (visible.width() + 2.0 * band) * progress;
                for strip in 0..9 {
                    let distance = (strip as f32 - 4.0).abs() / 4.0;
                    let x = center + (strip as f32 - 4.5) * band / 9.0;
                    let stripe = Rect::from_min_size(pos2(x, visible.top()), vec2(band / 9.0 + 1.0, visible.height()));
                    clipped.rect_filled(stripe, CornerRadius::ZERO, brand.gamma_multiply((1.0 - distance) * 0.18 * fade));
                }
            }
        }
    }
    if let Some((x, y)) = shown.filter(|_| fade > 0.0) {
        let tip = app.to_screen(x, y) + idle_offset(idle_age, idle_strength);
        if app.canvas.rect.contains(tip) {
            crate::live::pointer(painter, tip, brand.gamma_multiply(fade));
            let name = app
                .ui
                .agent
                .runtime_conversation
                .and_then(|id| app.ui.agent.conversations.iter().find(|chat| chat.id == id))
                .map_or(crate::i18n::tr("Agent"), |chat| chat.agent.label());
            crate::live::name_tag(painter, tip + vec2(14.0, 15.0), Align2::LEFT_TOP, name, brand.gamma_multiply(fade));
        }
    }
    let cursor_visible = shown.is_some_and(|(x, y)| app.canvas.rect.contains(app.to_screen(x, y)));
    if moving || !app.ui.agent.presence.shimmers.is_empty() || !app.ui.agent.presence.pulses.is_empty() || (fade > 0.0 && cursor_visible) {
        ctx.request_repaint_after(Duration::from_millis(if reduce_motion {
            100
        } else if moving {
            33
        } else {
            50
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lizard_doc::NodeId;
    use lizard_engine::Session;
    use serde_json::json;

    use crate::layers_drag_tests::Ui;

    #[test]
    fn agent_cursor_uses_page_coordinates_through_pan_and_stays_at_last_work() {
        let mut session = Session::new();
        let frame =
            NodeId(session.execute("node.create", &json!({"type":"frame","x":140,"y":90,"width":320,"height":240})).unwrap()["id"].as_u64().unwrap());
        let mut ui = Ui::new(session);
        let uid = ui.app.session.ds().unwrap().uid;
        ui.app.ui.agent.busy = true;
        ui.app.ui.agent.runtime_conversation = Some(7);
        ui.app.ui.agent.presence.document = Some(uid);
        ui.app.ui.agent.presence.conversation = Some(7);
        ui.app.ui.agent.presence.active = true;
        ui.app.ui.agent.presence.target = Some(frame);
        ui.app.ui.agent.presence.updated = Some(Instant::now());
        ui.frame(vec![]);
        let page = ui.app.ui.agent.presence.shown.unwrap();
        assert_eq!(page, (224.0, 152.0));
        let before = ui.app.to_screen(page.0, page.1);

        ui.app.session.ds_mut().unwrap().view.cx += 100.0;
        ui.frame(vec![]);
        assert_eq!(ui.app.ui.agent.presence.shown, Some(page));
        let after = ui.app.to_screen(page.0, page.1);
        assert!(after.x < before.x - 90.0, "the marker follows the frame immediately when the camera pans");

        ui.app.ui.agent.presence.target = None;
        ui.frame(vec![]);
        assert_eq!(ui.app.ui.agent.presence.shown, Some(page), "an untargeted tool leaves the cursor by its last work");
    }

    #[test]
    fn planned_creation_position_is_drawn_before_the_node_exists() {
        let mut ui = Ui::new(Session::new());
        let uid = ui.app.session.ds().unwrap().uid;
        ui.app.ui.agent.busy = true;
        ui.app.ui.agent.runtime_conversation = Some(7);
        ui.app.ui.agent.presence.document = Some(uid);
        ui.app.ui.agent.presence.conversation = Some(7);
        ui.app.ui.agent.presence.active = true;
        ui.app.ui.agent.presence.point = Some((180.0, 120.0));
        ui.app.ui.agent.presence.updated = Some(Instant::now());
        ui.frame(vec![]);
        let first = ui.app.ui.agent.presence.shown.unwrap();
        assert_eq!(first, (152.0, 98.0));
        ui.app.ui.agent.presence.point = Some((380.0, 220.0));
        ui.app.ui.agent.presence.updated = Some(Instant::now());
        ui.frame(vec![]);
        assert_eq!(ui.app.ui.agent.presence.destination, Some(first), "rapid nearby calls keep one work zone");
        ui.app.ui.agent.presence.destination_updated = None;
        ui.frame(vec![]);
        let moved = ui.app.ui.agent.presence.shown.unwrap();
        assert!(moved.0 > first.0 && moved.0 < 380.0);
        assert!(moved.1 > first.1 && moved.1 < 220.0);
    }

    #[test]
    fn cursor_moves_over_time_and_idles_before_fading() {
        let from = (20.0, 30.0);
        let to = (420.0, 230.0);
        let first = advance_cursor(from, to, Duration::from_millis(33), 1.0, false);
        let second = advance_cursor(first, to, Duration::from_millis(33), 1.0, false);
        assert!(from.0 < first.0 && first.0 < second.0 && second.0 < to.0);
        assert!((first.0 - from.0).hypot(first.1 - from.1) <= 320.0 * 0.033 + 0.01);
        assert_eq!(advance_cursor(from, to, Duration::ZERO, 1.0, true), to);
        assert_eq!(cursor_alpha(true, Duration::from_secs(30)), 1.0);
        assert_eq!(cursor_alpha(false, Duration::from_secs(4)), 1.0);
        assert!(cursor_alpha(false, Duration::from_millis(5_750)) > 0.0);
        assert_eq!(cursor_alpha(false, Duration::from_secs(7)), 0.0);
        assert_eq!(idle_offset(Duration::from_millis(200), 0.0), egui::Vec2::ZERO);
        assert_ne!(idle_offset(Duration::from_secs(1), 0.5), egui::Vec2::ZERO);
        let first_idle = advance_idle_strength(0.0, true, Duration::from_millis(50), false);
        let second_idle = advance_idle_strength(first_idle, true, Duration::from_millis(50), false);
        assert!(0.0 < first_idle && first_idle < second_idle && second_idle < 1.0);
        assert!(advance_idle_strength(second_idle, false, Duration::from_millis(50), false) < second_idle);
        assert_eq!(advance_idle_strength(second_idle, true, Duration::from_millis(50), true), 0.0);
    }

    #[test]
    fn work_zone_holds_nearby_targets_and_shifts_only_after_dwell() {
        let (first, changed) = work_zone(None, (300.0, 200.0), 1.0, Duration::ZERO);
        assert!(changed);
        assert_eq!(first, (272.0, 178.0));
        assert_eq!(work_zone(Some(first), (340.0, 230.0), 1.0, Duration::from_secs(3)), (first, false));
        assert_eq!(work_zone(Some(first), (340.0, 230.0), 0.5, Duration::from_secs(3)), (first, false));
        assert_eq!(work_zone(Some(first), (700.0, 200.0), 1.0, Duration::from_millis(300)), (first, false));
        let (next, changed) = work_zone(Some(first), (700.0, 200.0), 1.0, Duration::from_secs(2));
        assert!(changed);
        assert!((next.0 - 700.0).hypot(next.1 - 200.0) > 85.0);
        assert!((next.0 - 700.0).hypot(next.1 - 200.0) < 95.0);
    }
}
