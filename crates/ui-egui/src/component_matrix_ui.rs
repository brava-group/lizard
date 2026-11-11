//! Component-local value tables. The Design panel shows compact axis rows; the Workbench
//! floating panel gives their values room to form a real editable table.

use std::collections::BTreeMap;

use egui::{Align2, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use lizard_doc::{ComponentAxis, ComponentMatrix, Document, NodeId, NodeKind};
use lizard_engine::components::inside_instance;
use serde_json::{Value, json};

use crate::LizardApp;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::primitives::{Button, ButtonVariant, InlineField, PropertyController, PropertyControllerKind, Select, Surface};
use crate::right::{menu_item, menu_popup};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets;

const SIDEBAR_W: f32 = 230.0;
const LABEL_W: f32 = 196.0;
const CELL_W: f32 = 154.0;
const ROW_H: f32 = 36.0;
const FIELDS: &[&str] = &[
    "fill",
    "fills",
    "stroke",
    "strokes",
    "strokeWeight",
    "cornerRadius",
    "opacity",
    "visible",
    "width",
    "height",
    "padding",
    "gap",
    "fontSize",
    "fontFamily",
    "fontStyle",
    "lineHeight",
    "letterSpacing",
    "characters",
    "effects",
];

#[derive(Clone, Default)]
struct Editor {
    axis: String,
    filters: BTreeMap<String, String>,
    add_axis: bool,
    edit_options: bool,
    name: String,
    options: String,
    default: String,
    layer: Option<NodeId>,
    field: String,
    value: String,
    adding_row: bool,
}

fn editor_id(main: NodeId) -> Id {
    Id::new(("component_value_editor", main))
}

fn parse_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

fn value_text(value: &Value) -> String {
    value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())
}

pub(crate) fn show(app: &mut LizardApp, ui: &mut Ui, doc: &Document, main: NodeId) {
    let Some(node) = doc.node(main).filter(|node| matches!(node.kind, NodeKind::Component { .. })) else { return };
    let matrix = &node.component_matrix;
    let id = editor_id(main);
    let ctx = ui.ctx().clone();
    let mut state = ctx.data(|data| data.get_temp::<Editor>(id.with("state"))).unwrap_or_default();
    if !matrix.axes.iter().any(|axis| axis.name == state.axis) {
        state.axis = matrix.axes.first().map(|axis| axis.name.clone()).unwrap_or_default();
    }
    state.filters.retain(|name, option| matrix.axes.iter().any(|axis| axis.name == *name && axis.options.contains(option)));
    widgets::separator(ui);
    let mut clicked = widgets::section_header(ui, tr("Component values"), &[(Icon::Grid, tr("Edit value table"))], matrix.axes.is_empty()).is_some();
    for axis in &matrix.axes {
        let summary = format!("{} values", axis.options.len());
        let response = PropertyController::new(PropertyControllerKind::Variant, &axis.name).value(&summary).show(ui);
        if let Some(name) = response.renamed.as_deref() {
            app.run("component.matrixRenameAxis", json!({"id":main,"name":axis.name,"newName":name}));
        }
        if response.delete_clicked {
            app.run("component.matrixDeleteAxis", json!({"id":main,"name":axis.name}));
        }
        if response.settings_clicked {
            state.axis = axis.name.clone();
            clicked = true;
        }
    }
    if matrix.axes.is_empty() {
        widgets::note(ui, tr("Add an axis to set component values without drawing every combination."), Tokens::get(ui.ctx()).text_secondary);
    }
    let was_open = ctx.data(|data| data.get_temp::<bool>(id.with("open"))).unwrap_or(false);
    let mut open = was_open ^ clicked;
    if open {
        let t = Tokens::get(&ctx);
        let screen = ctx.content_rect();
        let size = vec2(1010.0_f32.min(screen.width() - 32.0), 510.0_f32.min(screen.height() - 32.0));
        egui::Window::new(tr("Component values"))
            .id(id)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size(size)
            .pivot(Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .frame(Surface::palette().frame(t).fill(t.panel).inner_margin(egui::Margin::same(0)))
            .show(&ctx, |ui| {
                ui.set_min_size(size);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                window_header(ui, &mut open, id);
                editor_contents(app, ui, doc, main, matrix, &mut state, id, size);
                Surface::palette().paint_top_highlight(ui.painter(), ui.max_rect());
            });
    }
    ctx.data_mut(|data| data.insert_temp(id.with("open"), open));
    ctx.data_mut(|data| data.insert_temp(id.with("state"), state));
}

fn window_header(ui: &mut Ui, open: &mut bool, id: Id) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, tr("Component values"), semibold(11.0), t.text);
    let close = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, id.with("close"), close).clicked() {
        *open = false;
    }
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.border));
    ui.painter().line_segment([pos2(rect.left() + SIDEBAR_W, rect.top()), pos2(rect.left() + SIDEBAR_W, rect.bottom())], Stroke::new(1.0, t.border));
}

