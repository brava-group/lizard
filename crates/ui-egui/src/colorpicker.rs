//! The colour picker popover: paint type, saturation/brightness square, hue and alpha sliders,
//! hex and RGB fields, gradient stops, image and pattern fills and document colours.

use egui::{Align2, Color32, CornerRadius, Id, Mesh, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use lizard_color::{
    Color, ColorStop, Gradient, GradientKind, Hsb, ImageFilters, ImageScaleMode, Paint, PaintKind, PatternAlign, PatternPaint, PatternTile,
};
use lizard_doc::{NodeId, NodeKind};
use serde_json::json;

use crate::LizardApp;
use crate::a11y::Tip;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme::{Tokens, doc_c32, regular, semibold};
use crate::widgets;

#[path = "colorpicker_custom.rs"]
mod custom;
#[path = "colorpicker_library.rs"]
mod library_tab;

#[cfg(feature = "catalog")]
pub fn open_library_preview(ctx: &egui::Context) {
    library_tab::activate(ctx);
}

#[cfg(feature = "catalog")]
pub fn close_library_preview(ctx: &egui::Context) {
    library_tab::deactivate(ctx);
}

/// Reuse the production solid-color picker for palette editors without a document target.
#[cfg(feature = "catalog")]
pub fn show_standalone_solid(ctx: &egui::Context, color: Color, anchor: Pos2, open: &mut bool) -> Option<Color> {
    custom::show_standalone_solid(ctx, color, anchor, open)
}

pub(crate) fn open_creation_from_eyedropper(ctx: &egui::Context, color: Color) {
    library_tab::create_variable(ctx, color);
}

fn eyedrop_params(target: &PickerTarget, stop: Option<usize>) -> serde_json::Value {
    use lizard_engine::selection_colors::PaintTarget;
    let targets = match target {
        PickerTarget::Paint { fills, index, ids } => {
            ids.iter().map(|id| PaintTarget { id: *id, fills: *fills, index: *index, range: None }).collect()
        }
        PickerTarget::SelectionColor { targets, .. } => targets.clone(),
        PickerTarget::TextRange { id, start, end } => vec![PaintTarget { id: *id, fills: true, index: 0, range: Some((*start, *end)) }],
        PickerTarget::PageBackground => return json!({"pageBackground": true}),
        _ => return json!({}),
    };
    json!({"targets": targets, "stop": stop})
}

#[cfg(test)]
mod eyedrop_params_tests {
    use super::{PickerTarget, eyedrop_params};
    use lizard_doc::NodeId;
    use serde_json::json;

    #[test]
    fn stroke_picker_passes_stroke_target_to_eyedropper() {
        let target = PickerTarget::Paint { fills: false, index: 2, ids: vec![NodeId(7)] };
        let params = eyedrop_params(&target, None);
        assert_eq!(params["targets"], json!([{"id": 7, "fills": false, "index": 2, "range": null}]));
    }
}

pub(crate) fn show_standalone_creation(app: &mut LizardApp, ctx: &egui::Context) {
    library_tab::show_standalone(app, ctx);
}
#[path = "colorpicker_gradient.rs"]
mod gradient;
#[path = "colorpicker_image.rs"]
mod image_tab;
#[path = "colorpicker_pattern.rs"]
mod pattern;
#[path = "colorpicker_shader.rs"]
mod shader_tab;

#[derive(Clone, Debug, PartialEq)]
pub enum PickerTarget {
    PageBackground,
    SelectionColor {
        targets: Vec<lizard_engine::selection_colors::PaintTarget>,
        selection: Vec<NodeId>,
        document: u64,
    },
    Paint {
        fills: bool,
        index: usize,
        ids: Vec<NodeId>,
    },
    /// A fill on the selected faces of a vector being edited.
    VectorRegion {
        id: NodeId,
        regions: Vec<usize>,
        index: usize,
    },
    Effect {
        ids: Vec<NodeId>,
        index: usize,
        /// A duo noise's second colour.
        secondary: bool,
    },
    /// The fill of a character range of a text layer being edited.
    TextRange {
        id: NodeId,
        start: usize,
        end: usize,
    },
    /// A colour variable's value in one mode.
    Variable {
        id: String,
        mode: String,
    },
    /// One paint of a colour style (edited from the style picker).
    Style {
        id: String,
        index: usize,
    },
    /// A paint in the unsaved style being composed beside the colour library.
    DraftStyle {
        index: usize,
    },
    /// The colour variable being composed beside the colour library.
    DraftVariable,
    /// The colour of one effect in an effect style.
    StyleEffect {
        id: String,
        index: usize,
        secondary: bool,
    },
    /// A colour field of a plugin's panel or parameters form. A drag only shows `pending`; the
    /// plugin hears of the colour when the drag ends.
    PluginField {
        plugin: String,
        control: String,
        pending: Option<String>,
    },
}

/// The paints of colour style `id`.
fn style_paints(doc: &lizard_doc::Document, id: &str) -> Option<Vec<Paint>> {
    match &doc.style(id)?.kind {
        lizard_doc::StyleKind::Paint { paints } => Some(paints.clone()),
        _ => None,
    }
}

/// The fills a text range paints with (its own, else the layer's).
pub fn range_fills(doc: &lizard_doc::Document, id: NodeId, start: usize) -> Option<Vec<Paint>> {
    let n = doc.node(id)?;
    let t = n.kind.text()?;
    Some(t.style_at(start).fills.clone().unwrap_or_else(|| n.fills.clone()))
}

fn current_paint(app: &LizardApp, target: &PickerTarget) -> Option<Paint> {
    let doc = app.session.doc().ok()?;
    match target {
        PickerTarget::SelectionColor { targets, selection, document } => {
            let ds = app.session.ds().ok()?;
            if ds.uid != *document || ds.selection != *selection {
                return None;
            }
            targets.first()?.paint(doc)
        }
        PickerTarget::PageBackground => match &doc.node(app.session.page().ok()?)?.kind {
            NodeKind::Page { background, .. } => Some(Paint::solid(*background)),
            _ => None,
        },
        PickerTarget::Paint { fills, index, ids } => {
            let n = doc.node(*ids.first()?)?;
            let list = if *fills { &n.fills } else { &n.strokes };
            list.get(*index).cloned()
        }
        PickerTarget::VectorRegion { id, regions, index } => {
            let n = doc.node(*id)?;
            let NodeKind::Vector { network } = &n.kind else { return None };
            network.regions.get(*regions.first()?)?.fills.as_ref().unwrap_or(&n.fills).get(*index).cloned()
        }
        PickerTarget::Effect { ids, index, secondary } => doc.node(*ids.first()?)?.effects.get(*index)?.color(*secondary).map(Paint::solid),
        PickerTarget::TextRange { id, start, .. } => range_fills(doc, *id, *start)?.into_iter().next(),
        PickerTarget::Variable { id, mode } => match doc.variables.get(id)?.values.get(mode)? {
            lizard_doc::VariableValue::Color(c) => {
                let mut p = Paint::solid(c.with_alpha(1.0));
                p.opacity = c.a;
                Some(p)
            }
            _ => None,
        },
        PickerTarget::Style { id, index } => style_paints(doc, id)?.get(*index).cloned(),
        PickerTarget::DraftStyle { index } => app.ui_context.as_ref().and_then(|ctx| library_tab::draft_paint(ctx, *index)),
        PickerTarget::DraftVariable => app.ui_context.as_ref().and_then(library_tab::draft_variable_paint),
        PickerTarget::StyleEffect { id, index, secondary } => match &doc.style(id)?.kind {
            lizard_doc::StyleKind::Effect { effects } => effects.get(*index)?.color(*secondary).map(Paint::solid),
            _ => None,
        },
        PickerTarget::PluginField { plugin, control, pending } => {
            let hex = match pending {
                Some(hex) => hex.clone(),
                None => crate::plugins_ui::field_value(app, plugin, control)?,
            };
            let c = Color::from_hex(&hex)?;
            let mut p = Paint::solid(c.with_alpha(1.0));
            p.opacity = c.a;
            Some(p)
        }
    }
}

/// Write `paint` to the target. `live` edits are part of an ongoing drag.
fn write(app: &mut LizardApp, target: &PickerTarget, paint: &Paint, live: bool) {
    if let PickerTarget::DraftStyle { index } = target {
        if let Some(ctx) = &app.ui_context {
            library_tab::set_draft_paint(ctx, *index, paint.clone());
        }
        if !live && let Some(color) = paint.solid_color() {
            remember_recent(app, color);
        }
        return;
    }
    if matches!(target, PickerTarget::DraftVariable) {
        if let Some(ctx) = &app.ui_context {
            library_tab::set_draft_variable_paint(ctx, paint);
        }
        if !live && let Some(color) = paint.solid_color() {
            remember_recent(app, color);
        }
        return;
    }
    if let PickerTarget::PluginField { plugin, control, .. } = target {
        let hex = paint.solid_color().unwrap_or(Color::BLACK).to_css_hex();
        let (plugin, control) = (plugin.clone(), control.clone());
        if !live {
            crate::plugins_ui::set_field(app, &plugin, &control, &hex);
        }
        app.ui.picker = Some(PickerTarget::PluginField { plugin, control, pending: live.then_some(hex) });
        return;
    }
    if live {
        let _ = app.session.begin_interaction("Color");
    }
    let r = match target {
        PickerTarget::SelectionColor { targets, .. } => app.session.execute("selection.setPaint", &json!({"targets": targets, "paint": paint})),
        PickerTarget::PageBackground => {
            let c = paint.solid_color().unwrap_or(Color::DEFAULT_PAGE);
            app.session.execute("page.setBackground", &json!({"color": c.to_css_hex()}))
        }
        PickerTarget::Paint { fills, index, ids } => {
            app.session.execute(if *fills { "fill.set" } else { "stroke.set" }, &json!({"ids": ids, "index": index, "paint": paint}))
        }
        PickerTarget::VectorRegion { id, regions, index } => {
            app.session.execute("vector.region.fill.set", &json!({"id": id, "regions": regions, "index": index, "paint": paint}))
        }
        PickerTarget::Effect { ids, index, secondary } => {
            let Some(mut e) = ids.first().and_then(|i| app.session.doc().ok()?.node(*i)?.effects.get(*index).cloned()) else { return };
            if let Some(c) = e.color_mut(*secondary) {
                *c = paint.solid_color().unwrap_or(Color::BLACK);
            }
            app.session.execute("effect.set", &json!({"ids": ids, "index": index, "effect": e}))
        }
        PickerTarget::TextRange { id, start, end } => {
            app.session.execute("text.style", &json!({"id": id, "start": start, "end": end, "textFills": [paint]}))
        }
        PickerTarget::Variable { id, mode } => {
            let c = paint.solid_color().unwrap_or(Color::BLACK);
            app.session.execute("variable.set", &json!({"variable": id, "mode": mode, "value": c.to_css_hex()}))
        }
        PickerTarget::PluginField { .. } => return,
        PickerTarget::Style { id, index } => {
            let Some(mut paints) = app.session.doc().ok().and_then(|d| style_paints(d, id)) else { return };
            if let Some(slot) = paints.get_mut(*index) {
                *slot = paint.clone();
            }
            app.session.execute("style.update", &json!({"style": id, "value": paints}))
        }
        PickerTarget::DraftStyle { .. } | PickerTarget::DraftVariable => return,
        PickerTarget::StyleEffect { id, index, secondary } => {
            let Some(mut effects) = app.session.doc().ok().and_then(|doc| match &doc.style(id)?.kind {
                lizard_doc::StyleKind::Effect { effects } => Some(effects.clone()),
                _ => None,
            }) else {
                return;
            };
            if let Some(color) = effects.get_mut(*index).and_then(|effect| effect.color_mut(*secondary)) {
                *color = paint.solid_color().unwrap_or(Color::BLACK);
            }
            app.session.execute("style.update", &json!({"style": id, "value": effects}))
        }
    };
    if let Err(e) = r {
        app.toast(e.to_string());
    }
    // Drag previews (`live`) are not choices: a drag records only the colour it ends on. Not "an
    // interaction is open": editing text holds one, and its colour clicks are choices.
    if !live && let Some(c) = paint.solid_color() {
        remember_recent(app, c);
    }
}

fn remember_recent(app: &mut LizardApp, c: Color) {
    // By 8-bit value: a document stores colours rounded to 8 bits, so the colour picked and the one read
    // back differ by a fraction and would show as two identical swatches.
    app.session.recent_colors.retain(|x| x.to_rgba8() != c.to_rgba8());
    app.session.recent_colors.insert(0, c);
    app.session.recent_colors.truncate(16);
}

fn finish(app: &mut LizardApp) {
    if let Some(PickerTarget::PluginField { plugin, control, pending: Some(hex) }) = app.ui.picker.clone() {
        crate::plugins_ui::set_field(app, &plugin, &control, &hex);
        app.ui.picker = Some(PickerTarget::PluginField { plugin, control, pending: None });
    }
    if app.session.ds().map(|d| d.interaction.is_some()).unwrap_or(false) {
        if let Some(t) = app.ui.picker.clone()
            && !matches!(t, PickerTarget::PluginField { .. })
            && let Some(c) = current_paint(app, &t).and_then(|p| p.solid_color())
        {
            remember_recent(app, c);
        }
        let _ = app.session.commit_interaction();
    }
}

pub(crate) fn dismiss(app: &mut LizardApp, ctx: &egui::Context) {
    if app.ui.picker.is_some() {
        finish(app);
        app.ui.picker = None;
        ctx.data_mut(|data| data.remove::<usize>(Id::new("cp_gradient_stop_popup")));
        library_tab::deactivate(ctx);
        shader_tab::deactivate(ctx);
    }
}

/// The selected gradient stop in the picker (shared with the on-canvas stops).
pub fn stop_id() -> Id {
    Id::new("cp_stop")
}

/// Apply a swatch colour: a solid paint's colour, or the selected gradient stop's.
fn set_color(app: &mut LizardApp, target: &PickerTarget, paint: &mut Paint, c: Color, stop: Id, ui: &Ui) {
    match &mut paint.kind {
        PaintKind::Solid { color } => {
            *color = c.with_alpha(1.0);
            paint.opacity = c.a;
        }
        PaintKind::Gradient(g) => {
            let sel: usize = ui.data(|d| d.get_temp(stop)).unwrap_or(0);
            if let Some(s) = g.stops.get_mut(sel) {
                s.color = c;
            }
        }
        PaintKind::Image(_) | PaintKind::Pattern(_) | PaintKind::Shader(_) => return,
    }
    write(app, target, paint, false);
}

/// The page's most used solid colours, recomputed only when the document snapshot changes.
fn page_colors(app: &LizardApp, ui: &Ui) -> Vec<Color> {
    let Ok(ds) = app.session.ds() else { return vec![] };
    let key = (std::sync::Arc::as_ptr(&ds.doc) as usize, ds.page);
    let cache_id = Id::new("cp_doc_colors");
    if let Some((k, colors)) = ui.data(|d| d.get_temp::<((usize, lizard_doc::NodeId), Vec<Color>)>(cache_id))
        && k == key
    {
        return colors;
    }
    let colors: Vec<Color> = lizard_engine::cmd::styles::document_colors(&ds.doc, ds.page).into_iter().take(21).map(|(c, _)| c).collect();
    ui.data_mut(|d| d.insert_temp(cache_id, (key, colors.clone())));
    colors
}

/// Colour models the value row can show.
const MODELS: [&str; 5] = ["Hex", "RGB", "HSL", "HSB", "CSS"];

fn hue_color(ctx: &egui::Context, h: f32) -> Color32 {
    doc_c32(ctx, Hsb { h, s: 1.0, b: 1.0 }.to_color())
}

/// Mask a gradient mesh with four smooth quarter-circle cutouts.
fn mask_round_corners(ui: &Ui, rect: Rect, radius: f32) {
    let painter = ui.painter();
    let background = Tokens::get(ui.ctx()).panel;
    let feather = 1.0 / ui.ctx().pixels_per_point();
    let steps = (radius * ui.ctx().pixels_per_point() * 3.0).ceil().max(24.0) as usize;
    let corners = [
        (rect.left_top(), pos2(rect.left() + radius, rect.top() + radius), std::f32::consts::PI, 1.5 * std::f32::consts::PI),
        (rect.right_top(), pos2(rect.right() - radius, rect.top() + radius), 1.5 * std::f32::consts::PI, 2.0 * std::f32::consts::PI),
        (rect.right_bottom(), pos2(rect.right() - radius, rect.bottom() - radius), 0.0, 0.5 * std::f32::consts::PI),
        (rect.left_bottom(), pos2(rect.left() + radius, rect.bottom() - radius), 0.5 * std::f32::consts::PI, std::f32::consts::PI),
    ];
    for (outer, center, start, end) in corners {
        let arcs: Vec<(Pos2, Pos2)> = (0..=steps)
            .map(|i| {
                let angle = start + (end - start) * i as f32 / steps as f32;
                let direction = vec2(angle.cos(), angle.sin());
                (center + direction * (radius + feather / 2.0), center + direction * (radius - feather / 2.0))
            })
            .collect();
        let mut mesh = Mesh::default();
        mesh.colored_vertex(outer, background);
        for (outside, _) in &arcs {
            mesh.colored_vertex(*outside, background);
        }
        for i in 1..=steps {
            mesh.add_triangle(0, i as u32, i as u32 + 1);
        }
        let transparent = Color32::from_rgba_unmultiplied(background.r(), background.g(), background.b(), 0);
        let fringe_start = mesh.vertices.len() as u32;
        for (outside, inside) in &arcs {
            mesh.colored_vertex(*outside, background);
            mesh.colored_vertex(*inside, transparent);
        }
        for i in 0..steps as u32 {
            let a = fringe_start + i * 2;
            mesh.add_triangle(a, a + 1, a + 2);
            mesh.add_triangle(a + 1, a + 3, a + 2);
        }
        painter.add(mesh);
    }
}

fn sv_square(ui: &mut Ui, rect: Rect, hsb: &mut Hsb) -> (bool, bool) {
    // White → hue horizontally, then transparent → black vertically.
    let mut m = Mesh::default();
    let hc = hue_color(ui.ctx(), hsb.h);
    let corners = [(rect.left_top(), Color32::WHITE), (rect.right_top(), hc), (rect.right_bottom(), hc), (rect.left_bottom(), Color32::WHITE)];
    for (p, c) in corners {
        m.colored_vertex(p, c);
    }
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    ui.painter().add(m);
    let mut m2 = Mesh::default();
    for (p, c) in [
        (rect.left_top(), Color32::TRANSPARENT),
        (rect.right_top(), Color32::TRANSPARENT),
        (rect.right_bottom(), Color32::BLACK),
        (rect.left_bottom(), Color32::BLACK),
    ] {
        m2.colored_vertex(p, c);
    }
    m2.add_triangle(0, 1, 2);
    m2.add_triangle(0, 2, 3);
    ui.painter().add(m2);
    mask_round_corners(ui, rect, 5.0);
    let resp = ui.interact(rect, Id::new("cp_sv"), Sense::click_and_drag());
    let mut changed = false;
    if (resp.dragged() || resp.clicked() || resp.drag_started())
        && let Some(p) = resp.interact_pointer_pos()
    {
        hsb.s = ((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
        hsb.b = 1.0 - ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
        changed = true;
    }
    let knob = pos2(rect.min.x + hsb.s * rect.width(), rect.min.y + (1.0 - hsb.b) * rect.height());
    ui.painter().circle_stroke(knob, 6.0, Stroke::new(2.0, Color32::WHITE));
    ui.painter().circle_stroke(knob, 7.0, Stroke::new(1.0, Color32::from_black_alpha(80)));
    (changed, resp.drag_stopped() || resp.clicked())
}

fn hue_slider(ui: &mut Ui, rect: Rect, hsb: &mut Hsb) -> (bool, bool) {
    let n = 12;
    let mut m = Mesh::default();
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = rect.min.x + t * rect.width();
        let c = hue_color(ui.ctx(), t * 360.0);
        m.colored_vertex(pos2(x, rect.min.y), c);
        m.colored_vertex(pos2(x, rect.max.y), c);
        if i > 0 {
            let k = (i * 2) as u32;
            m.add_triangle(k - 2, k - 1, k);
            m.add_triangle(k - 1, k, k + 1);
        }
    }
    ui.painter().add(m);
    mask_round_corners(ui, rect, rect.height() / 2.0);
    slider_knob(ui, rect, hsb.h / 360.0, Id::new("cp_hue"), hue_color(ui.ctx(), hsb.h))
        .map(|(t, done)| {
            hsb.h = (t * 360.0).clamp(0.0, 359.9);
            (true, done)
        })
        .unwrap_or((false, false))
}

fn slider_knob(ui: &mut Ui, rect: Rect, t: f32, id: Id, color: Color32) -> Option<(f32, bool)> {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let travel = (rect.width() - rect.height()).max(1.0);
    let x = rect.min.x + rect.height() / 2.0 + t.clamp(0.0, 1.0) * travel;
    let center = pos2(x, rect.center().y);
    ui.painter().circle_filled(center, rect.height() / 2.0, Color32::WHITE);
    ui.painter().circle_filled(center, rect.height() / 4.0, color);
    ui.painter().circle_stroke(center, rect.height() / 2.0, Stroke::new(1.0, Color32::from_black_alpha(52)));
    if (resp.dragged() || resp.clicked() || resp.drag_started())
        && let Some(p) = resp.interact_pointer_pos()
    {
        return Some((((p.x - rect.min.x - rect.height() / 2.0) / travel).clamp(0.0, 1.0), resp.drag_stopped() || resp.clicked()));
    }
    if resp.drag_stopped() {
        return Some((t, true));
    }
    None
}

fn alpha_slider(ui: &mut Ui, rect: Rect, color: Color) -> Option<(f32, bool)> {
    widgets::checker(&ui.painter().with_clip_rect(rect), rect, rect.height() / 2.0);
    let mut m = Mesh::default();
    let opaque = doc_c32(ui.ctx(), color.with_alpha(1.0));
    for (p, c) in [
        (rect.left_top(), Color32::TRANSPARENT),
        (rect.right_top(), opaque),
        (rect.right_bottom(), opaque),
        (rect.left_bottom(), Color32::TRANSPARENT),
    ] {
        m.colored_vertex(p, c);
    }
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    ui.painter().add(m);
    mask_round_corners(ui, rect, rect.height() / 2.0);
    slider_knob(ui, rect, color.a, Id::new("cp_alpha"), opaque)
}

/// Keep a style's colour picker beside its editor. Other targets retain their usual position.
fn initial_position(app: &LizardApp, ctx: &egui::Context, target: &PickerTarget) -> Pos2 {
    let beside = match target {
        PickerTarget::Style { .. } => {
            ctx.memory(|memory| memory.area_rect(Id::new("color_style_editor_area"))).map(|rect| pos2(rect.left() - 248.0, rect.top()))
        }
        PickerTarget::DraftStyle { .. } | PickerTarget::DraftVariable => {
            ctx.memory(|memory| memory.area_rect(Id::new("color_library_create"))).map(|rect| pos2(rect.left() - 248.0, rect.top()))
        }
        PickerTarget::StyleEffect { id, .. } => ctx
            .memory(|memory| {
                memory.area_rect(Id::new(("effect_style_settings", id))).or_else(|| memory.area_rect(Id::new("effect_style_editor_area")))
            })
            .map(|rect| pos2(rect.left() - 248.0, rect.top())),
        PickerTarget::PluginField { plugin, .. } => {
            crate::plugins_ui::field_window(app, ctx, plugin).map(|rect| pos2(rect.left() - 248.0, rect.top()))
        }
        _ => None,
    };
    beside.unwrap_or_else(|| pos2(ctx.content_rect().right() - app.ui.right_width - 260.0, 140.0))
}

pub fn show(app: &mut LizardApp, ctx: &egui::Context) {
    let Some(target) = app.ui.picker.clone() else {
        if library_tab::active(ctx) {
            library_tab::deactivate(ctx);
        }
        shader_tab::deactivate(ctx);
        return;
    };
    // A layer's picker belongs to the selection it was opened for: once that changes, close it
    // rather than keep editing layers that are no longer selected.
    let stale = match &target {
        PickerTarget::Paint { ids, .. } | PickerTarget::Effect { ids, .. } => *ids != app.session.selection(),
        PickerTarget::VectorRegion { id, regions, .. } => {
            !app.session.selection().contains(id)
                || app.session.ds().ok().and_then(|ds| ds.vector_edit.as_ref()).is_none_or(|ve| ve.id != *id || ve.regions != *regions)
        }
        PickerTarget::TextRange { id, .. } => !app.session.selection().contains(id),
        _ => false,
    };
    let current = if stale { None } else { current_paint(app, &target) };
    let Some(paint) = current else {
        dismiss(app, ctx);
        return;
    };
    if library_tab::active(ctx) {
        library_tab::show(app, ctx, &target, paint);
        return;
    }
    match &paint.kind {
        PaintKind::Solid { .. } => custom::show(app, ctx, &target, paint.clone()),
        PaintKind::Gradient(_) => gradient::show(app, ctx, &target, paint.clone()),
        PaintKind::Pattern(_) => pattern::show(app, ctx, &target, paint.clone()),
        PaintKind::Image(_) => image_tab::show(app, ctx, &target, paint.clone()),
        PaintKind::Shader(_) => shader_tab::show_editor(app, ctx, &target, paint.clone()),
    }
    if shader_tab::active(ctx) {
        shader_tab::show_browser(app, ctx, &target, paint);
    }
}

fn pattern_sources(app: &LizardApp, target: &PickerTarget) -> Vec<(NodeId, String)> {
    let Ok(doc) = app.session.doc() else { return Vec::new() };
    let painted = match target {
        PickerTarget::Paint { ids, .. } => ids.clone(),
        PickerTarget::VectorRegion { id, .. } => vec![*id],
        PickerTarget::SelectionColor { targets, .. } => targets.iter().map(|t| t.id).collect(),
        PickerTarget::TextRange { id, .. } => vec![*id],
        _ => Vec::new(),
    };
    let top = app.session.page().ok().and_then(|p| doc.node(p)).map(|p| p.children.clone()).unwrap_or_default();
    let comps = doc.nodes().filter(|n| matches!(n.kind, NodeKind::Component { .. })).map(|n| n.id);
    let mut out: Vec<(NodeId, String)> = Vec::new();
    for id in comps.chain(top) {
        let related = painted.iter().any(|t| doc.is_ancestor(*t, id) || doc.is_ancestor(id, *t));
        if !related
            && !out.iter().any(|(o, _)| *o == id)
            && let Some(n) = doc.node(id)
        {
            out.push((id, n.name.clone()));
        }
        if out.len() >= 64 {
            break;
        }
    }
    out
}

fn lizard_format_add(doc: &mut lizard_doc::Document, bytes: Vec<u8>) -> Option<String> {
    // Images are keyed by content hash; the engine's image.place uses the same scheme.
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut s = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut s);
        format!("img{:016x}{:08x}", s.finish(), bytes.len())
    };
    doc.images.insert(hash.clone(), lizard_doc::ImageData { bytes: std::sync::Arc::new(bytes), width: w, height: h });
    Some(hash)
}

