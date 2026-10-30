//! Libraries and the adjacent style/variable creation panel of the colour picker.
use super::*;
use figcraft_doc::{StyleKind, VariableType, VariableValue};
use figcraft_engine::library;

const STATE: &str = "color_picker_library_state";

#[derive(Clone, Copy, PartialEq, Eq)]
enum CreateKind {
    Style,
    Variable,
}

#[derive(Clone, Default)]
struct State {
    active: bool,
    query: String,
    filter: String,
    grid: bool,
    create: Option<CreateKind>,
    name: String,
    description: String,
    color: String,
    opacity: String,
    paints: Vec<Paint>,
    collection: String,
}

fn state(ctx: &egui::Context) -> State {
    ctx.data(|d| d.get_temp(Id::new(STATE))).unwrap_or_default()
}
fn save(ctx: &egui::Context, state: State) {
    ctx.data_mut(|d| d.insert_temp(Id::new(STATE), state));
}
pub(super) fn draft_paint(ctx: &egui::Context, index: usize) -> Option<Paint> {
    let s = state(ctx);
    (s.create == Some(CreateKind::Style)).then(|| s.paints.get(index).cloned()).flatten()
}
pub(super) fn set_draft_paint(ctx: &egui::Context, index: usize, paint: Paint) {
    let mut s = state(ctx);
    if s.create == Some(CreateKind::Style)
        && let Some(slot) = s.paints.get_mut(index)
    {
        if index == 0 {
            s.color = paint.preview_color().to_css_hex();
        }
        *slot = paint;
        save(ctx, s);
    }
}
pub(super) fn draft_variable_paint(ctx: &egui::Context) -> Option<Paint> {
    let s = state(ctx);
    if s.create != Some(CreateKind::Variable) {
        return None;
    }
    let color = Color::from_hex_or_name(&s.color)?;
    let opacity = s.opacity.parse::<f32>().ok()?;
    let mut paint = Paint::solid(color.with_alpha(1.0));
    paint.opacity = (opacity / 100.0).clamp(0.0, 1.0);
    Some(paint)
}
pub(super) fn set_draft_variable_paint(ctx: &egui::Context, paint: &Paint) {
    let mut s = state(ctx);
    if s.create == Some(CreateKind::Variable)
        && let Some(color) = paint.solid_color()
    {
        s.color = color.with_alpha(1.0).to_css_hex();
        s.opacity = format!("{:.2}", paint.opacity * 100.0);
        save(ctx, s);
    }
}
pub(super) fn active(ctx: &egui::Context) -> bool {
    state(ctx).active
}
pub(super) fn activate(ctx: &egui::Context) {
    let mut s = state(ctx);
    s.active = true;
    save(ctx, s);
}
pub(super) fn deactivate(ctx: &egui::Context) {
    let mut s = state(ctx);
    s.active = false;
    s.create = None;
    save(ctx, s);
}
pub(super) fn create(ctx: &egui::Context, paint: Paint) {
    let color = paint.preview_color();
    let mut s = state(ctx);
    s.create = Some(CreateKind::Style);
    s.color = color.to_css_hex();
    s.opacity = format!("{:.0}", color.a * 100.0);
    s.paints = vec![paint];
    s.name.clear();
    s.description.clear();
    save(ctx, s);
}
pub(super) fn create_variable(ctx: &egui::Context, color: Color) {
    let mut s = state(ctx);
    s.active = false;
    s.create = Some(CreateKind::Variable);
    s.color = color.to_css_hex();
    s.opacity = format!("{:.0}", color.a * 100.0);
    s.paints.clear();
    s.collection.clear();
    s.name.clear();
    s.description.clear();
    save(ctx, s);
}

/// The creation panel lists the topmost fill first. Documents store paint stacks bottom first.
fn creation_style_paints(s: &State) -> Vec<Paint> {
    s.paints.iter().rev().cloned().collect()
}