fn editor_contents(
    app: &mut LizardApp,
    ui: &mut Ui,
    doc: &Document,
    main: NodeId,
    matrix: &ComponentMatrix,
    state: &mut Editor,
    id: Id,
    size: egui::Vec2,
) {
    let t = Tokens::get(ui.ctx());
    let height = size.y - 40.0;
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(SIDEBAR_W, height), egui::Layout::top_down(egui::Align::Min), |ui| {
            axis_sidebar(app, ui, main, matrix, state, id)
        });
        let (line, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
        ui.painter().rect_filled(line, CornerRadius::ZERO, t.border);
        ui.allocate_ui_with_layout(vec2(size.x - SIDEBAR_W - 1.0, height), egui::Layout::top_down(egui::Align::Min), |ui| {
            table_view(app, ui, doc, main, matrix, state, id);
        });
    });
}

fn axis_sidebar(app: &mut LizardApp, ui: &mut Ui, main: NodeId, matrix: &ComponentMatrix, state: &mut Editor, id: Id) {
    let t = Tokens::get(ui.ctx());
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(head.left() + 16.0, head.center().y), Align2::LEFT_CENTER, tr("Axes"), semibold(11.0), t.text);
    let add = Rect::from_center_size(pos2(head.right() - 20.0, head.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Plus, tr("Add axis")).icon_size(14.0).show_at(ui, id.with("add_axis"), add).clicked() {
        state.add_axis = true;
        state.edit_options = false;
        state.name.clear();
        state.options.clear();
        state.default.clear();
    }
    for axis in &matrix.axes {
        let (row, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        let body = row.shrink2(vec2(10.0, 4.0));
        let selected = state.axis == axis.name;
        if selected || response.hovered() {
            ui.painter().rect_filled(body, CornerRadius::same(5), if selected { t.selected_row } else { t.hover });
        }
        ui.painter().text(
            pos2(body.left() + 8.0, body.center().y),
            Align2::LEFT_CENTER,
            &axis.name,
            if selected { semibold(11.0) } else { regular(11.0) },
            t.text,
        );
        ui.painter().text(
            pos2(body.right() - 8.0, body.center().y),
            Align2::RIGHT_CENTER,
            axis.options.len().to_string(),
            regular(11.0),
            t.text_secondary,
        );
        if response.clicked() {
            state.axis = axis.name.clone();
            state.edit_options = false;
        }
        crate::right::context_popup(&response).show(|ui| {
            if menu_item(ui, tr("Edit axis"), "") {
                state.axis = axis.name.clone();
                state.name = axis.name.clone();
                state.options = axis.options.join(", ");
                state.default = axis.default.clone();
                state.edit_options = true;
                state.add_axis = false;
                ui.close();
            }
            if menu_item(ui, tr("Delete axis"), "") {
                app.run("component.matrixDeleteAxis", json!({"id":main,"name":axis.name}));
                ui.close();
            }
        });
    }
    if matrix.axes.is_empty() {
        ui.add_space(16.0);
        widgets::note(ui, tr("Add Size, Style or State to reuse one component."), t.text_secondary);
    }
    ui.add_space(10.0);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line, CornerRadius::ZERO, t.border);
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(head.left() + 16.0, head.center().y), Align2::LEFT_CENTER, tr("Context"), semibold(11.0), t.text);
    for axis in matrix.axes.iter().filter(|axis| axis.name != state.axis) {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(egui::RichText::new(&axis.name).font(regular(11.0)).color(t.text_secondary));
        });
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let current = state.filters.get(&axis.name).map(String::as_str).unwrap_or(tr("Any"));
            let select = Select::trigger(ui, current, SIDEBAR_W - 32.0, None);
            menu_popup(&select).show(|ui| {
                if menu_item(ui, tr("Any"), "") {
                    state.filters.remove(&axis.name);
                    ui.close();
                }
                for option in &axis.options {
                    if menu_item(ui, option, "") {
                        state.filters.insert(axis.name.clone(), option.clone());
                        ui.close();
                    }
                }
            });
        });
        ui.add_space(8.0);
    }
}

