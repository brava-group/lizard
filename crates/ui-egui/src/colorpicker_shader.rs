//! Shader browser and parameter editor inside the existing colour picker flow.

use super::custom::{self, BORDER, FIELD, MUTED, PANEL, WIDTH, at, label_tab, paint_kind_button, vector_button};
use super::*;
use figcraft_color::ShaderFill;
use figcraft_doc::{ShaderDefinition, ShaderFloatParameter, ShaderParameterControl};

const BROWSER_STATE: &str = "color_picker_shader_browser";
const CODE_STATE: &str = "color_picker_shader_code";
#[cfg(not(target_arch = "wasm32"))]
const THUMB_SIZE: usize = 96;

#[derive(Clone)]
struct CodeState {
    shader_id: String,
    draft: String,
    checked_draft: String,
    diagnostic: Option<String>,
    changed_at: f64,
}

pub(super) fn active(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new(BROWSER_STATE))).unwrap_or(false)
}

pub(super) fn activate(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(Id::new(BROWSER_STATE), true));
}

pub(super) fn deactivate(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(Id::new(BROWSER_STATE), false));
}

pub(super) fn supports(target: &PickerTarget) -> bool {
    matches!(
        target,
        PickerTarget::Paint { fills: true, .. } | PickerTarget::VectorRegion { .. } | PickerTarget::TextRange { .. } | PickerTarget::Style { .. }
    )
}

fn definitions(app: &FigApp) -> Vec<ShaderDefinition> {
    let mut items = figcraft_shader::presets();
    if let Ok(doc) = app.session.doc() {
        for definition in doc.shaders.values() {
            if let Some(existing) = items.iter_mut().find(|item| item.id == definition.id) {
                *existing = definition.clone();
            } else {
                items.push(definition.clone());
            }
        }
    }
    items
}

#[cfg(not(target_arch = "wasm32"))]
fn thumbnail(ctx: &egui::Context, definition: &ShaderDefinition) -> Option<egui::TextureHandle> {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::{Arc, Mutex, OnceLock};

    enum Pending {
        Working,
        Ready(Arc<Vec<u8>>),
        Failed,
    }
    static PIXELS: OnceLock<Mutex<HashMap<u64, Pending>>> = OnceLock::new();
    let cache = PIXELS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    definition.id.hash(&mut hasher);
    definition.source.hash(&mut hasher);
    serde_json::to_vec(&definition.parameters).unwrap_or_default().hash(&mut hasher);
    let key = hasher.finish();
    let texture_id = Id::new(("shader_thumbnail", key));
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(texture_id)) {
        return Some(texture);
    }
    let pixels = {
        let mut guard = cache.lock().ok()?;
        match guard.get(&key) {
            Some(Pending::Ready(bytes)) => Some(bytes.clone()),
            Some(Pending::Working | Pending::Failed) => None,
            None => {
                if guard.len() >= 128 {
                    guard.clear();
                }
                guard.insert(key, Pending::Working);
                let definition = definition.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let values = definition.parameters.iter().flat_map(ShaderFloatParameter::default_entries).collect();
                    let rendered = figcraft_shader::render_rgba(&definition, &values, THUMB_SIZE as u32, THUMB_SIZE as u32);
                    if let Ok(mut guard) = cache.lock() {
                        guard.insert(
                            key,
                            match rendered {
                                Ok(bytes) => Pending::Ready(Arc::new(bytes)),
                                Err(error) => {
                                    log::warn!("shader thumbnail {}: {error}", definition.id);
                                    Pending::Failed
                                }
                            },
                        );
                    }
                    ctx.request_repaint();
                });
                None
            }
        }
    }?;
    let image = egui::ColorImage::from_rgba_unmultiplied([THUMB_SIZE, THUMB_SIZE], &pixels);
    let texture = ctx.load_texture(format!("shader-thumbnail-{key}"), image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(texture_id, texture.clone()));
    Some(texture)
}

