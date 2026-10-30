//! Compact pattern editor, laid out on the 240 × 522 reference dialog.

use super::custom::{BORDER, FIELD, MUTED, PANEL, WIDTH, at, field, label_tab, paint_kind_button, text_entry, vector_button};
use super::*;

const HEIGHT: f32 = 522.0;
const ACTIVE: Color32 = Color32::from_rgb(0x7c, 0xc4, 0xf8);

fn source_dimensions(app: &FigApp, source: u64) -> (f64, f64) {
    app.session
        .doc()
        .ok()
        .and_then(|doc| doc.world_bounds(NodeId(source)))
        .map_or((100.0, 100.0), |bounds| (bounds.width().max(1.0), bounds.height().max(1.0)))
}

fn preview_texture(app: &FigApp, ctx: &egui::Context, source: u64) -> Option<egui::TextureHandle> {
    let doc = app.session.doc().ok()?;
    let node = doc.node_arc(NodeId(source))?;
    let identity = std::sync::Arc::as_ptr(node) as usize;
    let cache = Id::new("cp_pattern_preview_texture");
    if let Some((old_source, old_identity, texture)) = ctx.data(|data| data.get_temp::<(u64, usize, egui::TextureHandle)>(cache))
        && old_source == source
        && old_identity == identity
    {
        return Some(texture);
    }
    let texture = crate::assets_panel::thumbnail(ctx, doc, NodeId(source), 206.0)?;
    ctx.data_mut(|data| data.insert_temp(cache, (source, identity, texture.clone())));
    Some(texture)
}

fn tile_icon(ui: &Ui, rect: Rect, offset: bool, color: Color32) {
    for row in 0..3 {
        let xs: &[f32] = if offset { if row == 1 { &[1.0, 5.0, 9.0, 13.0] } else { &[3.0, 7.0, 11.0] } } else { &[1.0, 6.0, 11.0] };
        for &x in xs {
            ui.painter().circle_filled(rect.min + vec2(x, row as f32 * 5.0 + 1.0), 1.0, color);
        }
    }
}

fn value_field(
    ui: &mut Ui,
    origin: Pos2,
    y: f32,
    label: Option<&str>,
    icon: Option<&str>,
    id: &'static str,
    value: f64,
    disabled: bool,
) -> Option<f64> {
    if let Some(label) = label {
        ui.painter().text(pos2(origin.x + 16.0, origin.y + y + 16.0), Align2::LEFT_CENTER, label, regular(11.0), MUTED);
    }
    let rect = at(origin, 80.0, y + 4.0, 136.0, 24.0);
    field(ui, rect, CornerRadius::same(5), false);
    if let Some(icon) = icon {
        if icon == "scale" {
            icons::paint_colorpicker(ui.painter(), at(origin, 88.0, y + 10.0, 12.0, 12.0), 15, MUTED);
        } else {
            ui.painter().text(pos2(origin.x + 94.0, origin.y + y + 16.0), Align2::CENTER_CENTER, icon, regular(11.0), MUTED);
        }
    }
    let x = if icon.is_some() { 107.0 } else { 88.0 };
    text_entry(ui, at(origin, x, y + 8.0, 100.0, 16.0), id, format!("{value:.0}%"))
        .and_then(|text| text.trim().trim_end_matches('%').parse::<f64>().ok())
        .filter(|value| value.is_finite() && (!disabled || *value == 0.0))
}