pub(super) fn show_standalone(app: &mut FigApp, ctx: &egui::Context) {
    if app.ui.picker.is_some() && !matches!(app.ui.picker, Some(PickerTarget::DraftStyle { .. } | PickerTarget::DraftVariable)) {
        return;
    }
    let mut s = state(ctx);
    let Some(kind) = s.create else { return };
    let Some(doc) = app.session.doc().ok().cloned() else { return };
    let screen = ctx.content_rect();
    let pos = pos2(screen.max.x - app.ui.right_width - 260.0, 140.0);
    creation_panel(app, ctx, &doc, pos, &mut s, kind);
    save(ctx, s);
}

#[derive(Clone)]
struct Swatch {
    name: String,
    color: Color,
    style: Option<(Option<String>, String)>,
    variable: Option<(Option<String>, String)>,
}

fn collect(app: &FigApp, doc: &figcraft_doc::Document, query: &str, filter: &str) -> Vec<(String, Vec<Swatch>)> {
    let q = query.trim().to_lowercase();
    let libs = crate::variable_ui::libraries(app, doc);
    let mut groups = Vec::new();
    if filter.is_empty() || filter == "_local" {
        let mut rows = Vec::new();
        for s in library::local_styles(doc) {
            if !s.name.to_lowercase().contains(&q) {
                continue;
            }
            if let StyleKind::Paint { paints } = &s.kind
                && let Some(p) = paints.first()
            {
                rows.push(Swatch { name: s.name.clone(), color: p.preview_color(), style: Some((None, s.id.clone())), variable: None });
            }
        }
        for v in &doc.variables.variables {
            if library::is_remote_variable(doc, &v.id) || v.kind != VariableType::Color || !v.name.to_lowercase().contains(&q) {
                continue;
            }
            if let Some(VariableValue::Color(color)) = doc.variables.resolve(&v.id, &Default::default()) {
                rows.push(Swatch { name: v.name.clone(), color, style: None, variable: Some((None, v.id.clone())) });
            }
        }
        if !rows.is_empty() {
            groups.push((tr("This file").to_string(), rows));
        }
    }
    for lib in libs {
        if !filter.is_empty() && filter != lib.manifest.id {
            continue;
        }
        let mut rows = Vec::new();
        for s in &lib.doc.styles {
            if !lib.manifest.style(&s.id).is_some_and(|entry| entry.published) || !s.name.to_lowercase().contains(&q) {
                continue;
            }
            if let StyleKind::Paint { paints } = &s.kind
                && let Some(p) = paints.first()
            {
                rows.push(Swatch {
                    name: s.name.clone(),
                    color: p.preview_color(),
                    style: Some((Some(lib.manifest.id.clone()), s.id.clone())),
                    variable: None,
                });
            }
        }
        for v in library::published_variables(&lib) {
            if v.kind != VariableType::Color || !v.name.to_lowercase().contains(&q) {
                continue;
            }
            if let Some(VariableValue::Color(color)) = lib.doc.variables.resolve(&v.id, &Default::default()) {
                rows.push(Swatch { name: v.name.clone(), color, style: None, variable: Some((Some(lib.manifest.id.clone()), v.id.clone())) });
            }
        }
        if !rows.is_empty() {
            groups.push((lib.manifest.name.clone(), rows));
        }
    }
    groups
}

fn icon_button(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash + std::fmt::Debug, index: usize, size: egui::Vec2, hint: &str) -> bool {
    let response = ui.interact(rect, Id::new(id), Sense::click()).on_hover_text(hint);
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(5), custom::FIELD);
    }
    icons::paint_panel_reference(ui.painter(), Rect::from_center_size(rect.center(), size), index, Color32::WHITE);
    response.clicked()
}