#[cfg(target_arch = "wasm32")]
fn thumbnail(_ctx: &egui::Context, _definition: &ShaderDefinition) -> Option<egui::TextureHandle> {
    None
}

fn apply(app: &mut FigApp, target: &PickerTarget, current: &Paint, definition: &ShaderDefinition) {
    if !supports(target) {
        return;
    }
    if let Err(error) = figcraft_shader::validate(definition) {
        app.toast(error);
        return;
    }
    let already_registered = app.session.doc().ok().is_some_and(|doc| doc.shaders.contains_key(&definition.id));
    finish(app);
    if let Err(error) = app.session.begin_interaction("Shader fill") {
        app.toast(error.to_string());
        return;
    }
    if !already_registered && let Err(error) = app.session.execute("shader.register", &json!({"definition": definition})) {
        app.toast(error.to_string());
        let _ = app.session.cancel_interaction();
        return;
    }
    let values = match &current.kind {
        PaintKind::Shader(fill) if fill.shader_id == definition.id => fill.values.clone(),
        _ => definition.parameters.iter().flat_map(ShaderFloatParameter::default_entries).collect(),
    };
    let paint = Paint { kind: PaintKind::Shader(ShaderFill { shader_id: definition.id.clone(), values }), ..current.clone() };
    write(app, target, &paint, false);
    let _ = app.session.commit_interaction();
}

pub(super) fn show_browser(app: &mut FigApp, ctx: &egui::Context, target: &PickerTarget, current: Paint) {
    if !supports(target) {
        deactivate(ctx);
        return;
    }
    let definitions = definitions(app);
    let picker_pos = crate::primitives::FloatingPanel::position(ctx, Id::new("color_picker"), super::initial_position(app, ctx, target));
    let position = pos2(picker_pos.x - 312.0, picker_pos.y);
    let mut open = true;
    let mut chosen = None;
    let mut create = false;
    crate::primitives::FloatingPanel::new(Id::new("shader_browser"), position)
        .width(crate::primitives::FloatingPanelWidth::Regular)
        .height(crate::primitives::FloatingPanelHeight::Fixed(420.0))
        .fill(PANEL)
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            if crate::primitives::FloatingPanel::title_header(ui, header, "Shaders", Id::new("shader_browser_close")) {
                deactivate(ctx);
            }
            let add = Rect::from_center_size(pos2(header.right() - 48.0, header.center().y), vec2(24.0, 24.0));
            if crate::primitives::Button::icon(Icon::Plus, "Create shader").show_at(ui, Id::new("shader_browser_add"), add).clicked() {
                create = true;
            }
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(355.0).show(ui, |ui| {
                for row in definitions.chunks(3) {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        for definition in row {
                            let (tile, response) = ui.allocate_exact_size(vec2(88.0, 110.0), Sense::click());
                            let square = Rect::from_min_size(tile.min, vec2(88.0, 88.0));
                            let selected = matches!(&current.kind, PaintKind::Shader(fill) if fill.shader_id == definition.id);
                            ui.painter().rect_filled(
                                square,
                                CornerRadius::same(7),
                                if response.hovered() { FIELD } else { Color32::from_rgb(34, 34, 34) },
                            );
                            if let Some(texture) = thumbnail(ctx, definition) {
                                ui.painter().with_clip_rect(square.shrink(1.0)).image(
                                    texture.id(),
                                    square.shrink(1.0),
                                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                            } else {
                                ui.painter().text(square.center(), Align2::CENTER_CENTER, "✦", semibold(20.0), Color32::from_rgb(130, 130, 150));
                            }
                            ui.painter().rect_stroke(
                                square,
                                CornerRadius::same(7),
                                Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { Color32::from_rgb(28, 149, 255) } else { BORDER }),
                                StrokeKind::Inside,
                            );
                            let label = if definition.name.chars().count() > 14 {
                                format!("{}…", definition.name.chars().take(13).collect::<String>())
                            } else {
                                definition.name.clone()
                            };
                            ui.painter().text(
                                pos2(tile.center().x, tile.bottom() - 11.0),
                                Align2::CENTER_CENTER,
                                label,
                                regular(10.0),
                                Color32::WHITE,
                            );
                            if response.on_hover_text(&definition.description).clicked() {
                                chosen = Some(definition.clone());
                            }
                        }
                    });
                }
            });
        });
    if !open {
        deactivate(ctx);
    }
    if let Some(definition) = chosen {
        apply(app, target, &current, &definition);
        deactivate(ctx);
    }
    if create {
        let mut definition = figcraft_shader::demo();
        let mut number = 1_u32;
        if let Ok(doc) = app.session.doc() {
            while doc.shaders.contains_key(&format!("lizard.custom.{number}")) {
                number += 1;
            }
        }
        definition.id = format!("lizard.custom.{number}");
        definition.name = format!("Custom shader {number}");
        definition.description = "Custom WGSL fill".into();
        definition.author.clear();
        definition.license.clear();
        definition.attribution.clear();
        apply(app, target, &current, &definition);
        ctx.data_mut(|data| {
            data.insert_temp(
                Id::new(CODE_STATE),
                CodeState {
                    shader_id: definition.id,
                    draft: definition.source.clone(),
                    checked_draft: definition.source,
                    diagnostic: None,
                    changed_at: 0.0,
                },
            )
        });
        deactivate(ctx);
    }
}