pub(super) fn show(app: &mut FigApp, ctx: &egui::Context, target: &PickerTarget, mut paint: Paint) {
    let PaintKind::Pattern(mut pattern) = paint.kind.clone() else { return };
    let pos = super::initial_position(app, ctx, target);
    let mut open = true;
    let mut close_now = false;
    let mut switched_mode = false;
    crate::primitives::FloatingPanel::new(Id::new("color_picker"), pos)
        .width(crate::primitives::FloatingPanelWidth::Compact)
        .height(crate::primitives::FloatingPanelHeight::Fixed(HEIGHT))
        .fill(PANEL)
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let dialog = Rect::from_min_size(header.min, vec2(WIDTH, HEIGHT));
            ui.allocate_exact_size(vec2(WIDTH, HEIGHT - 40.0), Sense::hover());
            let origin = dialog.min;
            let mut changed = false;
            label_tab(ui, at(origin, 8.0, 8.0, 91.609, 24.0), "cp_custom_tab", "Custom", true);
            if label_tab(ui, at(origin, 103.609, 8.0, 74.914, 24.0), "cp_libraries_tab", "Libraries", false) {
                library_tab::activate(ctx);
            }
            if vector_button(ui, at(origin, 180.0, 8.0, 28.0, 24.0), "cp_new_style", 8, false, "New style").clicked() {
                library_tab::create(ctx, paint.clone());
                library_tab::activate(ctx);
            }
            if vector_button(ui, at(origin, 208.0, 8.0, 24.0, 24.0), "cp_close", 9, false, "Close").clicked() {
                close_now = true;
            }

            for index in 0..6 {
                let Some(response) = paint_kind_button(ui, origin, index, index == 2) else { continue };
                if index == 5 && response.clicked() {
                    if super::shader_tab::supports(target) {
                        super::shader_tab::activate(ctx);
                    } else {
                        app.toast("Shaders are available for fills");
                    }
                    continue;
                }
                if response.clicked() && index != 2 {
                    custom::pick_kind(app, target, &mut paint, index);
                    switched_mode = true;
                    return;
                }
            }
            let blend_rect = at(origin, 208.0, 48.0, 24.0, 24.0);
            let blend = ui.interact(blend_rect, Id::new("cp_pattern_blend_menu"), Sense::click()).tip("Blend mode");
            ui.painter().rect_filled(blend_rect, CornerRadius::same(5), if blend.hovered() { FIELD } else { Color32::TRANSPARENT });
            custom::paint_blend_icon(ui, blend_rect);
            crate::right::menu_popup(&blend).show(|ui| {
                if let Some(mode) = crate::right::blend_menu(ui, false) {
                    paint.blend = mode;
                    write(app, target, &paint, false);
                }
            });
            ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 79.5, Stroke::new(1.0, BORDER));

            let preview = at(origin, 16.0, 88.0, 208.0, 208.0);
            ui.painter().rect_filled(preview, CornerRadius::same(5), Color32::from_rgb(22, 22, 22));
            if pattern.source != 0
                && let Some(texture) = preview_texture(app, ctx, pattern.source)
            {
                let size = texture.size_vec2();
                let scale = (206.0 / size.x.max(1.0)).min(206.0 / size.y.max(1.0));
                let image = Rect::from_center_size(preview.center(), size * scale);
                ui.painter().with_clip_rect(preview.shrink(1.0)).image(
                    texture.id(),
                    image,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                ui.painter().rect_filled(preview.shrink(1.0), CornerRadius::same(4), Color32::from_black_alpha(96));
            }
            ui.painter().rect_stroke(preview, CornerRadius::same(5), Stroke::new(1.0, BORDER), StrokeKind::Inside);
            let source_button = at(origin, 49.9, 180.0, 140.2, 24.0);
            field(ui, source_button, CornerRadius::same(5), true);
            icons::paint_colorpicker(ui.painter(), at(origin, 59.0, 185.5, 13.0, 13.0), 14, Color32::WHITE);
            let source_name = app.session.doc().ok().and_then(|doc| doc.node(NodeId(pattern.source))).map_or("Select source…", |_| "Change source…");
            ui.painter().text(pos2(origin.x + 77.0, origin.y + 192.0), Align2::LEFT_CENTER, source_name, regular(11.0), Color32::WHITE);
            let source_response = ui.interact(source_button, Id::new("cp_pattern_source"), Sense::click()).tip("Pattern source");
            let sources = pattern_sources(app, target);
            crate::right::menu_popup(&source_response).width(208.0).show(|ui| {
                if sources.is_empty() {
                    ui.label("No eligible layers");
                }
                for (id, name) in &sources {
                    if crate::right::menu_check(ui, id.0 == pattern.source, name, "") {
                        pattern.source = id.0;
                        changed = true;
                        ui.close();
                    }
                }
            });
            ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 304.5, Stroke::new(1.0, BORDER));

            ui.painter().text(pos2(origin.x + 16.0, origin.y + 325.0), Align2::LEFT_CENTER, "Tile type", regular(11.0), MUTED);
            let tile_group = at(origin, 80.0, 313.0, 136.0, 24.0);
            field(ui, tile_group, CornerRadius::same(5), false);
            for (index, tile) in [PatternTile::Grid, PatternTile::HorizontalHex].into_iter().enumerate() {
                let rect = at(origin, 80.0 + index as f32 * 68.0, 313.0, 68.0, 24.0);
                let selected = pattern.tile == tile || (index == 1 && pattern.tile == PatternTile::VerticalHex);
                if selected {
                    ui.painter().rect_filled(rect, CornerRadius::same(5), PANEL);
                }
                tile_icon(
                    ui,
                    Rect::from_center_size(rect.center(), vec2(if index == 0 { 12.0 } else { 14.0 }, 12.0)),
                    index == 1,
                    if selected { Color32::WHITE } else { MUTED },
                );
                if ui
                    .interact(rect, Id::new(("cp_pattern_tile", index)), Sense::click())
                    .on_hover_text(if index == 0 { "Grid" } else { "Offset rows" })
                    .clicked()
                {
                    pattern.tile = tile;
                    changed = true;
                }
            }

            let (source_w, source_h) = source_dimensions(app, pattern.source);
            let scale = pattern.scale.clamp(PatternPaint::SCALE_RANGE.0, PatternPaint::SCALE_RANGE.1);
            if let Some(value) = value_field(ui, origin, 345.0, Some("Scale"), Some("scale"), "cp_pattern_scale_exact", pattern.scale * 100.0, false)
            {
                pattern.scale = (value / 100.0).clamp(PatternPaint::SCALE_RANGE.0, PatternPaint::SCALE_RANGE.1);
                changed = true;
            }
            if let Some(value) = value_field(
                ui,
                origin,
                377.0,
                Some("Spacing"),
                Some("X"),
                "cp_pattern_spacing_x_exact",
                pattern.spacing.0 / (source_w * scale) * 100.0,
                false,
            ) {
                pattern.spacing.0 = (value / 100.0 * source_w * scale).clamp(0.0, PatternPaint::MAX_SPACING);
                changed = true;
            }
            if let Some(value) =
                value_field(ui, origin, 409.0, None, Some("Y"), "cp_pattern_spacing_y_exact", pattern.spacing.1 / (source_h * scale) * 100.0, false)
            {
                pattern.spacing.1 = (value / 100.0 * source_h * scale).clamp(0.0, PatternPaint::MAX_SPACING);
                changed = true;
            }

            ui.painter().text(pos2(origin.x + 16.0, origin.y + 453.0), Align2::LEFT_CENTER, "Alignment", regular(11.0), MUTED);
            let grid = at(origin, 80.0, 444.5, 136.0, 65.0);
            field(ui, grid, CornerRadius::same(5), false);
            let axes = [PatternAlign::Start, PatternAlign::Center, PatternAlign::End];
            for row in 0..3 {
                for col in 0..3 {
                    let center = pos2(origin.x + 91.0 + col as f32 * 57.0, origin.y + 455.0 + row as f32 * 21.0);
                    let cell = Rect::from_center_size(center, vec2(16.0, 16.0));
                    let selected = pattern.align_x.unwrap_or(pattern.align) == axes[col] && pattern.align_y.unwrap_or(pattern.align) == axes[row];
                    ui.painter().rect_filled(
                        Rect::from_center_size(center, if selected { vec2(10.0, 10.0) } else { vec2(2.0, 2.0) }),
                        CornerRadius::same(if selected { 2 } else { 1 }),
                        if selected { ACTIVE } else { Color32::from_white_alpha(102) },
                    );
                    if ui.interact(cell, Id::new(("cp_pattern_alignment", row, col)), Sense::click()).clicked() {
                        pattern.align_x = Some(axes[col]);
                        pattern.align_y = Some(axes[row]);
                        changed = true;
                    }
                }
            }

            if changed {
                paint.kind = PaintKind::Pattern(pattern);
                write(app, target, &paint, false);
                finish(app);
            }
        });
    if switched_mode {
        return;
    }
    if !open || close_now || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        finish(app);
        app.ui.picker = None;
    }
}