#[cfg(test)]
mod selection_color_tests {
    use super::*;
    use crate::NoServices;
    use lizard_engine::{Session, selection_colors::collect};

    fn setup() -> (LizardApp, NodeId, NodeId) {
        let mut s = Session::new();
        let a = NodeId(s.execute("shape.rectangle", &json!({"width":40,"height":40,"props":{"fill":"#ff0000"}})).unwrap()["id"].as_u64().unwrap());
        let b = NodeId(s.execute("shape.rectangle", &json!({"width":40,"height":40,"props":{"fill":"#0000ff"}})).unwrap()["id"].as_u64().unwrap());
        s.execute("select.set", &json!({"ids":[a,b]})).unwrap();
        let mut app = LizardApp::new(s, Box::new(NoServices));
        let targets = collect(app.session.doc().unwrap(), &[a, b])[0].targets.clone();
        crate::selection_colors::open_picker(&mut app, targets);
        (app, a, b)
    }

    #[test]
    fn picker_live_changes_keep_frozen_targets_and_commit_once() {
        let (mut app, a, b) = setup();
        let target = app.ui.picker.clone().unwrap();
        let before = app.session.undo_depth();
        for color in ["#0000ff", "#00ff00"] {
            write(&mut app, &target, &Paint::solid(Color::from_hex(color).unwrap()), true);
        }
        assert!(app.session.recent_colors.is_empty(), "previews must not be recorded");
        finish(&mut app);
        assert_eq!(app.session.recent_colors, vec![Color::from_hex("#00ff00").unwrap()]);
        assert_eq!(app.session.undo_depth().0, before.0 + 1);
        assert_eq!(current_paint(&app, &target).unwrap().solid_color().unwrap().to_hex(), "00FF00");
        assert_eq!(app.session.doc().unwrap().node(b).unwrap().fills[0].solid_color().unwrap().to_hex(), "0000FF");
        app.session.undo().unwrap();
        assert_eq!(app.session.doc().unwrap().node(a).unwrap().fills[0].solid_color().unwrap().to_hex(), "FF0000");
    }