pub(super) fn show_editor(app: &mut FigApp, ctx: &egui::Context, target: &PickerTarget, mut paint: Paint) {
    let PaintKind::Shader(fill) = &paint.kind else { return };
    let shader_id = fill.shader_id.clone();
    let definition = app.session.doc().ok().and_then(|doc| doc.shaders.get(&shader_id)).cloned();
    let parameter_count = definition.as_ref().map_or(0, |definition| definition.parameters.len());
    let height = 144.0 + parameter_count as f32 * 32.0;
    let mut open = true;
    let mut close_now = false;
    let mut browser_now = false;
    let mut open_color = None;
    let position = super::initial_position(app, ctx, target);
    crate::primitives::FloatingPanel::new(Id::new("color_picker"), position)
        .width(crate::primitives::FloatingPanelWidth::Compact)
        .height(crate::primitives::FloatingPanelHeight::Fixed(height))
        .fill(PANEL)
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let origin = header.min;
            ui.allocate_exact_size(vec2(WIDTH, height - 40.0), Sense::hover());
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
                let Some(response) = paint_kind_button(ui, origin, index, index == 5) else { continue };
                if response.clicked() {
                    if index == 5 {
                        browser_now = true;
                    } else {
                        custom::pick_kind(app, target, &mut paint, index);
                    }
                }
            }
            let blend_rect = at(origin, 180.0, 48.0, 24.0, 24.0);
            let blend = ui.interact(blend_rect, Id::new("cp_shader_blend"), Sense::click()).tip("Blend mode");
            ui.painter().rect_filled(blend_rect, CornerRadius::same(5), if blend.hovered() { FIELD } else { Color32::TRANSPARENT });
            custom::paint_blend_icon(ui, blend_rect);
            crate::right::menu_popup(&blend).show(|ui| {
                if let Some(mode) = crate::right::blend_menu(ui, false) {
                    paint.blend = mode;
                    write(app, target, &paint, false);
                }
            });
            vector_button(ui, at(origin, 208.0, 48.0, 24.0, 24.0), "cp_shader_contrast", 6, false, "Contrast");
            ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 79.5, Stroke::new(1.0, BORDER));

            let name = definition.as_ref().map_or("Missing shader", |definition| definition.name.as_str());
            let selected_rect = at(origin, 16.0, 92.0, 152.0, 24.0);
            let mut select_ui = ui.new_child(egui::UiBuilder::new().max_rect(selected_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
            if crate::primitives::Select::trigger(&mut select_ui, name, 152.0, None).clicked() {
                browser_now = true;
            }
            let more = at(origin, 192.0, 92.0, 24.0, 24.0);
            if ui.interact(more, Id::new("cp_shader_code"), Sense::click()).on_hover_text("Edit WGSL").clicked()
                && let Some(definition) = &definition
            {
                ctx.data_mut(|data| {
                    data.insert_temp(
                        Id::new(CODE_STATE),
                        CodeState {
                            shader_id: definition.id.clone(),
                            draft: definition.source.clone(),
                            checked_draft: definition.source.clone(),
                            diagnostic: None,
                            changed_at: 0.0,
                        },
                    )
                });
                deactivate(ctx);
            }
            ui.painter().text(more.center(), Align2::CENTER_CENTER, "•••", semibold(11.0), Color32::WHITE);
            if let Some(definition) = &definition {
                for (index, parameter) in definition.parameters.iter().enumerate() {
                    let y = 132.0 + index as f32 * 32.0;
                    ui.painter().text(at(origin, 16.0, y, 70.0, 24.0).left_center(), Align2::LEFT_CENTER, &parameter.name, regular(11.0), MUTED);
                    let field = at(origin, 88.0, y, 136.0, 24.0);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field).layout(egui::Layout::left_to_right(egui::Align::Center)));
                    let PaintKind::Shader(fill) = &paint.kind else { continue };
                    let value = parameter.resolved_scalar(&fill.values);
                    let control_id = Id::new(("cp_shader_parameter", &shader_id, &parameter.id));
                    let mut change = None;
                    match parameter.control {
                        ShaderParameterControl::Slider => {
                            change = crate::primitives::Slider::new(control_id, &parameter.name)
                                .range(f64::from(parameter.min), f64::from(parameter.max))
                                .step(f64::from(parameter.step))
                                .value(Some(f64::from(value)))
                                .width(crate::primitives::ControlWidth::Fixed(136.0))
                                .show(&mut child)
                                .change
                                .map(|(value, done)| (value as f32, done));
                        }
                        ShaderParameterControl::Number => {
                            let field = crate::primitives::NumberField::new(control_id, widgets::Lead::None, Some(f64::from(value)), "")
                                .step(f64::from(parameter.step))
                                .show(&mut child, crate::primitives::ControlWidth::Fixed(136.0));
                            change = field.edit.and_then(|edit| edit.settled()).map(|edit| match edit {
                                widgets::FieldEdit::Set(value) => (value as f32, true),
                                widgets::FieldEdit::Scrub(value, done) => (value as f32, done),
                            });
                        }
                        ShaderParameterControl::Select => {
                            let labels: Vec<&str> = parameter.options.iter().map(|option| option.label.as_str()).collect();
                            let selected = parameter.options.iter().position(|option| option.value == value).unwrap_or(0);
                            if let Some(index) = crate::primitives::Select::new(&labels, selected)
                                .label(&parameter.name)
                                .width(crate::primitives::ControlWidth::Fixed(136.0))
                                .position(crate::primitives::SelectPosition::BottomLeft)
                                .show(&mut child)
                                && let Some(option) = parameter.options.get(index)
                            {
                                change = Some((option.value, true));
                            }
                        }
                        ShaderParameterControl::Toggle => {
                            if crate::primitives::Switch::new(control_id, value != 0.0, if value != 0.0 { "On" } else { "Off" })
                                .show_at(&mut child, field)
                                .clicked()
                            {
                                change = Some((if value == 0.0 { 1.0 } else { 0.0 }, true));
                            }
                        }
                        ShaderParameterControl::Color => {
                            let color = parameter.color_value(&fill.values);
                            let hex = color.to_hex();
                            ui.painter().rect_filled(field, CornerRadius::same(5), FIELD);
                            let response = crate::primitives::ColorControl::new(
                                control_id,
                                &Paint::solid(color),
                                crate::primitives::ColorValue::EditableHex(&hex),
                                100.0,
                            )
                            .inline()
                            .show_at(&mut child, field);
                            if response.open_picker {
                                open_color = Some(parameter.id.clone());
                            }
                            if let Some(hex) = response.edited_hex
                                && let Some(color) = Color::from_hex(&hex)
                                && let PaintKind::Shader(fill) = &mut paint.kind
                            {
                                parameter.set_color(&mut fill.values, color);
                                write(app, target, &paint, false);
                            }
                        }
                    }
                    if let Some((next, done)) = change
                        && let Some(next) = parameter.scalar_value(next)
                        && let PaintKind::Shader(fill) = &mut paint.kind
                    {
                        fill.values.insert(parameter.id.clone(), next);
                        write(app, target, &paint, !done);
                        if done {
                            finish(app);
                        }
                    }
                }
            }
        });
    if browser_now {
        activate(ctx);
    }
    if let Some(parameter) = open_color {
        ctx.data_mut(|data| data.insert_temp(Id::new("shader_color_parameter_popup"), parameter));
    }
    if let Some(parameter_id) = ctx.data(|data| data.get_temp::<String>(Id::new("shader_color_parameter_popup")))
        && let Some(definition) = &definition
        && let Some(parameter) =
            definition.parameters.iter().find(|parameter| parameter.id == parameter_id && parameter.control == ShaderParameterControl::Color)
    {
        show_color_parameter(app, ctx, target, &mut paint, parameter);
    }
    if !open || close_now || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        finish(app);
        app.ui.picker = None;
        deactivate(ctx);
    }
    if ctx.data(|data| data.get_temp::<CodeState>(Id::new(CODE_STATE))).is_some() {
        show_code(app, ctx, target, &paint, definition);
    }
}

