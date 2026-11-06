//! The compact UI3 solid-colour picker. Coordinates follow the 240 × 513 reference dialog.

use super::*;
use crate::primitives::TooltipResponseExt as _;

pub(super) const WIDTH: f32 = 240.0;
const HEIGHT: f32 = 513.0;
pub(super) const FIELD: Color32 = crate::primitives::FloatingPanel::DARK_FIELD;
pub(super) const BORDER: Color32 = Color32::from_rgb(68, 68, 68);
pub(super) const MUTED: Color32 = Color32::from_rgba_premultiplied(178, 178, 178, 178);

pub(super) fn panel(ctx: &egui::Context) -> Color32 {
    Tokens::get(ctx).picker_surface
}

const PAINT_KIND_HINTS: [&str; 6] = ["Solid", "Gradient", "Pattern", "Image", "Video", "Shader"];
const PAINT_KIND_IDS: [&str; 6] = ["cp_kind0", "cp_kind1", "cp_kind2", "cp_kind3", "cp_kind4", "cp_kind5"];

pub(super) fn at(origin: Pos2, x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(origin + vec2(x, y), vec2(w, h))
}

pub(super) fn paint_blend_icon(ui: &Ui, button: Rect) {
    icons::paint_appearance(ui.painter(), Rect::from_center_size(button.center(), vec2(16.0, 16.0)), icons::AppearanceIcon::Blend, Color32::WHITE);
}

pub(super) fn vector_button(ui: &mut Ui, rect: Rect, id: &'static str, index: usize, selected: bool, hint: &str) -> egui::Response {
    let response = ui.interact(rect, Id::new(id), Sense::click()).tip(hint);
    let t = Tokens::get(ui.ctx());
    let bg = if selected {
        t.accent
    } else if response.hovered() {
        Color32::from_rgb(64, 64, 64)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg);
    let size = match index {
        6 => vec2(14.0, 14.0),
        9 => vec2(10.01, 10.006),
        10 => vec2(13.981, 13.978),
        11 => vec2(12.001, 13.0),
        12 => vec2(10.001, 14.258),
        13 => vec2(12.0, 1.0),
        _ => vec2(12.0, 12.0),
    };
    let color = if !ui.is_enabled() {
        MUTED
    } else if selected {
        t.accent_on
    } else {
        Color32::WHITE
    };
    icons::paint_colorpicker(ui.painter(), Rect::from_center_size(rect.center(), size), index, color);
    response
}

pub(super) fn paint_kind_button(ui: &mut Ui, origin: Pos2, index: usize, selected: bool) -> Option<egui::Response> {
    let rect = at(origin, 8.0 + index as f32 * 28.0, 48.0, 24.0, 24.0);
    if index == 4 {
        ui.interact(rect, Id::new(PAINT_KIND_IDS[index]), Sense::click()).tip(format!("{} is not available yet", PAINT_KIND_HINTS[index]));
        icons::paint_colorpicker(ui.painter(), Rect::from_center_size(rect.center(), vec2(12.0, 12.0)), index, Color32::from_white_alpha(102));
        None
    } else {
        Some(vector_button(ui, rect, PAINT_KIND_IDS[index], index, selected, PAINT_KIND_HINTS[index]))
    }
}

pub(super) fn label_tab(ui: &mut Ui, rect: Rect, id: &'static str, text: &str, active: bool) -> bool {
    let response = ui.interact(rect, Id::new(id), Sense::click()).tip(text);
    ui.painter().rect_filled(rect, CornerRadius::same(5), if active { FIELD } else { panel(ui.ctx()) });
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        if active { semibold(11.0) } else { regular(11.0) },
        if active { Color32::WHITE } else { MUTED },
    );
    response.clicked()
}

fn header_tabs(ui: &mut Ui, origin: Pos2, id: &'static str, selected: usize) -> Option<usize> {
    use crate::primitives::{ControlWidth, Tab, Tabs};

    let tabs = [Tab::new(tr("Custom")), Tab::new(tr("Libraries"))];
    let rect = Rect::from_min_size(origin + vec2(8.0, 4.0), vec2(168.0, 32.0));
    let mut tabs_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    Tabs::new(Id::new(id), &tabs, Some(selected))
        .width(ControlWidth::Fixed(168.0))
        .selected_fill(FIELD)
        .hover_fill(FIELD)
        .text_colors(Color32::WHITE, MUTED)
        .show(&mut tabs_ui)
}