fn table_view(app: &mut LizardApp, ui: &mut Ui, doc: &Document, main: NodeId, matrix: &ComponentMatrix, state: &mut Editor, id: Id) {
    let t = Tokens::get(ui.ctx());
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let current = matrix.axes.iter().find(|axis| axis.name == state.axis);
    ui.painter().text(
        pos2(head.left() + 16.0, head.center().y),
        Align2::LEFT_CENTER,
        current.map_or(tr("Choose an axis"), |axis| &axis.name),
        semibold(11.0),
        t.text,
    );
    if let Some(axis) = current {
        let edit = Rect::from_center_size(pos2(head.right() - 20.0, head.center().y), vec2(24.0, 24.0));
        if Button::icon(Icon::Adjust, tr("Edit axis options")).icon_size(14.0).show_at(ui, id.with("edit_axis"), edit).clicked() {
            state.name = axis.name.clone();
            state.options = axis.options.join(", ");
            state.default = axis.default.clone();
            state.edit_options = true;
            state.add_axis = false;
        }
    }
    ui.painter().line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, t.border));
    if state.add_axis || state.edit_options {
        axis_form(app, ui, main, state, id);
        ui.add_space(10.0);
    }
    let Some(axis) = current else {
        ui.add_space(24.0);
        widgets::note(ui, tr("Create an axis to add values for each option."), t.text_secondary);
        return;
    };
    let footer_h = if state.adding_row { 76.0 } else { 41.0 };
    let table_h = (ui.available_height() - footer_h).max(80.0);
    egui::ScrollArea::both().id_salt(id.with("table")).auto_shrink([false, false]).max_height(table_h).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let cell_width = ((ui.available_width() - LABEL_W) / axis.options.len() as f32).max(CELL_W);
        let width = LABEL_W + cell_width * axis.options.len() as f32;
        ui.set_min_width(width);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            table_header(ui, tr("Layer · Field"), LABEL_W, t);
            for option in &axis.options {
                table_header(ui, option, cell_width, t);
            }
        });
        for binding in &matrix.fields {
            let layer_name = doc.node(binding.layer).map(|node| node.name.as_str()).unwrap_or(tr("Missing layer"));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                table_label(ui, &format!("{layer_name} · {}", binding.field), LABEL_W, t);
                for option in &axis.options {
                    table_value(app, ui, main, matrix, binding, axis, option, &state.filters, cell_width, id, t);
                }
            });
        }
    });
    let (footer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 41.0), Sense::hover());
    ui.painter().line_segment([footer.left_top(), footer.right_top()], Stroke::new(1.0, t.border));
    let add = Rect::from_min_size(pos2(footer.left() + 8.0, footer.top() + 8.0), vec2(140.0, 24.0));
    if Button::new(tr("Add property")).variant(ButtonVariant::Ghost).leading(Icon::Plus).show_at(ui, id.with("add_row"), add).clicked() {
        state.adding_row = !state.adding_row;
    }
    if state.adding_row {
        add_row(app, ui, doc, main, axis, state, id);
    }
}

fn axis_form(app: &mut LizardApp, ui: &mut Ui, main: NodeId, state: &mut Editor, id: Id) {
    ui.add_space(12.0);
    let options: Vec<String> = state.options.split(',').map(str::trim).filter(|part| !part.is_empty()).map(str::to_string).collect();
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        widgets::text_input(ui, id.with("axis_name"), &mut state.name, 148.0, tr("Axis name"));
        ui.add_space(8.0);
        widgets::text_input(ui, id.with("axis_options"), &mut state.options, 340.0, tr("Small, Medium, Large"));
        ui.add_space(8.0);
        if Button::new(tr("Save")).variant(ButtonVariant::Primary).show(ui).clicked() && !state.name.trim().is_empty() && !options.is_empty() {
            let default = options.iter().find(|option| *option == &state.default).unwrap_or(&options[0]);
            app.run("component.matrixSetAxis", json!({"id":main,"name":state.name,"options":options,"default":default}));
            state.axis = state.name.clone();
            state.add_axis = false;
            state.edit_options = false;
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(egui::RichText::new(tr("Default")).font(regular(11.0)).color(Tokens::get(ui.ctx()).text_secondary));
        ui.add_space(8.0);
        let selected =
            options.iter().find(|option| *option == &state.default).or_else(|| options.first()).map(String::as_str).unwrap_or(tr("Add options"));
        let select = Select::trigger(ui, selected, 148.0, None);
        menu_popup(&select).show(|ui| {
            for option in &options {
                if menu_item(ui, option, "") {
                    state.default = option.clone();
                    ui.close();
                }
            }
        });
    });
}