fn show_color_parameter(app: &mut FigApp, ctx: &egui::Context, target: &PickerTarget, paint: &mut Paint, parameter: &ShaderFloatParameter) {
    let PaintKind::Shader(fill) = &paint.kind else { return };
    let color = parameter.color_value(&fill.values);
    let picker_pos = crate::primitives::FloatingPanel::position(ctx, Id::new("color_picker"), super::initial_position(app, ctx, target));
    let position = pos2(picker_pos.x - 248.0, picker_pos.y);
    let mut open = true;
    let mut close_now = false;
    crate::primitives::FloatingPanel::new(Id::new("shader_color_parameter_panel"), position)
        .width(crate::primitives::FloatingPanelWidth::Compact)
        .height(crate::primitives::FloatingPanelHeight::Fixed(360.0))
        .fill(PANEL)
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let origin = header.min;
            ui.allocate_exact_size(vec2(WIDTH, 320.0), Sense::hover());
            if crate::primitives::FloatingPanel::title_header(ui, header, &parameter.name, Id::new("shader_color_parameter_close")) {
                close_now = true;
            }
            let mut hsb = Hsb::from_color(color);
            let (square_changed, square_done) = super::sv_square(ui, at(origin, 16.0, 56.0, 208.0, 208.0), &mut hsb);
            let (hue_changed, hue_done) = super::hue_slider(ui, at(origin, 16.0, 280.0, 208.0, 16.0), &mut hsb);
            let hex = color.to_hex();
            let control = crate::primitives::ColorControl::new(
                Id::new(("shader_color_parameter_value", &parameter.id)),
                &Paint::solid(color),
                crate::primitives::ColorValue::EditableHex(&hex),
                f64::from(color.a) * 100.0,
            )
            .show_at(ui, at(origin, 16.0, 312.0, 208.0, 24.0));
            let mut next = (square_changed || hue_changed).then(|| (hsb.to_color().with_alpha(color.a), square_done || hue_done));
            if let Some(hex) = control.edited_hex
                && let Some(parsed) = Color::from_hex(&hex)
            {
                next = Some((parsed.with_alpha(color.a), true));
            }
            if let Some(edit) = control.opacity_edit {
                let (value, done) = match edit {
                    widgets::FieldEdit::Set(value) => (value, true),
                    widgets::FieldEdit::Scrub(value, done) => (value, done),
                };
                next = Some((color.with_alpha((value / 100.0).clamp(0.0, 1.0) as f32), done));
            }
            if let Some((next, done)) = next
                && let PaintKind::Shader(fill) = &mut paint.kind
            {
                parameter.set_color(&mut fill.values, next);
                write(app, target, paint, !done);
                if done {
                    finish(app);
                }
            }
        });
    if !open || close_now {
        finish(app);
        ctx.data_mut(|data| data.remove::<String>(Id::new("shader_color_parameter_popup")));
    }
}