/// The same two-row header serves every paint editor and the Libraries view. The editor for a
/// paint kind owns only the response to a kind change; the tabs and hit areas remain identical.
pub(super) struct PaintHeader {
    pub(super) tab: Option<usize>,
    pub(super) create: bool,
    pub(super) close: bool,
    pub(super) kinds: Vec<(usize, egui::Response)>,
}

pub(super) fn paint_header(ui: &mut Ui, origin: Pos2, selected_tab: usize, selected_kind: Option<usize>, stop: bool) -> PaintHeader {
    let tab = header_tabs(ui, origin, if stop { "cp_stop_tabs" } else { "cp_tabs" }, selected_tab);
    let create = !stop && vector_button(ui, at(origin, 180.0, 8.0, 28.0, 24.0), "cp_new_style", 8, false, tr("New style")).clicked();
    let close =
        vector_button(ui, at(origin, 208.0, 8.0, 24.0, 24.0), if stop { "cp_stop_close" } else { "cp_close" }, 9, false, tr("Close")).clicked();
    ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 39.5, Stroke::new(1.0, BORDER));
    let mut kinds = Vec::new();
    if let Some(selected) = selected_kind {
        for index in 0..6 {
            if let Some(response) = paint_kind_button(ui, origin, index, index == selected) {
                kinds.push((index, response));
            }
        }
    }
    PaintHeader { tab, create, close, kinds }
}

pub(super) fn field(ui: &Ui, rect: Rect, corners: CornerRadius, border: bool) {
    ui.painter().rect_filled(rect, corners, if border { panel(ui.ctx()) } else { FIELD });
    if border {
        ui.painter().rect_stroke(rect, corners, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    }
}

pub(super) fn text_entry(ui: &mut Ui, rect: Rect, id: &'static str, value: String) -> Option<String> {
    let key = Id::new(id);
    let focused = ui.memory(|m| m.has_focus(key));
    let mut text: String = if focused { ui.data(|d| d.get_temp(key)).unwrap_or(value) } else { value };
    let response = ui.put(
        rect,
        egui::TextEdit::singleline(&mut text)
            .id(key)
            .font(regular(11.0))
            .text_color(Color32::WHITE)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO),
    );
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(key, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(key));
    }
    response.changed().then_some(text)
}

fn model_entry(ui: &mut Ui, origin: Pos2, model: usize, color: Color, allow_alpha: bool) -> Option<(Color, bool)> {
    use crate::primitives::{ColorControl, ColorModel, ColorValue};

    let selected = match model {
        1 => ColorModel::Rgb,
        2 => ColorModel::Hsl,
        3 => ColorModel::Hsb,
        4 => ColorModel::Css,
        _ => ColorModel::Hex,
    };
    let paint = Paint::solid(color.with_alpha(1.0));
    let change = ColorControl::new(Id::new("cp_model_fields"), &paint, ColorValue::Model(selected), f64::from(color.a) * 100.0)
        .opacity_editable(allow_alpha)
        .show_at(ui, at(origin, 80.0, 380.0, 144.0, 24.0));
    let mut next = change.edited_color.map(|(color, _)| color);
    let mut done = change.edited_color.is_some_and(|(_, done)| done);
    if let Some(edit) = change.opacity_edit.filter(|_| allow_alpha) {
        let (value, finished) = match edit {
            widgets::FieldEdit::Set(value) => (value, true),
            widgets::FieldEdit::Scrub(value, finished) => (value, finished),
        };
        next = Some(next.unwrap_or(color).with_alpha((value / 100.0).clamp(0.0, 1.0) as f32));
        done = finished;
    }
    next.map(|color| (color, done))
}