fn table_header(ui: &mut Ui, title: &str, width: f32, t: Tokens) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
    table_lines(ui, rect, t);
    ui.painter().text(pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, title, semibold(11.0), t.text);
}

fn table_lines(ui: &Ui, rect: Rect, t: Tokens) {
    let stroke = Stroke::new(1.0, t.border);
    ui.painter().line_segment([rect.right_top(), rect.right_bottom()], stroke);
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
}

fn table_label(ui: &mut Ui, title: &str, width: f32, t: Tokens) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.hover);
    }
    table_lines(ui, rect, t);
    let shown = widgets::elide_font(ui, title, width - 24.0, regular(11.0));
    ui.painter().text(pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, shown, regular(11.0), t.text);
    response.on_hover_text(title);
}

#[allow(clippy::too_many_arguments)]
fn table_value(
    app: &mut LizardApp,
    ui: &mut Ui,
    main: NodeId,
    matrix: &ComponentMatrix,
    binding: &lizard_doc::ComponentField,
    axis: &ComponentAxis,
    option: &str,
    filters: &BTreeMap<String, String>,
    width: f32,
    id: Id,
    t: Tokens,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.hover);
    }
    table_lines(ui, rect, t);
    let mut when = filters.clone();
    when.insert(axis.name.clone(), option.to_string());
    let exact = binding.cells.iter().find(|cell| cell.when == when);
    let mut choices = matrix.choices(&BTreeMap::new());
    choices.extend(when.iter().map(|(key, value)| (key.clone(), value.clone())));
    let resolved = matrix.resolve(binding, &choices);
    let display = exact.map(|cell| &cell.value).or(resolved).map(value_text).unwrap_or_default();
    let edit_rect = Rect::from_min_max(pos2(rect.left() + 8.0, rect.top() + 5.0), pos2(rect.right() - 26.0, rect.bottom() - 5.0));
    if let Some(edited) = InlineField::new(id.with(("cell", binding.layer, &binding.field, &when)), &display).show_at(ui, edit_rect)
        && !edited.trim().is_empty()
    {
        app.run("component.matrixSetCell", json!({"id":main,"layer":binding.layer,"field":binding.field,"when":when,"value":parse_value(&edited)}));
    }
    if exact.is_some() && response.hovered() {
        let delete = Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(22.0, 22.0));
        if Button::icon(Icon::Minus, tr("Delete cell")).show_at(ui, id.with(("delete", binding.layer, &binding.field, &when)), delete).clicked() {
            app.run("component.matrixDeleteCell", json!({"id":main,"layer":binding.layer,"field":binding.field,"when":when}));
        }
    } else if let Some(value) = resolved {
        response.on_hover_text(format!("{}: {}", tr("Inherited value"), value));
    }
}

fn add_row(app: &mut LizardApp, ui: &mut Ui, doc: &Document, main: NodeId, axis: &ComponentAxis, state: &mut Editor, id: Id) {
    let layer = state.layer.filter(|layer| *layer == main || doc.is_ancestor(main, *layer)).unwrap_or(main);
    let field = if state.field.is_empty() { "fill".to_string() } else { state.field.clone() };
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let layer_name = doc.node(layer).map(|node| node.name.as_str()).unwrap_or(tr("Layer"));
        let select = Select::trigger(ui, layer_name, 174.0, None);
        menu_popup(&select).show(|ui| {
            for candidate in doc.subtree(main).into_iter().filter(|candidate| !inside_instance(doc, *candidate)).take(128) {
                if let Some(node) = doc.node(candidate)
                    && menu_item(ui, &format!("{} · {}", node.name, candidate), "")
                {
                    state.layer = Some(candidate);
                    ui.close();
                }
            }
        });
        ui.add_space(8.0);
        let select = Select::trigger(ui, &field, 126.0, None);
        menu_popup(&select).show(|ui| {
            for candidate in FIELDS {
                if menu_item(ui, candidate, "") {
                    state.field = (*candidate).to_string();
                    ui.close();
                }
            }
        });
        ui.add_space(8.0);
        widgets::text_input(ui, id.with("new_value"), &mut state.value, 184.0, tr("Value"));
        ui.add_space(8.0);
        if Button::new(tr("Add row")).variant(ButtonVariant::Secondary).leading(Icon::Plus).show(ui).clicked() && !state.value.trim().is_empty() {
            let mut when = state.filters.clone();
            when.insert(axis.name.clone(), axis.default.clone());
            app.run("component.matrixSetCell", json!({"id":main,"layer":layer,"field":field,"when":when,"value":parse_value(&state.value)}));
            state.value.clear();
        }
    });
}
