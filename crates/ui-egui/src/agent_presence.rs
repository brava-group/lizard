//! Temporary agent cursor and node activity overlay. It never changes the document.

use std::time::Duration;

use web_time::Instant;

use egui::{Align2, CornerRadius, Painter, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::LizardApp;
use crate::theme::Tokens;

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
    let fade = if active { 1.0 } else { (1.0 - idle_age.as_secs_f32() / 1.0).clamp(0.0, 1.0) };
    let target = presence.target.and_then(|id| app.session.ds().ok()?.doc.world_bounds(id)).map(|bounds| {
        let x = bounds.x0 + bounds.width() * 0.35;
        let y = bounds.y0 + bounds.height() * 0.35;
        (x, y)
    });
    let reduce_motion = app.reduce_motion();
    // Smooth only in page space. A pan or zoom then reprojects the agent with its frame in
    // the same UI frame, without the cursor trailing behind the camera.
    let shown = target.map(|target| {
        let old = presence.shown.unwrap_or(target);
        if reduce_motion { target } else { (old.0 + (target.0 - old.0) * 0.22, old.1 + (target.1 - old.1) * 0.22) }
    });
    let moving = shown.zip(target).is_some_and(|(shown, target)| (shown.0 - target.0).abs() + (shown.1 - target.1).abs() > 0.5);
    app.ui.agent.presence.shown = shown;
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
            painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(2.0, brand.gamma_multiply(0.65 * fade)), StrokeKind::Outside);
            if !reduce_motion {
                let visible = rect.intersect(app.canvas.rect);
                let clipped = painter.with_clip_rect(visible);
                let band = visible.width().clamp(36.0, 180.0) * 0.4;
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
        let tip = app.to_screen(x, y);
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
    if moving || !app.ui.agent.presence.shimmers.is_empty() || !app.ui.agent.presence.pulses.is_empty() || (fade > 0.0 && !active) {
        ctx.request_repaint_after(Duration::from_millis(if reduce_motion { 100 } else { 33 }));
    }
}

#[cfg(test)]
mod tests {
    use lizard_doc::NodeId;
    use lizard_engine::Session;
    use serde_json::json;

    use crate::layers_drag_tests::Ui;

    #[test]
    fn agent_cursor_uses_page_coordinates_through_pan_and_has_no_center_fallback() {
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
        ui.frame(vec![]);
        let page = ui.app.ui.agent.presence.shown.unwrap();
        assert_eq!(page, (252.0, 174.0));
        let before = ui.app.to_screen(page.0, page.1);

        ui.app.session.ds_mut().unwrap().view.cx += 100.0;
        ui.frame(vec![]);
        assert_eq!(ui.app.ui.agent.presence.shown, Some(page));
        let after = ui.app.to_screen(page.0, page.1);
        assert!(after.x < before.x - 90.0, "the marker follows the frame immediately when the camera pans");

        ui.app.ui.agent.presence.target = None;
        ui.frame(vec![]);
        assert!(ui.app.ui.agent.presence.shown.is_none(), "without a target there is no marker at canvas center");
    }
}