pub(super) fn pick_kind(app: &mut LizardApp, target: &PickerTarget, paint: &mut Paint, index: usize) {
    let base = paint.preview_color();
    match index {
        0 => paint.kind = PaintKind::Solid { color: base.with_alpha(1.0) },
        1 => {
            let mut gradient = Gradient::default_for(GradientKind::Linear, base);
            gradient.stops = vec![
                ColorStop { position: 0.0, color: base },
                ColorStop { position: 0.5, color: base.lerp(Color::BLACK, 0.235) },
                ColorStop { position: 1.0, color: base.lerp(Color::BLACK, 0.47) },
            ];
            paint.kind = PaintKind::Gradient(gradient);
        }
        3 => paint.kind = Paint::image("", ImageScaleMode::Fill).kind,
        2 => paint.kind = Paint::pattern(0).kind,
        _ => return,
    }
    write(app, target, paint, false);
}

pub(super) fn show(app: &mut LizardApp, ctx: &egui::Context, target: &PickerTarget, paint: Paint) {
    show_inner(app, ctx, target, paint, None);
}

pub(super) fn show_stop(app: &mut LizardApp, ctx: &egui::Context, target: &PickerTarget, paint: Paint, index: usize, parent_pos: Pos2) {
    show_inner(app, ctx, target, paint, Some((index, parent_pos)));
}

/// The same solid-color controls serve document paints and standalone theme palettes.
fn edit_solid_controls(ui: &mut Ui, origin: Pos2, color: Color, allow_alpha: bool) -> Option<(Color, bool)> {
    let mut hsb = ui
        .data(|d| d.get_temp::<(f32, f32, f32, [u8; 4])>(Id::new("cp_hsb")))
        .filter(|(_, _, _, rgba)| *rgba == color.to_rgba8())
        .map(|(h, s, b, _)| Hsb { h, s, b })
        .unwrap_or_else(|| Hsb::from_color(color));
    let square = at(origin, 16.0, 96.0, 208.0, 208.0);
    let (sv_changed, sv_done) = sv_square(ui, square, &mut hsb);
    let hue = at(origin, 52.0, 314.0, 172.0, 16.0);
    let (hue_changed, hue_done) = hue_slider(ui, hue, &mut hsb);
    let alpha = at(origin, 52.0, 342.0, 172.0, 16.0);
    let alpha_change = if allow_alpha { alpha_slider(ui, alpha, color) } else { ui.add_enabled_ui(false, |ui| alpha_slider(ui, alpha, color)).inner };
    let mut next = hsb.to_color().with_alpha(alpha_change.map_or(color.a, |(a, _)| a));
    let mut changed = sv_changed || hue_changed || alpha_change.is_some();
    let mut done = sv_done || hue_done || alpha_change.is_some_and(|(_, end)| end);

    let model: usize = ui.data(|d| d.get_temp(Id::new("cp_model"))).unwrap_or(0).min(MODELS.len() - 1);
    let mut format_ui = ui.new_child(egui::UiBuilder::new().max_rect(at(origin, 16.0, 380.0, 55.0, 24.0)));
    let format_response = crate::primitives::Select::trigger(&mut format_ui, MODELS[model], 55.0, None);
    crate::right::menu_popup(&format_response).show(|ui| {
        for model in MODELS {
            if crate::right::menu_item(ui, model, "") {
                ui.data_mut(|d| d.insert_temp(Id::new("cp_model"), MODELS.iter().position(|m| *m == model).unwrap_or(0)));
                ui.close();
            }
        }
    });
    if let Some((parsed, finished)) = model_entry(ui, origin, model, next, allow_alpha) {
        next = parsed;
        hsb = Hsb::from_color(next);
        changed = true;
        done |= finished;
    }
    ui.data_mut(|d| d.insert_temp(Id::new("cp_hsb"), (hsb.h, hsb.s, hsb.b, next.to_rgba8())));
    changed.then_some((next, done))
}

#[cfg(feature = "catalog")]
pub(super) fn show_standalone_solid(ctx: &egui::Context, color: Color, anchor: Pos2, open: &mut bool) -> Option<Color> {
    let mut updated = None;
    let mut close_now = false;
    crate::primitives::FloatingPanel::new(Id::new("theme_solid_color_picker"), anchor)
        .width(crate::primitives::FloatingPanelWidth::Compact)
        .height(crate::primitives::FloatingPanelHeight::Fixed(414.0))
        .fill(panel(ctx))
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.allocate_exact_size(vec2(WIDTH, 374.0), Sense::hover());
            let origin = header.min;
            label_tab(ui, at(origin, 8.0, 8.0, 91.609, 24.0), "cp_custom_tab", "Custom", true);
            if vector_button(ui, at(origin, 208.0, 8.0, 24.0, 24.0), "cp_close", 9, false, tr("Close")).clicked() {
                close_now = true;
            }
            ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 79.5, Stroke::new(1.0, BORDER));
            if let Some((next, _)) = edit_solid_controls(ui, origin, color, false) {
                updated = Some(next);
            }
        });
    if close_now || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        *open = false;
    }
    updated
}