fn show_code(app: &mut FigApp, ctx: &egui::Context, target: &PickerTarget, _paint: &Paint, definition: Option<ShaderDefinition>) {
    let Some(mut state) = ctx.data(|data| data.get_temp::<CodeState>(Id::new(CODE_STATE))) else { return };
    let Some(definition) = definition.filter(|definition| definition.id == state.shader_id) else {
        ctx.data_mut(|data| data.remove::<CodeState>(Id::new(CODE_STATE)));
        return;
    };
    let picker_pos = crate::primitives::FloatingPanel::position(ctx, Id::new("color_picker"), super::initial_position(app, ctx, target));
    let position = pos2(picker_pos.x - 408.0, picker_pos.y);
    let mut open = true;
    let mut close_now = false;
    let mut apply = false;
    crate::primitives::FloatingPanel::new(Id::new("shader_code_editor"), position)
        .width(crate::primitives::FloatingPanelWidth::Wide)
        .height(crate::primitives::FloatingPanelHeight::Fixed(470.0))
        .fill(PANEL)
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            if crate::primitives::FloatingPanel::title_header(ui, header, "Edit WGSL", Id::new("shader_code_close")) {
                close_now = true;
            }
            ui.add_space(8.0);
            let preview_rect = ui.allocate_exact_size(vec2(368.0, 94.0), Sense::hover()).0;
            let mut candidate = definition.clone();
            candidate.source = state.draft.clone();
            if state.diagnostic.is_none() && state.checked_draft == state.draft {
                if let Some(texture) = thumbnail(ctx, &candidate) {
                    ui.painter().image(texture.id(), preview_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                } else {
                    ui.painter().rect_filled(preview_rect, CornerRadius::same(5), FIELD);
                    ui.painter().text(preview_rect.center(), Align2::CENTER_CENTER, "Rendering preview…", regular(11.0), MUTED);
                }
            } else {
                ui.painter().rect_filled(preview_rect, CornerRadius::same(5), FIELD);
                ui.painter().text(preview_rect.center(), Align2::CENTER_CENTER, "Last valid shader remains on canvas", regular(11.0), MUTED);
            }
            ui.add_space(4.0);
            egui::ScrollArea::both().max_height(242.0).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let (gutter, _) = ui.allocate_exact_size(vec2(28.0, 228.0), Sense::hover());
                    let longest = state.draft.lines().map(str::len).max().unwrap_or(0);
                    let editor_width = (longest as f32 * 7.0 + 24.0).clamp(328.0, 2_048.0);
                    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
                        ui.fonts_mut(|fonts| fonts.layout_job(wgsl_layout_job(text.as_str())))
                    };
                    let output = egui::TextEdit::multiline(&mut state.draft)
                        .code_editor()
                        .desired_width(editor_width)
                        .min_size(vec2(editor_width, 228.0))
                        .layouter(&mut layouter)
                        .show(ui);
                    let painter = ui.painter().with_clip_rect(gutter.intersect(ui.clip_rect()));
                    for (index, row) in output.galley.rows.iter().enumerate() {
                        let y = output.galley_pos.y + row.rect().center().y;
                        painter.text(
                            pos2(gutter.right() - 4.0, y),
                            Align2::RIGHT_CENTER,
                            (index + 1).to_string(),
                            egui::FontId::monospace(11.0),
                            MUTED,
                        );
                    }
                    if output.response.changed() {
                        state.changed_at = ctx.input(|input| input.time);
                        ctx.request_repaint_after(std::time::Duration::from_millis(320));
                    }
                });
            });
            if state.checked_draft != state.draft && ctx.input(|input| input.time) - state.changed_at >= 0.3 {
                candidate.source = state.draft.clone();
                state.diagnostic = figcraft_shader::validate(&candidate).err();
                state.checked_draft = state.draft.clone();
            }
            let diagnostic = state.diagnostic.as_deref().unwrap_or("WGSL valid");
            ui.label(egui::RichText::new(diagnostic.lines().take(2).collect::<Vec<_>>().join(" ")).size(10.0).color(if state.diagnostic.is_some() {
                Color32::from_rgb(255, 130, 120)
            } else {
                Color32::from_rgb(125, 205, 150)
            }));
            ui.horizontal(|ui| {
                if crate::primitives::Button::new("Restore saved").show(ui).clicked() {
                    state.draft = definition.source.clone();
                    state.checked_draft = state.draft.clone();
                    state.diagnostic = None;
                    state.changed_at = 0.0;
                }
                if crate::primitives::Button::new("Apply WGSL").enabled(state.diagnostic.is_none()).show(ui).clicked() {
                    apply = true;
                }
            });
        });
    if apply {
        let mut updated = definition;
        updated.source = state.draft.clone();
        match figcraft_shader::validate(&updated) {
            Ok(()) => {
                app.run("shader.register", json!({"definition": updated}));
                close_now = true;
            }
            Err(error) => {
                state.diagnostic = Some(error);
            }
        }
    }
    if !open || close_now {
        ctx.data_mut(|data| data.remove::<CodeState>(Id::new(CODE_STATE)));
    } else {
        ctx.data_mut(|data| data.insert_temp(Id::new(CODE_STATE), state));
    }
}

