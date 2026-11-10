//! Temporary agent cursor and node activity overlay. It never changes the document.

use std::time::Duration;

use web_time::Instant;

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};
use serde_json::Value;

use crate::LizardApp;
use crate::theme::Tokens;

/// Start a short canvas fade for a committed node. It is visual only and does not change history.
pub fn start_entrance(presence: &mut crate::agent_panel::AgentPresence, id: lizard_doc::NodeId) {
    presence.entrances.retain(|entrance| entrance.id != id);
    if presence.entrances.len() >= 32 {
        presence.entrances.remove(0);
    }
    presence.entrances.push(crate::agent_panel::AgentEntrance { id, started: Instant::now() });
}

/// Animate an atomic batch on the canvas after the editor has committed it. The agent receives
/// the batch result immediately; this visual playback is independent of its next tool call.
pub fn schedule_batch_reveal(presence: &mut crate::agent_panel::AgentPresence, ids: &[lizard_doc::NodeId]) {
    let now = Instant::now();
    for (index, id) in ids.iter().take(32).enumerate() {
        presence.entrances.push(crate::agent_panel::AgentEntrance { id: *id, started: now + Duration::from_millis(index as u64 * 20) });
    }
    if presence.entrances.len() > 32 {
        presence.entrances.drain(..presence.entrances.len() - 32);
    }
    presence.cursor_trail.clear();
    let stride = ids.len().div_ceil(8).max(1);
    presence.cursor_trail.extend(ids.iter().step_by(stride).copied());
    if let Some(last) = ids.last()
        && presence.cursor_trail.last() != Some(last)
    {
        presence.cursor_trail.push(*last);
    }
    presence.cursor_trail_next = (!presence.cursor_trail.is_empty()).then_some(now);
}

fn entrance_background(app: &LizardApp, id: lizard_doc::NodeId, fallback: Color32) -> Color32 {
    let Ok(ds) = app.session.ds() else { return fallback };
    let mut parent = ds.doc.node(id).and_then(|node| node.parent);
    for _ in 0..12 {
        let Some(node) = parent.and_then(|id| ds.doc.node(id)) else { break };
        if let Some(color) = node.fills.iter().rev().find_map(|paint| {
            if !paint.visible || paint.opacity < 0.98 {
                return None;
            }
            match paint.kind {
                lizard_color::PaintKind::Solid { color } if color.a >= 0.98 => Some(color),
                _ => None,
            }
        }) {
            let [r, g, b, _] = color.to_rgba8();
            return Color32::from_rgb(r, g, b);
        }
        if let lizard_doc::NodeKind::Page { background, background_visible: true, .. } = node.kind {
            let [r, g, b, _] = background.to_rgba8();
            return Color32::from_rgb(r, g, b);
        }
        parent = node.parent;
    }
    fallback
}

const CURSOR_HOLD: Duration = Duration::from_secs(5);
const CURSOR_FADE: Duration = Duration::from_millis(1_500);

/// A transient canvas preview. The document is only changed by the completed tool call.
#[derive(Clone, Debug)]
pub struct NodeDraft {
    pub source: String,
    pub args: Value,
    raw: String,
    started: Instant,
    updated: Instant,
}