fn show_inner(app: &mut LizardApp, ctx: &egui::Context, target: &PickerTarget, mut paint: Paint, stop: Option<(usize, Pos2)>) {
    let pos = stop.map_or_else(|| super::initial_position(app, ctx, target), |(_, parent)| pos2((parent.x - WIDTH - 8.0).max(8.0), parent.y));
    let solid_only = stop.is_some()
        || matches!(
            target,
            PickerTarget::DrawingTool { .. }
                | PickerTarget::Effect { .. }
                | PickerTarget::StyleEffect { .. }
                | PickerTarget::PageBackground
                | PickerTarget::Variable { .. }
                | PickerTarget::DraftVariable
                | PickerTarget::PluginField { .. }
        );
    let height = if solid_only { HEIGHT - 40.0 } else { HEIGHT };
    let mut open = true;
    let mut close_now = false;
    crate::primitives::FloatingPanel::new(Id::new(if stop.is_some() { "color_stop_picker" } else { "color_picker" }), pos)
        .width(crate::primitives::FloatingPanelWidth::Compact)
        .height(crate::primitives::FloatingPanelHeight::Fixed(height))
        .fill(panel(ctx))
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let dialog = Rect::from_min_size(header.min, vec2(WIDTH, height));
            ui.allocate_exact_size(vec2(WIDTH, height - 40.0), Sense::hover());
            let header_origin = dialog.min;
            let origin = if solid_only { dialog.min - vec2(0.0, 40.0) } else { dialog.min };
            let paint_types_allowed = !solid_only;

            let controls = paint_header(ui, header_origin, 0, (!solid_only).then_some(0), stop.is_some());
            if controls.tab == Some(1) {
                library_tab::activate(ctx);
            }
            if controls.create {
                library_tab::create(ctx, paint.clone());
                library_tab::activate(ctx);
            }
            if controls.close {
                close_now = true;
            }

            // 40 pt paint-type and colour-tool row, with the reference's 24 pt cells.
            if !solid_only {
                for (i, response) in controls.kinds {
                    if i == 5 && response.clicked() && paint_types_allowed {
                        if super::shader_tab::supports(target) {
                            super::shader_tab::activate(ctx);
                        } else {
                            app.toast("Shaders are available for fills");
                        }
                        continue;
                    }
                    if i == 1 && paint_types_allowed {
                        response.context_menu(|ui| {
                            for (label, kind) in [
                                ("Linear", GradientKind::Linear),
                                ("Radial", GradientKind::Radial),
                                ("Angular", GradientKind::Angular),
                                ("Diamond", GradientKind::Diamond),
                            ] {
                                if ui.button(label).clicked() {
                                    paint.kind = PaintKind::Gradient(Gradient::default_for(kind, paint.preview_color()));
                                    write(app, target, &paint, false);
                                    ui.close();
                                }
                            }
                        });
                    }
                    if response.clicked() && paint_types_allowed {
                        pick_kind(app, target, &mut paint, i);
                    }
                }
                let blend_rect = at(origin, 180.0, 48.0, 24.0, 24.0);
                let blend_response = ui.interact(blend_rect, Id::new("cp_blend"), Sense::click()).tip(tr("Blend mode"));
                ui.painter().rect_filled(blend_rect, CornerRadius::same(5), if blend_response.hovered() { FIELD } else { Color32::TRANSPARENT });
                paint_blend_icon(ui, blend_rect);
                if paint_types_allowed {
                    crate::right::menu_popup(&blend_response).show(|ui| {
                        if let Some(mode) = crate::right::blend_menu(ui, false) {
                            paint.blend = mode;
                            write(app, target, &paint, false);
                        }
                    });
                }
                vector_button(ui, at(origin, 208.0, 48.0, 24.0, 24.0), "cp_contrast", 6, false, tr("Contrast"));
                ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 79.5, Stroke::new(1.0, BORDER));
            } else {
                ui.painter().hline(header_origin.x..=header_origin.x + WIDTH, header_origin.y + 39.5, Stroke::new(1.0, BORDER));
            }

            let color = match stop {
                Some((index, _)) => match &paint.kind {
                    PaintKind::Gradient(gradient) => gradient.stops.get(index).map_or(Color::BLACK, |stop| stop.color),
                    _ => Color::BLACK,
                },
                None => paint.solid_color().unwrap_or(Color::BLACK).with_alpha(paint.opacity),
            };
            let eye = at(origin, 16.0, 326.0, 24.0, 24.0);
            if vector_button(ui, eye, "cp_eye", 10, false, tr("Pick color (I)")).clicked() {
                close_now = true;
                app.run("edit.pickColor", eyedrop_params(target, stop.map(|(index, _)| index)));
            }
            if let Some((next, done)) = edit_solid_controls(ui, origin, color, true) {
                if let Some((index, _)) = stop {
                    if let PaintKind::Gradient(gradient) = &mut paint.kind
                        && let Some(stop) = gradient.stops.get_mut(index)
                    {
                        stop.color = next;
                    }
                } else {
                    paint.kind = PaintKind::Solid { color: next.with_alpha(1.0) };
                    paint.opacity = next.a;
                }
                write(app, target, &paint, !done);
                if done {
                    finish(app);
                }
            }

            // Page swatches, two 16 pt rows with 8 pt gutters.
            ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 412.5, Stroke::new(1.0, BORDER));
            let set = at(origin, 16.0, 424.0, 208.0, 24.0);
            field(ui, set, CornerRadius::same(5), true);
            ui.painter().text(
                set.left_center() + vec2(8.0, 0.0),
                Align2::LEFT_CENTER,
                if ui.data(|d| d.get_temp::<bool>(Id::new("cp_swatches_recent"))).unwrap_or(false) { "Recent" } else { "On this page" },
                regular(11.0),
                Color32::WHITE,
            );
            icons::paint_colorpicker(ui.painter(), at(origin, 212.0, 434.5, 5.002, 3.002), 7, Color32::WHITE);
            let set_response = ui.interact(set, Id::new("cp_swatches_set"), Sense::click());
            let recent = app.session.recent_colors.clone();
            crate::right::menu_popup(&set_response).show(|ui| {
                if crate::right::menu_item(ui, "On this page", "") {
                    ui.data_mut(|d| d.insert_temp(Id::new("cp_swatches_recent"), false));
                    ui.close();
                }
                if crate::right::menu_item(ui, "Recent", "") {
                    ui.data_mut(|d| d.insert_temp(Id::new("cp_swatches_recent"), true));
                    ui.close();
                }
            });
            let show_recent: bool = ui.data(|d| d.get_temp(Id::new("cp_swatches_recent"))).unwrap_or(false);
            let colors = if show_recent { recent } else { page_colors(app, ui) };
            for (i, swatch) in colors.iter().take(18).enumerate() {
                let x = 16.0 + (i % 9) as f32 * 24.0;
                let y = 456.0 + (i / 9) as f32 * 24.0;
                let rect = at(origin, x, y, 16.0, 16.0);
                widgets::paint_swatch(ui.painter(), rect, &Paint::solid(*swatch));
                if ui.interact(rect, Id::new(("cp_swatch", i)), Sense::click()).lizard_tooltip(swatch.to_hex()).clicked() {
                    if let Some((index, _)) = stop {
                        if let PaintKind::Gradient(gradient) = &mut paint.kind
                            && let Some(current) = gradient.stops.get_mut(index)
                        {
                            current.color = *swatch;
                            write(app, target, &paint, false);
                        }
                    } else {
                        set_color(app, target, &mut paint, *swatch, stop_id(), ui);
                    }
                }
            }
        });
    if !open || close_now || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        finish(app);
        if stop.is_some() {
            ctx.data_mut(|data| data.remove::<usize>(Id::new("cp_gradient_stop_popup")));
        } else {
            app.ui.picker = None;
        }
    }
}