    #[test]
    fn writing_a_variable_keeps_its_alpha() {
        // The picker holds a colour variable's alpha as the paint opacity; writing it back applies it once.
        let (mut app, _, _) = setup();
        app.session.execute("variable.createCollection", &json!({"name":"T"})).unwrap();
        let v = app.session.execute("variable.create", &json!({"collection":"T","name":"tint","type":"color","value":"#0D6EFD80"})).unwrap();
        let var = app.session.doc().unwrap().variables.get(v["id"].as_str().unwrap()).unwrap().clone();
        let target = PickerTarget::Variable { id: var.id.clone(), mode: var.values.keys().next().unwrap().clone() };
        for _ in 0..2 {
            let paint = current_paint(&app, &target).unwrap();
            write(&mut app, &target, &paint, false);
        }
        let c = current_paint(&app, &target).unwrap().solid_color().unwrap();
        assert_eq!(c.to_css_hex().to_uppercase(), "#0D6EFD80");
    }

    #[test]
    fn draft_style_picker_updates_the_unsaved_paint_without_document_commands() {
        let mut app = LizardApp::new(Session::new(), Box::new(NoServices));
        let ctx = egui::Context::default();
        app.ui_context = Some(ctx.clone());
        library_tab::create(&ctx, Paint::solid(Color::BLACK));
        let target = PickerTarget::DraftStyle { index: 0 };
        let before = app.session.undo_depth();
        let mut paint = Paint::solid(Color::from_hex("#E32636").unwrap());
        paint.opacity = 0.4;
        write(&mut app, &target, &paint, true);
        let draft = current_paint(&app, &target).unwrap();
        assert_eq!(draft.solid_color().unwrap().to_hex(), "E32636");
        assert_eq!(draft.opacity, 0.4);
        assert_eq!(app.session.undo_depth(), before);
        write(&mut app, &target, &paint, false);
        assert_eq!(app.session.recent_colors.len(), 1);
    }

