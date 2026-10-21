//! Temporary agent cursor and node activity overlay. It never changes the document.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::FigApp;

const BLUE: Color32 = Color32::from_rgb(0x7C, 0xC4, 0xF7);

pub fn draw(app: &mut FigApp, painter: &Painter, ctx: &egui::Context) {
    let now = Instant::now();
    let Some(document) = app.session.ds().ok().map(|ds| ds.uid) else { return };
    let presence = &app.ui.agent.presence;
    if presence.document != Some(document) || presence.conversation != app.ui.agent.runtime_conversation {
        return;
    }
    let active = app.ui.agent.busy && presence.active;
    let idle_age = presence.updated.map_or(Duration::MAX, |updated| now.saturating_duration_since(updated));
    let fade = if active { 1.0 } else { (1.0 - idle_age.as_secs_f32() / 1.0).clamp(0.0, 1.0) };
    let target = presence.target.and_then(|id| app.session.ds().ok()?.doc.world_bounds(id)).map(|bounds| {
        let x = bounds.x0 + bounds.width() * 0.35;
        let y = bounds.y0 + bounds.height() * 0.35;
        app.to_screen(x, y)
    });
    let target = target.unwrap_or_else(|| app.canvas.rect.center());
    let reduce_motion = app.reduce_motion();
    let old = presence.shown.map(|(x, y)| pos2(x, y)).unwrap_or(target);
    let tip = if reduce_motion { target } else { old + (target - old) * 0.22 };
    let moving = tip.distance(target) > 0.5;
    app.ui.agent.presence.shown = Some((tip.x, tip.y));
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
        painter.rect_stroke(rect, CornerRadius::same(5), Stroke::new(2.0, BLUE.gamma_multiply(alpha)), StrokeKind::Outside);
        if !reduce_motion && rect.width() > 12.0 && rect.height() > 12.0 {
            let y = rect.top() + rect.height() * t;
            let visible = rect.intersect(app.canvas.rect);
            if y >= visible.top() && y <= visible.bottom() {
                let clipped = painter.with_clip_rect(visible);
                clipped.line_segment([pos2(visible.left(), y), pos2(visible.right(), y)], Stroke::new(10.0, BLUE.gamma_multiply(alpha * 0.12)));
            }
        }
    }
    if app.ui.agent.presence.generating
        && active
        && let Some(id) = app.ui.agent.presence.target
        && let Some(bounds) = app.session.ds().ok().and_then(|ds| ds.doc.world_bounds(id))
    {
        let rect = Rect::from_two_pos(app.to_screen(bounds.x0, bounds.y0), app.to_screen(bounds.x1, bounds.y1));
        if rect.is_positive() && rect.intersects(app.canvas.rect) {
            let glow = if reduce_motion { 0.5 } else { 0.35 + 0.25 * (ctx.input(|i| i.time) as f32 * 5.0).sin().abs() };
            painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(2.0, BLUE.gamma_multiply(glow)), StrokeKind::Outside);
        }
    }
    if fade > 0.0 && app.canvas.rect.contains(tip) {
        crate::live::pointer(painter, tip, BLUE.gamma_multiply(fade));
        let name = app
            .ui
            .agent
            .runtime_conversation
            .and_then(|id| app.ui.agent.conversations.iter().find(|chat| chat.id == id))
            .map_or("Agente", |chat| chat.agent.label());
        crate::live::name_tag(painter, tip + vec2(14.0, 15.0), Align2::LEFT_TOP, name, BLUE.gamma_multiply(fade));
    }
    if moving || (active && app.ui.agent.presence.generating && !reduce_motion) || !app.ui.agent.presence.pulses.is_empty() || (fade > 0.0 && !active)
    {
        ctx.request_repaint_after(Duration::from_millis(if reduce_motion { 100 } else { 33 }));
    }
}