/// Recover complete top-level fields from a JSON prefix while the agent is still writing it.
fn draft_object(raw: &str) -> Option<Value> {
    if raw.len() > 64 * 1024 {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(raw)
        && value.is_object()
    {
        return Some(value);
    }
    let mut depth = 0_i32;
    let mut quoted = false;
    let mut escaped = false;
    let mut last_field = None;
    for (index, ch) in raw.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            ',' if depth == 1 => last_field = Some(index),
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    let end = last_field?;
    serde_json::from_str::<Value>(&format!("{}}}", &raw[..end])).ok().filter(Value::is_object)
}

pub fn update_draft(presence: &mut crate::agent_panel::AgentPresence, source: &str, input: &Value) {
    let (raw, args) = match input {
        Value::Object(fields) => {
            let mut args =
                presence.draft.as_ref().filter(|draft| draft.source == source).and_then(|draft| draft.args.as_object().cloned()).unwrap_or_default();
            args.extend(fields.clone());
            let args = Value::Object(args);
            let raw = args.to_string();
            if raw.len() > 64 * 1024 {
                return;
            }
            (raw, Some(args))
        }
        Value::String(chunk) => {
            let previous = presence.draft.as_ref().filter(|draft| draft.source == source).map_or("", |draft| draft.raw.as_str());
            let raw = if chunk.starts_with(previous) { chunk.clone() } else { format!("{previous}{chunk}") };
            if raw.len() > 64 * 1024 {
                return;
            }
            let args = draft_object(&raw);
            (raw, args)
        }
        _ => return,
    };
    let args = args.unwrap_or_else(|| presence.draft.as_ref().filter(|draft| draft.source == source).map_or(Value::Null, |draft| draft.args.clone()));
    let started = presence.draft.as_ref().filter(|draft| draft.source == source).map_or_else(Instant::now, |draft| draft.started);
    presence.draft = Some(NodeDraft { source: source.to_owned(), args, raw, started, updated: Instant::now() });
}

fn draft_rect(app: &LizardApp, args: &Value) -> Option<Rect> {
    let kind = args.get("type")?.as_str()?;
    let width = args.get("width").and_then(Value::as_f64).unwrap_or(if kind == "text" { 160.0 } else { 100.0 });
    let height = args.get("height").and_then(Value::as_f64).unwrap_or(if kind == "text" { 32.0 } else { 100.0 });
    if !width.is_finite() || !height.is_finite() || !(0.0..=20_000.0).contains(&width) || !(0.0..=20_000.0).contains(&height) {
        return None;
    }
    let mut x = args.get("x").and_then(Value::as_f64).unwrap_or(0.0);
    let mut y = args.get("y").and_then(Value::as_f64).unwrap_or(0.0);
    if let Some(parent) = args.get("parent").and_then(lizard_engine::params::id_of)
        && let Ok(doc) = app.session.doc()
        && let Some(bounds) = doc.world_bounds(parent)
    {
        if let Some(node) = doc.node(parent)
            && let Some(layout) = &node.layout
        {
            x = bounds.x0 + layout.padding[3];
            y = bounds.y0 + layout.padding[0];
            if let Some(last) = node.children.last().and_then(|id| doc.world_bounds(*id)) {
                match layout.mode {
                    lizard_doc::LayoutMode::Vertical => y = last.y1 + layout.gap,
                    _ => x = last.x1 + layout.gap,
                }
            }
        } else {
            x += bounds.x0;
            y += bounds.y0;
        }
    }
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let rect = Rect::from_two_pos(app.to_screen(x, y), app.to_screen(x + width, y + height));
    (rect.is_positive() && rect.intersects(app.canvas.rect)).then_some(rect)
}

fn draft_fill(args: &Value, fallback: Color32) -> Color32 {
    let color = args.pointer("/props/fill").or_else(|| args.get("color")).and_then(Value::as_str);
    color
        .and_then(lizard_color::Color::from_hex)
        .map(|color| {
            let [r, g, b, _] = color.to_rgba8();
            Color32::from_rgba_unmultiplied(r, g, b, 190)
        })
        .unwrap_or(fallback.gamma_multiply(0.18))
}

fn draw_draft(app: &LizardApp, painter: &Painter, now: Instant, brand: Color32, reduce_motion: bool) -> bool {
    let Some(draft) = app.ui.agent.presence.draft.as_ref() else { return false };
    let Some(rect) = draft_rect(app, &draft.args) else { return false };
    let progress = if reduce_motion { 1.0 } else { (now.saturating_duration_since(draft.started).as_secs_f32() / 0.36).clamp(0.0, 1.0) };
    let visible = rect.intersect(app.canvas.rect);
    let clip = Rect::from_min_max(visible.min, pos2(visible.left() + visible.width() * progress, visible.bottom()));
    let painter = painter.with_clip_rect(clip);
    let kind = draft.args.get("type").and_then(Value::as_str).unwrap_or("");
    let fill = draft_fill(&draft.args, brand);
    let radius = draft.args.pointer("/props/cornerRadius").and_then(Value::as_f64).unwrap_or(4.0).clamp(0.0, 60.0) as u8;
    if kind == "ellipse" {
        painter.add(egui::Shape::ellipse_filled(rect.center(), rect.size() * 0.5, fill));
    } else if kind != "text" {
        painter.rect_filled(rect, CornerRadius::same(radius), fill);
    }
    painter.rect_stroke(rect, CornerRadius::same(radius), Stroke::new(1.5, brand.gamma_multiply(0.85)), StrokeKind::Inside);
    if kind == "text"
        && let Some(text) = draft.args.get("text").and_then(Value::as_str)
    {
        let size = draft.args.pointer("/props/fontSize").and_then(Value::as_f64).unwrap_or(16.0).clamp(8.0, 120.0) as f32;
        painter.text(rect.min, Align2::LEFT_TOP, text.chars().take(200).collect::<String>(), FontId::proportional(size), fill);
    }
    true
}

fn cursor_alpha(active: bool, idle: Duration) -> f32 {
    if active || idle <= CURSOR_HOLD { 1.0 } else { (1.0 - (idle - CURSOR_HOLD).as_secs_f32() / CURSOR_FADE.as_secs_f32()).clamp(0.0, 1.0) }
}

fn advance_cursor(from: (f64, f64), to: (f64, f64), dt: Duration, zoom: f64, reduce_motion: bool) -> (f64, f64) {
    if reduce_motion {
        return to;
    }
    let seconds = dt.as_secs_f64().clamp(1.0 / 60.0, 0.1);
    let progress = 1.0 - (-18.0 * seconds).exp();
    let dx = (to.0 - from.0) * progress;
    let dy = (to.1 - from.1) * progress;
    let distance = dx.hypot(dy);
    let limit = 2_400.0 * seconds / zoom.max(0.01);
    let scale = if distance > limit { limit / distance } else { 1.0 };
    (from.0 + dx * scale, from.1 + dy * scale)
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

/// A point travelling clockwise around a frame, measured from its top-left corner.
fn frame_border_point(rect: Rect, distance: f32) -> Pos2 {
    let width = rect.width();
    let height = rect.height();
    let perimeter = 2.0 * (width + height);
    let distance = distance.rem_euclid(perimeter);
    if distance < width {
        pos2(rect.left() + distance, rect.top())
    } else if distance < width + height {
        pos2(rect.right(), rect.top() + distance - width)
    } else if distance < 2.0 * width + height {
        pos2(rect.right() - (distance - width - height), rect.bottom())
    } else {
        pos2(rect.left(), rect.bottom() - (distance - 2.0 * width - height))
    }
}

fn border_color_with_opacity(color: Color32, opacity: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (opacity.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub(crate) fn draw_frame_border(painter: &Painter, rect: Rect, brand: Color32, elapsed: f32, fade: f32, reduce_motion: bool) {
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(2.0, border_color_with_opacity(brand, 0.7 * fade)), StrokeKind::Outside);
    if reduce_motion {
        return;
    }
    let perimeter = 2.0 * (rect.width() + rect.height());
    let head = (elapsed / 2.8).fract() * perimeter;
    let tail = (perimeter * 0.4).clamp(120.0, 560.0).min(perimeter * 0.48);
    let highlight = Color32::from_rgb(
        ((u16::from(brand.r()) + 255) / 2) as u8,
        ((u16::from(brand.g()) + 255) / 2) as u8,
        ((u16::from(brand.b()) + 255) / 2) as u8,
    );
    const SEGMENTS: usize = 48;
    for index in 0..SEGMENTS {
        let start = head - tail * (index + 1) as f32 / SEGMENTS as f32;
        let end = head - tail * index as f32 / SEGMENTS as f32;
        let strength = (1.0 - index as f32 / SEGMENTS as f32).sqrt() * fade;
        let points = [frame_border_point(rect, start), frame_border_point(rect, end)];
        painter.line_segment(points, Stroke::new(9.0, border_color_with_opacity(brand, 0.38 * strength)));
        painter.line_segment(points, Stroke::new(3.5, border_color_with_opacity(highlight, strength)));
    }
}

/// Place the visible pointer and tag near the real work point without covering another agent.
/// The page-space cursor stays unchanged; only this canvas overlay moves.
fn cursor_placement(real_tip: Pos2, tag_size: Vec2, canvas: Rect, occupied: &[Rect]) -> Option<(Pos2, [Rect; 2])> {
    const SLOTS: [(f32, f32); 25] = [
        (0.0, 0.0),
        (-1.0, 0.0),
        (1.0, 0.0),
        (0.0, -1.0),
        (0.0, 1.0),
        (-1.0, -1.0),
        (1.0, -1.0),
        (-1.0, 1.0),
        (1.0, 1.0),
        (-2.0, 0.0),
        (2.0, 0.0),
        (0.0, -2.0),
        (0.0, 2.0),
        (-2.0, -1.0),
        (2.0, -1.0),
        (-2.0, 1.0),
        (2.0, 1.0),
        (-1.0, -2.0),
        (1.0, -2.0),
        (-1.0, 2.0),
        (1.0, 2.0),
        (-2.0, -2.0),
        (2.0, -2.0),
        (-2.0, 2.0),
        (2.0, 2.0),
    ];
    let spacing = vec2((tag_size.x + 30.0).max(80.0), (tag_size.y + 30.0).max(52.0));
    SLOTS.into_iter().find_map(|(column, row)| {
        let tip = real_tip + vec2(column * spacing.x, row * spacing.y);
        let pointer = Rect::from_min_size(tip - vec2(3.5, 2.5), vec2(22.0, 22.0));
        let tag = Rect::from_min_size(tip + vec2(14.0, 15.0), tag_size);
        let fits = [pointer, tag].iter().all(|rect| canvas.contains(rect.min) && canvas.contains(rect.max));
        let clear = [pointer, tag].iter().all(|rect| occupied.iter().all(|other| !rect.expand(4.0).intersects(*other)));
        (fits && clear).then_some((tip, [pointer, tag]))
    })
}

pub fn draw(app: &mut LizardApp, painter: &Painter, ctx: &egui::Context) {
    let mut occupied = Vec::with_capacity(12);
    draw_one(app, painter, ctx, 0, &mut occupied);
    for index in 0..app.ui.agent.other_presences.len() {
        std::mem::swap(&mut app.ui.agent.presence, &mut app.ui.agent.other_presences[index]);
        draw_one(app, painter, ctx, index + 1, &mut occupied);
        std::mem::swap(&mut app.ui.agent.presence, &mut app.ui.agent.other_presences[index]);
    }
}

fn draw_one(app: &mut LizardApp, painter: &Painter, ctx: &egui::Context, agent_index: usize, occupied: &mut Vec<Rect>) {
    let brand = crate::cursors::MULTIPLAYER_COLORS[agent_index % crate::cursors::MULTIPLAYER_COLORS.len()];
    let now = Instant::now();
    let Some(document) = app.session.ds().ok().map(|ds| ds.uid) else { return };
    if app.ui.agent.presence.cursor_trail_next.is_some_and(|at| now >= at) && !app.ui.agent.presence.cursor_trail.is_empty() {
        app.ui.agent.presence.target = Some(app.ui.agent.presence.cursor_trail.remove(0));
        app.ui.agent.presence.cursor_trail_next = (!app.ui.agent.presence.cursor_trail.is_empty()).then_some(now + Duration::from_millis(85));
    }
    let presence = &app.ui.agent.presence;
    if presence.document != Some(document) || presence.conversation != app.ui.agent.runtime_conversation {
        return;
    }
    let active = app.ui.agent.busy && presence.active;
    let idle_age = presence.updated.map_or(Duration::MAX, |updated| now.saturating_duration_since(updated));
    let fade = cursor_alpha(active, idle_age);
    let work = presence.point.or_else(|| {
        presence.target.and_then(|id| app.session.ds().ok()?.doc.world_bounds(id)).map(|bounds| {
            let x = bounds.x0 + bounds.width() * 0.35;
            let y = bounds.y0 + bounds.height() * 0.35;
            (x, y)
        })
    });
    let zoom = app.session.ds().map_or(1.0, |ds| ds.view.zoom).max(0.01);
    let destination = work.or(presence.destination);
    let target = destination.or(presence.shown).or_else(|| {
        active
            .then(|| {
                app.session
                    .ds()
                    .ok()
                    .map(|ds| ds.view.screen_to_page(f64::from(app.canvas.rect.width()) * 0.5, f64::from(app.canvas.rect.height()) * 0.5))
            })
            .flatten()
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
    let border_fade = if active {
        1.0
    } else {
        app.ui.agent.presence.updated.map_or(0.0, |updated| (1.0 - now.saturating_duration_since(updated).as_secs_f32() / 0.9).clamp(0.0, 1.0))
    };
    app.ui.agent.presence.shimmers.retain(|_| border_fade > 0.0);
    for shimmer in &app.ui.agent.presence.shimmers {
        let Some(bounds) = app.session.ds().ok().and_then(|ds| ds.doc.world_bounds(shimmer.id)) else { continue };
        let rect = Rect::from_two_pos(app.to_screen(bounds.x0, bounds.y0), app.to_screen(bounds.x1, bounds.y1));
        if rect.is_positive() && rect.intersects(app.canvas.rect) {
            let elapsed = now.saturating_duration_since(shimmer.started).as_secs_f32();
            let clipped = painter.with_clip_rect(app.canvas.rect);
            draw_frame_border(&clipped, rect, brand, elapsed, border_fade, reduce_motion);
        }
    }
    app.ui.agent.presence.entrances.retain(|entrance| now.saturating_duration_since(entrance.started) < Duration::from_millis(450));
    for entrance in &app.ui.agent.presence.entrances {
        let Some(bounds) = app.session.ds().ok().and_then(|ds| ds.doc.world_bounds(entrance.id)) else { continue };
        let rect = Rect::from_two_pos(app.to_screen(bounds.x0, bounds.y0), app.to_screen(bounds.x1, bounds.y1));
        if !rect.is_positive() || !rect.intersects(app.canvas.rect) {
            continue;
        }
        if reduce_motion {
            continue;
        }
        let t = (now.saturating_duration_since(entrance.started).as_secs_f32() / 0.45).clamp(0.0, 1.0);
        let fade = (1.0 - t).powi(2);
        let background = entrance_background(app, entrance.id, Tokens::get(ctx).canvas);
        painter.with_clip_rect(app.canvas.rect).rect_filled(rect, CornerRadius::ZERO, background.gamma_multiply(fade * 0.92));
    }
    if app.ui.agent.presence.draft.as_ref().is_some_and(|draft| now.saturating_duration_since(draft.updated) > Duration::from_secs(20) || !active) {
        app.ui.agent.presence.draft = None;
    }
    let draft_visible = draw_draft(app, painter, now, brand, reduce_motion);
    if let Some((x, y)) = shown.filter(|_| fade > 0.0) {
        let real_tip = app.to_screen(x, y) + idle_offset(idle_age, idle_strength);
        if app.canvas.rect.contains(real_tip) {
            let name = app
                .ui
                .agent
                .runtime_conversation
                .and_then(|id| app.ui.agent.conversations.iter().find(|chat| chat.id == id))
                .map_or(crate::i18n::tr("Agent"), |chat| crate::agent_panel::agent_display_name(chat, agent_index));
            let tag_size = painter.layout_no_wrap(name.to_owned(), crate::theme::semibold(11.0), Color32::WHITE).size() + vec2(12.0, 6.0);
            if let Some((tip, bounds)) = cursor_placement(real_tip, tag_size, app.canvas.rect, occupied) {
                if tip != real_tip {
                    painter.line_segment([real_tip, tip], Stroke::new(1.0, brand.gamma_multiply(fade * 0.6)));
                }
                if let Some(texture) = app.canvas.cursor_textures.multiplayer(ctx, agent_index) {
                    painter.image(
                        texture.id(),
                        Rect::from_min_size(tip - vec2(4.5, 3.5), vec2(23.125, 23.125)),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE.gamma_multiply(fade),
                    );
                }
                crate::live::name_tag(painter, tip + vec2(14.0, 15.0), Align2::LEFT_TOP, name, brand.gamma_multiply(fade));
                occupied.extend(bounds);
            }
        }
    }
    let cursor_visible = shown.is_some_and(|(x, y)| app.canvas.rect.contains(app.to_screen(x, y)));
    if moving
        || draft_visible
        || !app.ui.agent.presence.shimmers.is_empty()
        || !app.ui.agent.presence.pulses.is_empty()
        || !app.ui.agent.presence.entrances.is_empty()
        || (fade > 0.0 && cursor_visible)
    {
        ctx.request_repaint_after(Duration::from_millis(if reduce_motion {
            100
        } else if moving || !app.ui.agent.presence.shimmers.is_empty() {
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
    fn frame_border_animation_stays_on_the_perimeter_and_wraps() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 60.0));
        for (distance, expected) in [
            (0.0, pos2(10.0, 20.0)),
            (100.0, pos2(110.0, 20.0)),
            (160.0, pos2(110.0, 80.0)),
            (260.0, pos2(10.0, 80.0)),
            (320.0, pos2(10.0, 20.0)),
            (340.0, pos2(30.0, 20.0)),
        ] {
            assert_eq!(frame_border_point(rect, distance), expected);
        }
    }

    #[test]
    fn frame_border_fade_preserves_the_agents_hue() {
        let brand = Color32::from_rgb(151, 71, 255);
        let faded = border_color_with_opacity(brand, 0.4);
        let [red, green, blue, alpha] = faded.to_srgba_unmultiplied();
        assert_eq!(alpha, 102);
        assert!((i16::from(red) - i16::from(brand.r())).abs() <= 2);
        assert!((i16::from(green) - i16::from(brand.g())).abs() <= 2);
        assert!((i16::from(blue) - i16::from(brand.b())).abs() <= 2);
    }

    #[test]
    fn colocated_agent_cursors_have_separate_pointers_and_names() {
        let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0));
        let tip = pos2(450.0, 300.0);
        let mut occupied = Vec::new();
        for _ in 0..7 {
            let (_, bounds) = cursor_placement(tip, vec2(54.0, 22.0), canvas, &occupied).unwrap();
            for rect in &bounds {
                assert!(occupied.iter().all(|previous| !rect.intersects(*previous)));
            }
            occupied.extend(bounds);
        }
        assert_eq!(occupied.len(), 14);
    }

    #[test]
    fn cursor_collision_avoids_the_canvas_edge() {
        let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(250.0, 180.0));
        let first = cursor_placement(pos2(218.0, 120.0), vec2(52.0, 22.0), canvas, &[]).unwrap();
        let second = cursor_placement(pos2(218.0, 120.0), vec2(52.0, 22.0), canvas, &first.1).unwrap();
        assert_ne!(first.0, second.0);
        assert!(first.1.into_iter().chain(second.1).all(|rect| canvas.contains(rect.min) && canvas.contains(rect.max)));
    }

    #[test]
    fn atomic_batch_keeps_entry_animation_and_a_short_cursor_path() {
        let mut presence = crate::agent_panel::AgentPresence::default();
        let ids: Vec<_> = (1..=24).map(NodeId).collect();
        schedule_batch_reveal(&mut presence, &ids);
        assert_eq!(presence.entrances.len(), 24);
        assert_eq!(presence.entrances[0].id, NodeId(1));
        assert!(presence.entrances[0].started < presence.entrances[1].started);
        assert_eq!(presence.cursor_trail.first(), Some(&NodeId(1)));
        assert_eq!(presence.cursor_trail.last(), Some(&NodeId(24)));
        assert!(presence.cursor_trail.len() <= 9);
        assert!(presence.cursor_trail_next.is_some());
    }

    #[test]
    fn incomplete_tool_json_updates_the_canvas_draft_field_by_field() {
        let mut presence = crate::agent_panel::AgentPresence::default();
        update_draft(&mut presence, "call-1", &json!("{\"type\":\"rectangle\","));
        assert_eq!(presence.draft.as_ref().map(|draft| &draft.args), Some(&json!({"type":"rectangle"})));
        update_draft(&mut presence, "call-1", &json!("\"x\":24,\"y\":"));
        assert_eq!(presence.draft.as_ref().map(|draft| &draft.args), Some(&json!({"type":"rectangle","x":24})));
        update_draft(&mut presence, "call-1", &json!("32,\"width\":180,\"height\":90}"));
        assert_eq!(presence.draft.as_ref().map(|draft| &draft.args), Some(&json!({"type":"rectangle","x":24,"y":32,"width":180,"height":90})));
        update_draft(&mut presence, "call-2", &json!({"type":"frame"}));
        update_draft(&mut presence, "call-2", &json!({"width":240}));
        assert_eq!(presence.draft.as_ref().map(|draft| &draft.args), Some(&json!({"type":"frame","width":240})));
    }

    #[test]
    fn draft_geometry_uses_the_parent_and_does_not_create_a_layer() {
        let mut session = Session::new();
        let parent =
            NodeId(session.execute("node.create", &json!({"type":"frame","x":140,"y":90,"width":320,"height":240})).unwrap()["id"].as_u64().unwrap());
        let mut ui = Ui::new(session);
        ui.frame(vec![]);
        let count = ui.app.session.doc().unwrap().node(parent).unwrap().children.len();
        let args = json!({"type":"rectangle","parent":parent,"x":48,"y":32,"width":100,"height":60});
        let rect = draft_rect(&ui.app, &args).unwrap();
        assert_eq!(rect.min, ui.app.to_screen(188.0, 122.0));
        assert_eq!(ui.app.session.doc().unwrap().node(parent).unwrap().children.len(), count);
    }

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
        assert_eq!(page, (252.0, 174.0));
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
        assert_eq!(first, (180.0, 120.0));
        ui.app.ui.agent.presence.point = Some((380.0, 220.0));
        ui.app.ui.agent.presence.updated = Some(Instant::now());
        ui.frame(vec![]);
        assert_eq!(ui.app.ui.agent.presence.destination, Some((380.0, 220.0)), "the destination follows the current work");
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
        assert!((first.0 - from.0).hypot(first.1 - from.1) <= 2_400.0 * 0.033 + 0.01);
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
    fn streamed_batch_moves_cursor_to_each_created_layer() {
        let mut ui = Ui::new(Session::new());
        let uid = ui.app.session.ds().unwrap().uid;
        ui.app.ui.agent.busy = true;
        ui.app.ui.agent.runtime_document = Some(uid);
        ui.app.ui.agent.runtime_conversation = Some(7);
        ui.app.ui.agent.presence.document = Some(uid);
        ui.app.ui.agent.presence.conversation = Some(7);
        ui.app.ui.agent.presence.active = true;
        let batch =
            json!({"__agent_document":uid,"commands":[{"command":"node.create","params":{"type":"frame","x":100,"y":100,"width":200,"height":100}}]});
        let started = crate::control::handle(&mut ui.app, &ui.ctx, "agent.batch.begin", &batch).unwrap();
        assert_eq!(started["accepted"], true);
        crate::control::handle(&mut ui.app, &ui.ctx, "agent.batch.step", &json!({"__agent_document":uid})).unwrap();
        let id = ui.app.ui.agent.presence.target.unwrap();
        assert!(ui.app.session.doc().unwrap().node(id).unwrap().kind.is_frame_like());
        assert!(ui.app.ui.agent.presence.point.is_none());
        crate::control::handle(&mut ui.app, &ui.ctx, "agent.batch.finish", &json!({"__agent_document":uid})).unwrap();
    }

    #[test]
    fn inspecting_the_page_does_not_move_cursor_away_from_last_edit() {
        let mut session = Session::new();
        let frame = NodeId(
            session.execute("node.create", &json!({"type":"frame","x":100,"y":100,"width":320,"height":240})).unwrap()["id"].as_u64().unwrap(),
        );
        let child = NodeId(
            session.execute("node.create", &json!({"type":"rectangle","parent":frame,"x":250,"y":180,"width":40,"height":20})).unwrap()["id"]
                .as_u64()
                .unwrap(),
        );
        let mut ui = Ui::new(session);
        let uid = ui.app.session.ds().unwrap().uid;
        ui.app.ui.agent.busy = true;
        ui.app.ui.agent.runtime_document = Some(uid);
        ui.app.ui.agent.runtime_conversation = Some(7);
        ui.app.ui.agent.presence.document = Some(uid);
        ui.app.ui.agent.presence.conversation = Some(7);
        ui.app.ui.agent.presence.active = true;
        ui.app.ui.agent.presence.target = Some(child);
        crate::control::handle(
            &mut ui.app,
            &ui.ctx,
            "agent.activity",
            &json!({"__agent_document":uid,"phase":"done","tool":"inspect_document","ids":[frame.0]}),
        )
        .unwrap();
        assert_eq!(ui.app.ui.agent.presence.target, Some(child));
    }
}