    #[test]
    fn draft_variable_picker_preserves_color_and_opacity_without_document_commands() {
        let mut app = LizardApp::new(Session::new(), Box::new(NoServices));
        let ctx = egui::Context::default();
        app.ui_context = Some(ctx.clone());
        library_tab::create_variable(&ctx, Color::from_hex("#1E1E1E").unwrap());
        let target = PickerTarget::DraftVariable;
        let before = app.session.undo_depth();
        let mut paint = Paint::solid(Color::from_hex("#E32636").unwrap());
        paint.opacity = 0.4;
        write(&mut app, &target, &paint, true);
        let draft = current_paint(&app, &target).unwrap();
        assert_eq!(draft.solid_color().unwrap().to_hex(), "E32636");
        assert!((draft.opacity - 0.4).abs() < 0.0001);
        assert_eq!(app.session.undo_depth(), before);
        write(&mut app, &target, &paint, false);
        assert_eq!(app.session.recent_colors.len(), 1);
        assert_eq!(app.session.recent_colors[0].to_css_hex(), "#E3263666");
        assert_eq!(app.session.undo_depth(), before);
    }

    #[test]
    fn picker_does_not_follow_selection_or_document_switches() {
        let (mut app, _, _) = setup();
        let target = app.ui.picker.clone().unwrap();
        app.session.execute("select.none", &json!({})).unwrap();
        assert!(current_paint(&app, &target).is_none());
        let (mut app, _, _) = setup();
        let target = app.ui.picker.clone().unwrap();
        app.session.execute("file.new", &json!({})).unwrap();
        assert!(current_paint(&app, &target).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers_drag_tests::Ui;
    use lizard_engine::Session;

    #[test]
    fn dragging_records_only_the_final_colour_in_recent_colors() {
        let mut u = Ui::new(Session::new());
        let target = PickerTarget::PageBackground;
        u.app.ui.picker = Some(target.clone());
        for hex in ["#110000", "#220000", "#330000"] {
            let c = Color::from_hex(hex).unwrap();
            write(&mut u.app, &target, &Paint::solid(c), true);
        }
        assert!(u.app.session.recent_colors.is_empty(), "previews must not be recorded: {:?}", u.app.session.recent_colors);
        finish(&mut u.app);
        let last = Color::from_hex("#330000").unwrap();
        assert_eq!(u.app.session.recent_colors, vec![last]);
    }

    #[test]
    fn a_drag_released_with_a_final_write_records_one_colour() {
        let mut u = Ui::new(Session::new());
        let target = PickerTarget::PageBackground;
        u.app.ui.picker = Some(target.clone());
        // Not a whole 8-bit value: the page stores it rounded, so recording both would add two near-identical swatches.
        let c = Color::new(0.3, 0.0, 0.0, 1.0);
        write(&mut u.app, &target, &Paint::solid(Color::new(0.1, 0.0, 0.0, 1.0)), true);
        write(&mut u.app, &target, &Paint::solid(c), false);
        finish(&mut u.app);
        assert_eq!(u.app.session.recent_colors.len(), 1, "{:?}", u.app.session.recent_colors);
    }

    #[test]
    fn a_click_outside_a_drag_still_records_the_colour() {
        let mut u = Ui::new(Session::new());
        let target = PickerTarget::PageBackground;
        u.app.ui.picker = Some(target.clone());
        let c = Color::from_hex("#445566").unwrap();
        write(&mut u.app, &target, &Paint::solid(c), false);
        assert_eq!(u.app.session.recent_colors, vec![c]);
    }
}