pub(super) fn show(app: &mut FigApp, ctx: &egui::Context, target: &PickerTarget, paint: Paint) {
    let Some(doc) = app.session.doc().ok().cloned() else { return };
    let mut s = state(ctx);
    let screen = ctx.content_rect();
    let pos = pos2(screen.max.x - app.ui.right_width - 260.0, 140.0);
    let rows = collect(app, &doc, &s.query, &s.filter);
    let mut picked: Option<Swatch> = None;
    let mut close = false;
    let mut custom = false;
    crate::primitives::FloatingPanel::new(Id::new("color_picker"), pos)
        .height(crate::primitives::FloatingPanelHeight::Fixed(427.0))
        .fill(custom::PANEL)
        .corner_radius(8)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let panel = Rect::from_min_size(header.min, vec2(240.0, 427.0));
            ui.allocate_exact_size(vec2(240.0, 387.0), Sense::hover());
            ui.painter().rect_filled(panel, CornerRadius::same(8), custom::PANEL);
            let o = panel.min;
            if custom::label_tab(ui, custom::at(o, 8.0, 8.0, 91.609, 24.0), "cpl_custom", "Custom", false) {
                custom = true;
            }
            custom::label_tab(ui, custom::at(o, 103.609, 8.0, 74.914, 24.0), "cpl_libraries", "Libraries", true);
            if icon_button(ui, custom::at(o, 180.0, 8.0, 28.0, 24.0), "cpl_add", 39, vec2(12.0, 12.0), tr("Create style or variable")) {
                s.create = Some(CreateKind::Style);
                s.color = paint.preview_color().to_css_hex();
                s.opacity = format!("{:.0}", paint.preview_color().a * 100.0);
                s.paints = vec![paint.clone()];
                s.name.clear();
                s.description.clear();
            }
            if icon_button(ui, custom::at(o, 208.0, 8.0, 24.0, 24.0), "cpl_close", 40, vec2(10.0, 10.0), tr("Close")) {
                close = true;
            }
            ui.painter().hline(o.x..=o.x + 240.0, o.y + 39.5, Stroke::new(1.0, custom::BORDER));
            let search = custom::at(o, 12.0, 48.0, 216.0, 24.0);
            custom::field(ui, search, CornerRadius::same(5), false);
            icons::paint_panel_reference(
                ui.painter(),
                Rect::from_center_size(pos2(search.min.x + 13.0, search.center().y), vec2(14.0, 14.0)),
                41,
                custom::MUTED,
            );
            ui.put(
                Rect::from_min_max(pos2(search.min.x + 28.0, search.min.y + 3.0), search.max - vec2(5.0, 2.0)),
                egui::TextEdit::singleline(&mut s.query)
                    .hint_text(tr("Search"))
                    .font(regular(11.0))
                    .text_color(Color32::WHITE)
                    .frame(egui::Frame::NONE),
            );
            let filter = custom::at(o, 12.0, 88.0, 176.0, 24.0);
            let filter_label = if s.filter == "_local" {
                tr("This file").to_string()
            } else if s.filter.is_empty() {
                tr("All libraries").to_string()
            } else {
                crate::variable_ui::libraries(app, &doc)
                    .iter()
                    .find(|lib| lib.manifest.id == s.filter)
                    .map_or_else(|| tr("All libraries").to_string(), |lib| lib.manifest.name.clone())
            };
            let filter_response = ui.interact(filter, Id::new("color_library_filter"), Sense::click());
            custom::field(ui, filter, CornerRadius::same(5), filter_response.hovered());
            ui.painter().text(filter.min + vec2(9.0, 12.0), Align2::LEFT_CENTER, filter_label, regular(11.0), Color32::WHITE);
            icons::paint_panel_reference(
                ui.painter(),
                Rect::from_center_size(pos2(filter.max.x - 12.0, filter.center().y), vec2(9.0, 5.0)),
                42,
                custom::MUTED,
            );
            crate::right::menu_popup(&filter_response).show(|ui| {
                if crate::right::menu_item(ui, tr("All libraries"), "") {
                    s.filter.clear();
                    ui.close();
                }
                if crate::right::menu_item(ui, tr("This file"), "") {
                    s.filter = "_local".to_string();
                    ui.close();
                }
                for lib in crate::variable_ui::libraries(app, &doc) {
                    if crate::right::menu_item(ui, &lib.manifest.name, "") {
                        s.filter = lib.manifest.id.clone();
                        ui.close();
                    }
                }
            });
            if icon_button(ui, custom::at(o, 200.0, 88.0, 24.0, 24.0), "cpl_grid", 43, vec2(14.0, 14.0), tr("Grid/list view")) {
                s.grid = !s.grid;
            }
            ui.painter().hline(o.x..=o.x + 240.0, o.y + 123.5, Stroke::new(1.0, custom::BORDER));
            let list_rect = custom::at(o, 0.0, 124.0, 240.0, 303.0);
            ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
                egui::ScrollArea::vertical().id_salt("color_library_scroll").max_height(303.0).show(ui, |ui| {
                    ui.set_width(240.0);
                    if rows.is_empty() {
                        ui.label(tr("No matching colors"));
                    }
                    for (group, entries) in &rows {
                        let shared_root = entries
                            .first()
                            .and_then(|first| first.name.split_once('/').map(|(root, _)| root))
                            .filter(|root| entries.iter().all(|entry| entry.name.starts_with(&format!("{root}/"))));
                        let title = shared_root.map_or_else(|| group.clone(), |root| format!("{group} / {root}"));
                        ui.add_space(9.0);
                        ui.label(egui::RichText::new(title).size(11.0).color(custom::MUTED));
                        ui.add_space(5.0);
                        if s.grid {
                            ui.horizontal_wrapped(|ui| {
                                for swatch in entries {
                                    let (r, response) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                                    ui.painter().rect_filled(r.shrink(3.0), CornerRadius::same(5), doc_c32(ctx, swatch.color));
                                    if response.on_hover_text(&swatch.name).clicked() {
                                        picked = Some(swatch.clone());
                                    }
                                }
                            });
                        } else {
                            let mut previous_folder = String::new();
                            for swatch in entries {
                                let short = shared_root.and_then(|root| swatch.name.strip_prefix(&format!("{root}/"))).unwrap_or(&swatch.name);
                                let (folder, label) = short.rsplit_once('/').unwrap_or(("", short));
                                if !folder.is_empty() && folder != previous_folder {
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new(folder).size(11.0).color(custom::MUTED));
                                    previous_folder = folder.to_string();
                                }
                                let (r, response) = ui.allocate_exact_size(vec2(240.0, 28.0), Sense::click());
                                if response.hovered() {
                                    ui.painter().rect_filled(r, CornerRadius::same(5), custom::FIELD);
                                }
                                ui.painter().rect_filled(
                                    Rect::from_min_size(r.min + vec2(12.0, 5.0), vec2(18.0, 18.0)),
                                    CornerRadius::same(4),
                                    doc_c32(ctx, swatch.color),
                                );
                                ui.painter().text(r.min + vec2(39.0, 14.0), Align2::LEFT_CENTER, label, regular(11.0), Color32::WHITE);
                                if response.clicked() {
                                    picked = Some(swatch.clone());
                                }
                            }
                        }
                    }
                });
            });
        });
    if custom {
        s.active = false;
        s.create = None;
    }
    if close {
        app.ui.picker = None;
        s.active = false;
        s.create = None;
    }
    if let Some(swatch) = picked {
        match (&target, &swatch.style, &swatch.variable) {
            (PickerTarget::Paint { fills, ids, .. }, Some((lib, id)), _) => {
                let kind = if *fills { "fill" } else { "stroke" };
                if let Some(lib) = lib {
                    app.run("library.applyStyle", json!({"library": lib, "style": id, "ids": ids, "target": kind}));
                } else {
                    app.run("style.apply", json!({"style": id, "ids": ids, "target": kind}));
                }
            }
            (PickerTarget::Paint { fills, index, ids }, _, Some((lib, id))) => {
                let field = format!("{}/{index}/color", if *fills { "fills" } else { "strokes" });
                if let Some(lib) = lib {
                    app.run("library.bindVariable", json!({"library": lib, "variable": id, "ids": ids, "field": field}));
                } else {
                    app.run("variable.bind", json!({"variable": id, "ids": ids, "field": field}));
                }
            }
            _ => {
                let mut next = paint.clone();
                next.kind = PaintKind::Solid { color: swatch.color.with_alpha(1.0) };
                next.opacity = swatch.color.a;
                write(app, target, &next, false);
            }
        }
        app.ui.picker = None;
        s.active = false;
        s.create = None;
    }
    if let Some(kind) = s.create {
        let library_pos = crate::primitives::FloatingPanel::position(ctx, Id::new("color_picker"), pos);
        creation_panel(app, ctx, &doc, library_pos, &mut s, kind);
    }
    save(ctx, s);
}