fn wgsl_layout_job(source: &str) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};

    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let font = egui::FontId::monospace(11.0);
    let format = |color| TextFormat { font_id: font.clone(), color, ..Default::default() };
    let normal = format(Color32::from_rgb(225, 225, 225));
    let mut cursor = 0;
    let mut plain = 0;
    while cursor < source.len() {
        let rest = &source[cursor..];
        let token = if rest.starts_with("//") {
            let end = rest.find('\n').unwrap_or(rest.len());
            Some((cursor + end, Color32::from_rgb(132, 171, 136)))
        } else if let Some(first) = rest.chars().next() {
            if first.is_ascii_digit() {
                let end = rest.bytes().take_while(|byte| byte.is_ascii_digit() || *byte == b'.').count();
                Some((cursor + end, Color32::from_rgb(230, 174, 116)))
            } else if first.is_ascii_alphabetic() || first == '_' || first == '@' {
                let end = rest.bytes().take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'@').count();
                let word = &rest[..end];
                let color = if word.starts_with('@') {
                    Color32::from_rgb(233, 175, 116)
                } else if matches!(word, "fn" | "let" | "var" | "return" | "for" | "if" | "else" | "struct" | "loop" | "break" | "continue") {
                    Color32::from_rgb(195, 154, 239)
                } else if matches!(word, "f32" | "i32" | "u32" | "bool" | "vec2f" | "vec3f" | "vec4f" | "mat2x2f" | "mat3x3f" | "mat4x4f") {
                    Color32::from_rgb(139, 193, 236)
                } else {
                    Color32::from_rgb(225, 225, 225)
                };
                Some((cursor + end, color))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((end, color)) = token {
            if plain < cursor {
                job.append(&source[plain..cursor], 0.0, normal.clone());
            }
            job.append(&source[cursor..end], 0.0, format(color));
            cursor = end;
            plain = end;
        } else if let Some(ch) = rest.chars().next() {
            cursor += ch.len_utf8();
        } else {
            break;
        }
    }
    if plain < source.len() {
        job.append(&source[plain..], 0.0, normal);
    }
    job
}