fn creation_panel(app: &mut FigApp, ctx: &egui::Context, doc: &figcraft_doc::Document, library_pos: Pos2, s: &mut State, kind: CreateKind) {
    let count = s.paints.len().max(1);
    let height = if kind == CreateKind::Style { 390.0 + (count - 1) as f32 * 32.0 } else { 170.0 };
    let x = (library_pos.x - 312.0).max(ctx.content_rect().min.x + 8.0);
    let mut cancel = false;
    let mut submit = false;
    let mut remove = None;
    crate::primitives::FloatingPanel::new(Id::new("color_library_create"), pos2(x, library_pos.y))
        .width(crate::primitives::FloatingPanelWidth::Regular)
        .height(crate::primitives::FloatingPanelHeight::Fixed(height))
        .fill(custom::PANEL)
        .show_with_header(ctx, |ui, header| {
            let panel = Rect::from_min_size(header.min, vec2(304.0, height));
            ui.allocate_exact_size(vec2(304.0, height - 40.0), Sense::hover());
            ui.painter().rect_filled(panel, CornerRadius::same(13), custom::PANEL);
            let o = panel.min;
            let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(o + vec2(x, y), vec2(w, h));
            ui.painter().hline(o.x..=o.x + 304.0, o.y + 39.5, Stroke::new(1.0, custom::BORDER));
            if custom::label_tab(ui, at(8.0, 8.0, 45.35, 24.0), "color_create_style_tab", tr("Style"), kind == CreateKind::Style) {
                s.create = Some(CreateKind::Style);
                if matches!(app.ui.picker, Some(PickerTarget::DraftVariable)) {
                    app.ui.picker = None;
                }
                if s.paints.is_empty() {
                    let color = Color::from_hex_or_name(&s.color).unwrap_or(Color::BLACK);
                    s.paints.push(Paint::solid(color));
                }
            }
            if custom::label_tab(ui, at(57.35, 8.0, 58.5, 24.0), "color_create_variable_tab", tr("Variable"), kind == CreateKind::Variable) {
                s.create = Some(CreateKind::Variable);
                if matches!(app.ui.picker, Some(PickerTarget::DraftStyle { .. })) {
                    app.ui.picker = None;
                }
            }
            let close = at(272.0, 8.0, 24.0, 24.0);
            if icon_button(ui, close, "color_create_close", if kind == CreateKind::Style { 45 } else { 44 }, vec2(10.0, 10.0), tr("Close")) {
                cancel = true;
            }
            if kind == CreateKind::Style {
                let preview = at(16.0, 56.0, 272.0, 106.0);
                crate::style_picker::style_paint_preview(ui, preview, &creation_style_paints(s));
            }
            let name_y = if kind == CreateKind::Style { 178.0 } else { 56.0 };
            let input_x = if kind == CreateKind::Style { 112.0 } else { 125.0 };
            let input_w = if kind == CreateKind::Style { 176.0 } else { 163.0 };
            ui.painter().text(at(16.0, name_y, 88.0, 24.0).left_center(), Align2::LEFT_CENTER, tr("Name"), regular(11.0), custom::MUTED);
            let name_rect = at(input_x, name_y, input_w, 24.0);
            let mut name_ui = ui.new_child(egui::UiBuilder::new().max_rect(name_rect));
            widgets::text_input(
                &mut name_ui,
                Id::new("color_create_name"),
                &mut s.name,
                input_w,
                if kind == CreateKind::Style { tr("New color style") } else { tr("Color") },
            );
            if kind == CreateKind::Style {
                ui.painter().text(at(16.0, 210.0, 88.0, 24.0).left_center(), Align2::LEFT_CENTER, tr("Description"), regular(11.0), custom::MUTED);
                let description = at(112.0, 210.0, 176.0, 24.0);
                let mut description_ui = ui.new_child(egui::UiBuilder::new().max_rect(description));
                widgets::text_input(&mut description_ui, Id::new("color_create_description"), &mut s.description, 176.0, tr("What is it for?"));
                ui.painter().text(at(16.0, 250.0, 180.0, 40.0).left_center(), Align2::LEFT_CENTER, tr("Properties"), semibold(11.0), Color32::WHITE);
                if crate::primitives::Button::icon(Icon::Plus, tr("Add fill"))
                    .icon_size(14.0)
                    .show_at(ui, Id::new("color_create_add_fill"), at(272.0, 258.0, 24.0, 24.0))
                    .clicked()
                {
                    let color = Color::from_hex_or_name(&s.color).unwrap_or(Color::BLACK);
                    s.paints.push(Paint::solid(color));
                }
                for index in 0..s.paints.len() {
                    let row_y = 290.0 + index as f32 * 32.0;
                    let fill = at(16.0, row_y + 4.0, 220.0, 24.0);
                    let paint = &s.paints[index];
                    let hex = paint.solid_color().map(Color::to_hex).unwrap_or_default();
                    let label = paint.label();
                    let value = if paint.solid_color().is_some() {
                        crate::primitives::ColorValue::EditableHex(&hex)
                    } else {
                        crate::primitives::ColorValue::Label(&label)
                    };
                    let change =
                        crate::primitives::ColorControl::new(Id::new(("color_create_paint", index)), paint, value, f64::from(paint.opacity) * 100.0)
                            .show_at(ui, fill);
                    if change.open_picker {
                        app.ui.picker = Some(PickerTarget::DraftStyle { index });
                        s.active = false;
                    }
                    if let Some(hex) = change.edited_hex
                        && let Some(color) = Color::from_hex_or_name(&hex)
                    {
                        s.paints[index].kind = PaintKind::Solid { color: color.with_alpha(1.0) };
                        if index == 0 {
                            s.color = color.to_hex();
                        }
                    }
                    if let Some(edit) = change.opacity_edit {
                        let value = match edit {
                            widgets::FieldEdit::Set(value) | widgets::FieldEdit::Scrub(value, _) => value,
                        };
                        s.paints[index].opacity = (value / 100.0).clamp(0.0, 1.0) as f32;
                    }
                    if crate::primitives::Button::icon(if s.paints[index].visible { Icon::Eye } else { Icon::EyeOff }, tr("Toggle visibility"))
                        .icon_size(14.0)
                        .show_at(ui, Id::new(("color_create_visibility", index)), at(244.0, row_y + 4.0, 24.0, 24.0))
                        .clicked()
                    {
                        s.paints[index].visible = !s.paints[index].visible;
                    }
                    if crate::primitives::Button::icon(Icon::Minus, tr("Remove fill"))
                        .icon_size(14.0)
                        .show_at(ui, Id::new(("color_create_remove_fill", index)), at(272.0, row_y + 4.0, 24.0, 24.0))
                        .clicked()
                    {
                        remove = Some(index);
                    }
                }
            } else {
                ui.painter().text(at(16.0, 88.0, 88.0, 24.0).left_center(), Align2::LEFT_CENTER, tr("Value"), regular(11.0), custom::MUTED);
                let value = at(125.0, 90.0, 163.0, 24.0);
                let color = Color::from_hex_or_name(&s.color).unwrap_or(Color::BLACK);
                let opacity = s.opacity.parse::<f64>().unwrap_or(100.0).clamp(0.0, 100.0);
                let mut paint = Paint::solid(color.with_alpha(1.0));
                paint.opacity = (opacity / 100.0) as f32;
                let hex = color.to_hex();
                let change = crate::primitives::ColorControl::new(
                    Id::new("color_create_variable_value"),
                    &paint,
                    crate::primitives::ColorValue::EditableHex(&hex),
                    opacity,
                )
                .show_at(ui, value);
                if change.open_picker {
                    app.ui.picker = Some(PickerTarget::DraftVariable);
                    s.active = false;
                }
                if let Some(hex) = change.edited_hex
                    && let Some(next) = Color::from_hex_or_name(&hex)
                {
                    s.color = next.with_alpha(1.0).to_css_hex();
                    if next.a < 1.0 {
                        s.opacity = format!("{:.2}", next.a * 100.0);
                    }
                }
                if let Some(edit) = change.opacity_edit {
                    let percent = match edit {
                        widgets::FieldEdit::Set(value) | widgets::FieldEdit::Scrub(value, _) => value,
                    };
                    s.opacity = format!("{:.2}", percent.clamp(0.0, 100.0));
                }
            }
            let footer_y = height - 56.0;
            let button_w = if kind == CreateKind::Style { 94.0 } else { 112.0 };
            let create = at(304.0 - 16.0 - button_w, footer_y + 16.0, button_w, 24.0);
            let enabled = !s.name.trim().is_empty()
                && if kind == CreateKind::Style {
                    !s.paints.is_empty()
                } else {
                    Color::from_hex_or_name(&s.color).is_some() && s.opacity.parse::<f32>().is_ok()
                };
            if crate::primitives::Button::new(if kind == CreateKind::Style { tr("Create style") } else { tr("Create variable") })
                .enabled(enabled)
                .show_at(ui, Id::new("color_create_submit"), create)
                .clicked()
            {
                submit = true;
            }
        });
    if let Some(index) = remove
        && index < s.paints.len()
    {
        s.paints.remove(index);
        if matches!(app.ui.picker, Some(PickerTarget::DraftStyle { .. })) {
            app.ui.picker = None;
        }
    }
    if cancel {
        s.create = None;
        if matches!(app.ui.picker, Some(PickerTarget::DraftStyle { .. } | PickerTarget::DraftVariable)) {
            app.ui.picker = None;
        }
    }
    if submit {
        let result = match kind {
            CreateKind::Style => app.run(
                "style.create",
                json!({"type":"paint", "name": s.name.trim(), "description": s.description.trim(), "value": creation_style_paints(s)}),
            ),
            CreateKind::Variable => {
                let color = Color::from_hex_or_name(&s.color).unwrap_or(Color::BLACK);
                let opacity = s.opacity.parse::<f32>().unwrap_or(100.0);
                let color = color.with_alpha((opacity / 100.0).clamp(0.0, 1.0));
                let collection = if s.collection.is_empty() {
                    doc.variables.collections.iter().find(|c| c.extends.is_none()).map(|c| c.id.clone()).or_else(|| {
                        app.run("variable.createCollection", json!({"name": tr("Colors")})).and_then(|v| v["id"].as_str().map(str::to_string))
                    })
                } else {
                    Some(s.collection.clone())
                };
                collection.and_then(|collection| {
                    app.run("variable.create", json!({"collection": collection, "name": s.name.trim(), "type":"color", "value": color.to_css_hex()}))
                })
            }
        };
        if result.is_some() {
            s.create = None;
            if matches!(app.ui.picker, Some(PickerTarget::DraftStyle { .. } | PickerTarget::DraftVariable)) {
                app.ui.picker = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_rows_save_as_bottom_first_without_losing_fill_properties() {
        let mut top = Paint::solid(Color::from_hex("#FFFFFF").unwrap());
        top.opacity = 0.62;
        let bottom = Paint::solid(Color::from_hex("#A22B2B").unwrap());
        let state = State { paints: vec![top.clone(), bottom.clone()], ..State::default() };
        let stored = creation_style_paints(&state);
        assert_eq!(stored, vec![bottom, top]);
        assert_eq!(stored[1].opacity, 0.62);
    }
}
