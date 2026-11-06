//! The Design panel's layout section: auto layout direction and wrap, W/H with their sizing (Fixed /
//! Hug / Fill) and min/max limits, alignment (including text baselines), fixed or auto gaps,
//! padding, strokes in layout and canvas stacking, grid tracks, a grid child's cell, spans and
//! alignment in its cell, and clip content. Everything goes through `node.set` props or `layout.*`
//! commands.

use crate::primitives::TooltipResponseExt as _;
use egui::{Align2, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use lizard_doc::*;
use serde_json::json;

use crate::LizardApp;
use crate::a11y::Tip;
use crate::i18n::{tr, tr_noop};
use crate::icons::{self, Icon, LayoutIcon, SizingIcon};
use crate::primitives::{Button, ButtonBehavior, ControlWidth, Menu, MenuItem, Segment, SegmentedControl};
use crate::right::{block, common, menu_item, menu_popup, row, row_rect};
use crate::theme::{Tokens, regular};
use crate::widgets::{self, FIELD_H, Lead};

/// Most tracks listed in the panel (the rest are still laid out; edit them through commands).
const MAX_TRACK_ROWS: usize = 24;

fn parent_of(doc: &Document, id: NodeId) -> Option<&Node> {
    doc.node(id).and_then(|n| n.parent).and_then(|p| doc.node(p))
}

pub fn layout_section(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let ids = sel.to_vec();
    let frames = sel.iter().all(|i| doc.node(*i).is_some_and(|n| n.kind.is_frame_like()));
    let al = common(doc, sel, |n| n.layout.clone());
    let has_al = matches!(al, Some(Some(_)));
    let is_text = !sel.is_empty() && sel.iter().all(|id| doc.node(*id).is_some_and(|n| matches!(n.kind, NodeKind::Text(_))));
    let title = if has_al { tr("Auto layout") } else { tr("Layout") };
    // An instance's auto layout is its main's (only padding, gap and alignment are its own); the
    // layers inside it override their size and resizing, not their place in the layout.
    let in_instance = sel.iter().any(|i| lizard_engine::components::owning_instance(doc, *i).is_some());
    let nested = sel.iter().any(|i| lizard_engine::components::inside_instance(doc, *i));
    let header_action = if has_al && !in_instance {
        layout_header(ui, title, Some((LayoutIcon::AutoLayout, tr("Remove auto layout"))), true)
    } else if frames && !in_instance {
        layout_header(ui, title, Some((LayoutIcon::AutoLayout, tr("Add auto layout"))), false)
    } else {
        layout_header(ui, title, None, false)
    };
    if header_action {
        app.run(if has_al { "layout.remove" } else { "layout.add" }, json!({"ids": ids}));
    }
    let from_main = tr("In an instance, this comes from the main component");
    if let Some(Some(l)) = &al {
        ui.add_enabled_ui(!in_instance, |ui| direction(app, ui, &ids, l)).response.lizard_disabled_tooltip(from_main);
    }
    if is_text {
        let resize = common(doc, sel, |node| match &node.kind {
            NodeKind::Text(text) => text.auto_resize,
            _ => TextAutoResize::None,
        });
        let selected = match resize {
            Some(TextAutoResize::WidthAndHeight) => Some(0),
            Some(TextAutoResize::Height) => Some(1),
            Some(TextAutoResize::None | TextAutoResize::Truncate) => Some(2),
            None => None,
        };
        row(ui, 40.0, |ui| {
            let segments = [
                Segment::layout_icon(LayoutIcon::TextAutoWidth, tr("Auto width")),
                Segment::layout_icon(LayoutIcon::TextAutoHeight, tr("Auto height")),
                Segment::layout_icon(LayoutIcon::TextFixed, tr("Fixed size")),
            ];
            if let Some(picked) = SegmentedControl::new(&segments, selected).width(ControlWidth::Fill).show(ui) {
                let mode = ["widthAndHeight", "height", "none"][picked];
                if picked == 2 {
                    app.set_props(&ids, json!({"textAutoResize": mode}));
                } else {
                    app.set_props(&ids, json!({"textAutoResize": mode, "constrainProportions": false}));
                }
            }
        });
    }
    ui.add_enabled_ui(!nested, |ui| dimensions(app, ui, doc, sel, has_al)).response.lizard_disabled_tooltip(from_main);
    let parent_al = sel.iter().all(|i| parent_of(doc, *i).is_some_and(|p| p.layout.is_some()));
    if parent_al {
        let absolute = common(doc, sel, |n| n.child_layout.absolute).unwrap_or(false);
        row(ui, 32.0, |ui| {
            if icons::button(
                ui,
                Icon::AbsolutePosition,
                24.0,
                absolute,
                if absolute { tr("Use auto layout") } else { tr("Ignore auto layout (absolute position)") },
            )
            .clicked()
            {
                app.set_props(&ids, json!({"absolute": !absolute}));
            }
        });
    }
    if let Some(Some(l)) = &al {
        ui.add_space(8.0);
        alignment(app, ui, &ids, l);
        spacing(app, ui, &ids, l);
        ui.add_enabled_ui(!in_instance, |ui| {
            if l.mode == LayoutMode::Grid && sel.len() == 1 {
                tracks(app, ui, doc, &ids);
            }
        })
        .response
        .lizard_disabled_tooltip(from_main);
    }
    crate::selection_spacing::panel(app, ui, sel);
    let grid_parent = sel.iter().all(|i| parent_of(doc, *i).and_then(|p| p.layout.as_ref()).is_some_and(|l| l.mode == LayoutMode::Grid));
    if grid_parent && common(doc, sel, |n| n.child_layout.absolute) == Some(false) {
        ui.add_enabled_ui(!nested, |ui| grid_cell(app, ui, doc, sel)).response.lizard_disabled_tooltip(from_main);
    }
    if frames {
        ui.add_space(8.0);
        let clip = common(doc, sel, |n| n.clip);
        row(ui, 24.0, |ui| {
            if widgets::checkbox(ui, clip.unwrap_or(false), tr("Clip content")) {
                app.set_props(&ids, json!({"clip": !clip.unwrap_or(false)}));
            }
        });
    }
    ui.add_space(12.0);
}

fn layout_header(ui: &mut Ui, title: &str, action: Option<(LayoutIcon, &str)>, selected: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(rect.min.x + widgets::PAD, rect.center().y), Align2::LEFT_CENTER, title, crate::theme::property_section_title(), t.text);
    let Some((icon, tip)) = action else { return false };
    let button = widgets::section_action_rect(rect, 0);
    let response = ui.interact(button, ui.id().with("layout_header_action"), Sense::click()).tip(tip);
    if selected || response.hovered() {
        ui.painter().rect_filled(button, CornerRadius::same(5), if selected { t.selected_row } else { t.hover });
    }
    icons::paint_layout(
        ui.painter(),
        Rect::from_center_size(button.center(), vec2(18.0, 18.0)),
        if selected { LayoutIcon::AutoLayoutActive } else { icon },
        if selected { t.accent_text } else { t.icon },
    );
    response.clicked()
}

fn layout_button(ui: &mut Ui, icon: LayoutIcon, selected: bool, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    paint_layout_button(ui, rect, response, icon, selected, tip)
}

fn layout_button_at(ui: &mut Ui, rect: Rect, icon: LayoutIcon, selected: bool, tip: &str) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(("layout_action", tip)), Sense::click());
    paint_layout_button(ui, rect, response, icon, selected, tip)
}

fn paint_layout_button(ui: &Ui, rect: Rect, response: egui::Response, icon: LayoutIcon, selected: bool, tip: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let response = response.tip(tip);
    if selected || response.hovered() {
        let fill = if selected && icon == LayoutIcon::PaddingIndividual {
            t.selected_row
        } else if selected {
            t.field
        } else {
            t.hover
        };
        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    }
    let artwork_size = if icon == LayoutIcon::Wrap { 16.0 } else { 18.0 };
    let artwork = Rect::from_center_size(rect.center(), vec2(artwork_size, artwork_size));
    let artwork_icon = if selected && icon == LayoutIcon::PaddingIndividual { LayoutIcon::PaddingIndividualActive } else { icon };
    let color = if selected && icon == LayoutIcon::PaddingIndividual {
        t.accent_text
    } else if selected {
        t.icon
    } else {
        t.icon_secondary
    };
    icons::paint_layout(ui.painter(), artwork, artwork_icon, color);
    response
}

fn direction(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], l: &AutoLayout) {
    let direction_row = row_rect(ui, 24.0, |ui| {
        let segments = [
            Segment::layout_icon(LayoutIcon::Free, tr("Free")),
            Segment::layout_icon(LayoutIcon::Vertical, tr("Vertical")),
            Segment::layout_icon(LayoutIcon::Horizontal, tr("Horizontal")),
            Segment::layout_icon(LayoutIcon::Grid, tr("Grid")),
        ];
        let active = match l.mode {
            LayoutMode::Vertical => 1,
            LayoutMode::Horizontal => 2,
            LayoutMode::Grid => 3,
        };
        if let Some(index) = SegmentedControl::new(&segments, Some(active)).width(ControlWidth::Fill).icon_size(16.0).show(ui) {
            if index == 0 {
                app.run("layout.remove", json!({"ids": ids}));
            } else {
                // Wrap carries over between the flows: rows of a horizontal one, columns of a vertical one.
                let mode = ["vertical", "horizontal", "grid"][index - 1];
                app.set_props(ids, json!({"layout": mode}));
            }
        }
    });
    if l.mode != LayoutMode::Grid {
        let resp = layout_button_at(ui, widgets::section_action_rect(direction_row, 0), LayoutIcon::Wrap, l.wrap, tr("Wrap"));
        crate::a11y::toggle(&resp, tr("Wrap"), l.wrap);
        if resp.clicked() {
            app.set_props(ids, json!({"wrap": !l.wrap}));
        }
    }
    ui.add_space(8.0);
}

/// One axis' sizing, min/max keys and labels (the add-min and add-max menu items, the remove
/// button, the min and max field leads).
struct Axis {
    sizing: &'static str,
    min: &'static str,
    max: &'static str,
    add_min: &'static str,
    add_max: &'static str,
    remove: &'static str,
    min_lead: &'static str,
    max_lead: &'static str,
}

const AXES: [Axis; 2] = [
    Axis {
        sizing: "sizingH",
        min: "minWidth",
        max: "maxWidth",
        add_min: tr_noop("Add min width"),
        add_max: tr_noop("Add max width"),
        remove: tr_noop("Remove min and max width"),
        min_lead: tr_noop("Min W"),
        max_lead: tr_noop("Max W"),
    },
    Axis {
        sizing: "sizingV",
        min: "minHeight",
        max: "maxHeight",
        add_min: tr_noop("Add min height"),
        add_max: tr_noop("Add max height"),
        remove: tr_noop("Remove min and max height"),
        min_lead: tr_noop("Min H"),
        max_lead: tr_noop("Max H"),
    },
];

fn sizing_menu_option(ui: &mut Ui, label: &str, icon: SizingIcon, selected: bool, enabled: bool) -> bool {
    MenuItem::new(label)
        .checked(selected)
        .enabled(enabled)
        .max_width(ui.available_width())
        .show_with_parts(
            ui,
            24.0,
            |painter, slot, color| {
                icons::paint_sizing(painter, Rect::from_center_size(pos2(slot.left() + 10.0, slot.center().y), vec2(16.0, 16.0)), icon, color);
            },
            0.0,
            |_, _, _| {},
        )
        .clicked()
}

fn sizing_menu_separator(ui: &mut Ui) {
    Menu::separator(ui, None);
}

/// A node's min (`is_min`) or max limit on one axis.
fn limit(n: &Node, horizontal: bool, is_min: bool) -> Option<f64> {
    let cl = &n.child_layout;
    match (horizontal, is_min) {
        (true, true) => cl.min_w,
        (true, false) => cl.max_w,
        (false, true) => cl.min_h,
        (false, false) => cl.max_h,
    }
}

fn dimensions(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId], has_al: bool) {
    let ids = sel.to_vec();
    let w_of = |n: &Node| (n.width * 100.0).round() / 100.0;
    let h_of = |n: &Node| (n.height * 100.0).round() / 100.0;
    let (w, h) = (common(doc, sel, w_of), common(doc, sel, h_of));
    let col = widgets::col_width(ui.available_width());
    let parent_al = sel.iter().all(|i| parent_of(doc, *i).is_some_and(|p| p.layout.is_some()));
    let is_text = sel.iter().all(|i| matches!(doc.node(*i).map(|n| &n.kind), Some(NodeKind::Text(_))));
    let sizing_available = has_al || parent_al || is_text;
    let can_fill = parent_al && sel.iter().all(|i| doc.node(*i).is_some_and(|n| !n.child_layout.absolute));
    let can_limit = has_al || can_fill;
    let first = sel.first().and_then(|i| doc.node(*i));
    row(ui, 24.0, |ui| {
        for (ax, (field, value)) in [("width", w), ("height", h)].into_iter().enumerate() {
            let horizontal = ax == 0;
            let id = Id::new(if horizontal { "lay_w" } else { "lay_h" });
            let lead = Lead::Text(match (is_text, horizontal) {
                (true, true) => tr("W"),
                (true, false) => tr("H"),
                (false, true) => "W",
                (false, false) => "H",
            });
            if sizing_available {
                let axis = &AXES[ax];
                let cur = common(doc, sel, |n| if horizontal { n.child_layout.sizing_h } else { n.child_layout.sizing_v });
                let label = match cur {
                    Some(Sizing::Fixed) => "",
                    Some(Sizing::Hug) => tr("Hug"),
                    Some(Sizing::Fill) => tr("Fill|sizing"),
                    None => tr("Mixed"),
                };
                let (edit, selector) = crate::variable_ui::dimension_field(app, ui, &ids, field, id, lead, value, col, label);
                if horizontal {
                    app.field_of(&ids, field, edit, w_of, |v| json!(v.max(0.0)));
                } else {
                    app.field_of(&ids, field, edit, h_of, |v| json!(v.max(0.0)));
                }
                crate::a11y::combo(&selector, field, label, crate::a11y::menu_open(&selector));
                let current = first.map(|n| if horizontal { n.width } else { n.height }).unwrap_or(0.0);
                let (fixed_icon, hug_icon, fill_icon, min_icon, max_icon) = if horizontal {
                    (SizingIcon::Fixed, SizingIcon::Hug, SizingIcon::Fill, SizingIcon::Minimum, SizingIcon::Maximum)
                } else {
                    (SizingIcon::HeightFixed, SizingIcon::HeightHug, SizingIcon::HeightFill, SizingIcon::HeightMinimum, SizingIcon::HeightMaximum)
                };
                let mut anchor = selector.clone();
                anchor.rect = Rect::from_min_max(pos2(selector.rect.right() - col, selector.rect.top()), selector.rect.right_bottom());
                crate::primitives::Select::popup_for(
                    ui,
                    &anchor,
                    match cur {
                        Some(Sizing::Hug) => 1,
                        Some(Sizing::Fill) => 2,
                        _ => 0,
                    },
                    3,
                    (if horizontal { 215.711_f32 } else { 209.047_f32 }).max(col),
                    crate::primitives::SelectPosition::BottomLeft,
                )
                .show(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let fixed = if horizontal {
                        format!("{} ({})", tr("Fixed width"), widgets::fmt_num(current))
                    } else {
                        format!("{} ({})", tr("Fixed height"), widgets::fmt_num(current))
                    };
                    if sizing_menu_option(ui, &fixed, fixed_icon, cur == Some(Sizing::Fixed), true) {
                        app.set_props(&ids, json!({axis.sizing: "fixed"}));
                        ui.close();
                    }
                    if sizing_menu_option(ui, tr("Hug contents"), hug_icon, cur == Some(Sizing::Hug), has_al || is_text) {
                        app.set_props(&ids, json!({axis.sizing: "hug"}));
                        ui.close();
                    }
                    if sizing_menu_option(ui, tr("Fill container"), fill_icon, cur == Some(Sizing::Fill), can_fill) {
                        app.set_props(&ids, json!({axis.sizing: "fill"}));
                        ui.close();
                    }
                    sizing_menu_separator(ui);
                    let (min, max) = (first.and_then(|n| limit(n, horizontal, true)), first.and_then(|n| limit(n, horizontal, false)));
                    if sizing_menu_option(ui, tr(axis.add_min), min_icon, false, can_limit && min.is_none()) {
                        app.set_props(&ids, json!({axis.min: current}));
                        ui.close();
                    }
                    if sizing_menu_option(ui, tr(axis.add_max), max_icon, false, can_limit && max.is_none()) {
                        app.set_props(&ids, json!({axis.max: current}));
                        ui.close();
                    }
                    sizing_menu_separator(ui);
                    let apply_variable = format!("{}…", tr("Apply variable"));
                    if sizing_menu_option(ui, &apply_variable, SizingIcon::Variable, false, true) {
                        crate::variable_ui::open(ui, &ids, field, lizard_doc::VariableType::Number, selector.rect.left_bottom());
                        ui.close();
                    }
                });
            } else {
                let edit = crate::variable_ui::number_field_mixed(app, ui, &ids, field, id, lead, value, "", col);
                if horizontal {
                    app.field_of(&ids, field, edit, w_of, |v| json!(v.max(0.0)));
                } else {
                    app.field_of(&ids, field, edit, h_of, |v| json!(v.max(0.0)));
                }
            }
        }
        // Keep the field grid intact; place the trailing action in the same column as section actions.
        ui.add_space(8.0);
        if is_text {
            let locked = common(doc, sel, |n| n.constrain_proportions).unwrap_or(false);
            let toggle = Button::layout_icon(LayoutIcon::TextAspectRatio, tr("Constrain proportions"))
                .icon_size(24.0)
                .behavior(ButtonBehavior::Toggle { selected: locked })
                .show(ui);
            if toggle.clicked() {
                if locked {
                    app.set_props(&ids, json!({"constrainProportions": false}));
                } else {
                    app.set_props(&ids, json!({"constrainProportions": true, "textAutoResize": "none"}));
                }
            }
        } else {
            let locked = common(doc, sel, |n| n.constrain_proportions).unwrap_or(false);
            let lock = layout_button(ui, LayoutIcon::AspectRatio, locked, tr("Constrain proportions"));
            crate::a11y::toggle(&lock, tr("Constrain proportions"), locked);
            if lock.clicked() {
                app.set_props(&ids, json!({"constrainProportions": !locked}));
            }
        }
    });
    if !sizing_available {
        return;
    }
    if can_limit {
        for (ax, axis) in AXES.iter().enumerate() {
            limit_row(app, ui, doc, sel, ax == 0, axis, col);
        }
    }
}

/// The min/max fields of one axis (shown once either is set): min in the first column, max in the
/// second, an empty slot offers to add the missing one; the button removes both.
fn limit_row(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId], horizontal: bool, axis: &Axis, col: f32) {
    let nodes: Vec<&Node> = sel.iter().filter_map(|i| doc.node(*i)).collect();
    let has = |is_min: bool| nodes.iter().any(|n| limit(n, horizontal, is_min).is_some());
    if !has(true) && !has(false) {
        return;
    }
    let ids = sel.to_vec();
    let current = nodes.first().map(|n| if horizontal { n.width } else { n.height }).unwrap_or(0.0);
    row(ui, 24.0, |ui| {
        for (is_min, key) in [(true, axis.min), (false, axis.max)] {
            let (lead, add) = if is_min { (tr(axis.min_lead), tr(axis.add_min)) } else { (tr(axis.max_lead), tr(axis.add_max)) };
            if has(is_min) {
                let v = common(doc, sel, |n| limit(n, horizontal, is_min).map(|v| (v * 100.0).round() / 100.0)).flatten();
                let e = crate::variable_ui::number_field(app, ui, &ids, key, Id::new(("lay_limit", key)), Lead::Text(lead), v, "", col);
                app.field(&ids, key, e, |v| json!(v.max(0.0)));
            } else if add_slot(ui, lead, add, col).clicked() {
                app.set_props(&ids, json!({key: current}));
            }
        }
        if icons::button(ui, Icon::Minus, 24.0, false, tr(axis.remove)).clicked() {
            app.set_props(&ids, json!({axis.min: null, axis.max: null}));
        }
    });
}

/// A quiet placeholder button the size of a field ("+ Max W") with the tooltip `tip`.
fn add_slot(ui: &mut Ui, label: &str, tip: &str, width: f32) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(width, FIELD_H), Sense::click());
    let resp = resp.tip(tip);
    let border = if resp.hovered() { t.field_hover_border } else { t.border };
    ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, border), StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(10.0, 10.0)), Icon::Plus, t.icon_secondary);
    ui.painter().text(pos2(r.min.x + 22.0, r.center().y), Align2::LEFT_CENTER, label, regular(11.0), t.text_secondary);
    resp
}

fn alignment(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], l: &AutoLayout) {
    let t = Tokens::get(ui.ctx());
    let grid_mode = l.mode == LayoutMode::Grid;
    let alignment_block = block(ui, 64.0, 0.0, |ui| {
        // Both controls fill the two property columns. `block` reserves the action slot.
        let column_width = (ui.available_width() - 8.0).max(0.0) / 2.0;
        let (grid, _) = ui.allocate_exact_size(vec2(column_width, 64.0), Sense::hover());
        ui.painter().rect_filled(grid, CornerRadius::same(5), t.field);
        let horizontal = l.mode != LayoutMode::Vertical;
        let cur_main = match l.main_align {
            MainAlign::Min => 0,
            MainAlign::Center => 1,
            MainAlign::Max => 2,
            MainAlign::SpaceBetween | MainAlign::SpaceAround | MainAlign::SpaceEvenly => 9,
        };
        // Wrapped lines spread across the flow light the whole cross axis, like an auto gap does
        // the main one.
        let spread = !grid_mode && l.wrap && l.content_align == ContentAlign::SpaceBetween;
        let cur_cross = match l.cross_align {
            _ if spread => 9,
            CrossAlign::Min | CrossAlign::Baseline => 0,
            CrossAlign::Center => 1,
            CrossAlign::Max => 2,
        };
        for yi in 0..3 {
            for xi in 0..3 {
                let c = pos2(grid.min.x + (xi as f32 + 0.5) * grid.width() / 3.0, grid.min.y + (yi as f32 + 0.5) * grid.height() / 3.0);
                let r = Rect::from_center_size(c, vec2(grid.width() / 3.0, grid.height() / 3.0));
                let resp = ui.interact(r, Id::new(("al_grid", xi, yi)), Sense::click());
                let (main_i, cross_i) = if horizontal { (xi, yi) } else { (yi, xi) };
                let on = (cur_main == main_i || cur_main == 9) && (cur_cross == cross_i || cur_cross == 9);
                if on {
                    if grid_mode {
                        ui.painter().rect_filled(Rect::from_center_size(c, vec2(4.0, 4.0)), CornerRadius::same(1), t.accent);
                    } else {
                        icons::paint_alignment(
                            ui.painter(),
                            Rect::from_center_size(c, vec2(16.0, 16.0)),
                            horizontal,
                            if horizontal { yi } else { xi },
                        );
                    }
                } else {
                    ui.painter().circle_filled(c, if resp.hovered() { 2.5 } else { 1.5 }, if resp.hovered() { t.accent } else { t.text_tertiary });
                }
                if resp.clicked() {
                    let m = ["min", "center", "max"][main_i];
                    let cr = ["min", "center", "max"][cross_i];
                    // An auto gap keeps its mode: the cells pick the cross alignment.
                    let main = if l.main_align.is_auto_gap() && !grid_mode { json!(l.main_align) } else { json!(m) };
                    app.set_props(ids, json!({"mainAlign": main, "crossAlign": cr}));
                }
            }
        }
        ui.allocate_ui_with_layout(vec2(column_width, 64.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            if grid_mode {
                // Grid: gap between columns and between rows.
                let e = crate::variable_ui::number_field(
                    app,
                    ui,
                    ids,
                    "columnGap",
                    Id::new("al_gap"),
                    Lead::LayoutIcon(LayoutIcon::GapVertical, tr("Column gap")),
                    Some(l.gap),
                    "",
                    column_width,
                );
                app.field(ids, "columnGap", e, |v| json!(v.max(0.0)));
                ui.add_space(8.0);
                let e = crate::variable_ui::number_field(
                    app,
                    ui,
                    ids,
                    "rowGap",
                    Id::new("al_rowgap"),
                    Lead::LayoutIcon(LayoutIcon::GapVertical, tr("Row gap")),
                    Some(l.cross_gap),
                    "",
                    column_width,
                );
                app.field(ids, "rowGap", e, |v| json!(v.max(0.0)));
                return;
            }
            let auto = l.main_align.is_auto_gap();
            let (shown, lead) = ((!auto).then_some(l.gap), Lead::LayoutIcon(LayoutIcon::GapVertical, tr("Gap between items")));
            let e = crate::variable_ui::number_field_or(app, ui, ids, "gap", Id::new("al_gap"), shown, column_width, tr("Auto"), lead, "");
            app.field(ids, "gap", e, |v| json!(v));
        });
    });
    let action = widgets::section_action_rect(alignment_block, 0);
    let settings_rect = Rect::from_min_size(pos2(action.left(), alignment_block.top()), vec2(24.0, 24.0));
    let settings = layout_button_at(ui, settings_rect, LayoutIcon::Settings, false, tr("Auto layout settings"));
    menu_popup(&settings).width(210.0).show(|ui| {
        if !grid_mode {
            for (label, mode) in [
                (tr("Fixed"), "min"),
                (tr("Auto (space between)"), "spaceBetween"),
                (tr("Auto (space evenly)"), "spaceEvenly"),
                (tr("Auto (space around)"), "spaceAround"),
            ] {
                if menu_item(ui, label, "") {
                    app.set_props(ids, json!({"mainAlign": mode}));
                    ui.close();
                }
            }
            ui.separator();
        }
        for (label, key, value) in [
            (tr("Included in layout"), "strokesIncluded", true),
            (tr("Excluded from layout"), "strokesIncluded", false),
            (tr("Last on top"), "reverseZ", false),
            (tr("First on top"), "reverseZ", true),
        ] {
            if menu_item(ui, label, "") {
                app.set_props(ids, json!({key: value}));
                ui.close();
            }
        }
    });
    // Text baselines: horizontal stacks only.
    if l.mode == LayoutMode::Horizontal {
        ui.add_space(6.0);
        let on = l.cross_align == CrossAlign::Baseline;
        row(ui, 28.0, |ui| {
            if widgets::checkbox(ui, on, tr("Align text baselines")) {
                app.set_props(ids, json!({"crossAlign": if on { "min" } else { "baseline" }}));
            }
        });
    }
}

/// The Spacing row: the H/V padding pair, or, with Individual padding on, two rows of one field
/// per side (like Individual corners).
fn spacing(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], l: &AutoLayout) {
    let col = widgets::col_width(ui.available_width());
    let [pt, pr, pb, pl] = l.padding;
    let individual_id = Id::new(("individual_padding", ids));
    let mut individual = ui.data(|d| d.get_temp::<bool>(individual_id)).unwrap_or(false);
    ui.add_space(8.0);
    let padding_row = row_rect(ui, 24.0, |ui| {
        if individual {
            padding_side_field(app, ui, ids, "paddingLeft", LayoutIcon::PaddingLeft, tr("Left padding"), pl, col);
            padding_side_field(app, ui, ids, "paddingTop", LayoutIcon::PaddingTop, tr("Top padding"), pt, col);
        } else {
            let e = crate::variable_ui::number_field(
                app,
                ui,
                ids,
                "paddingHorizontal",
                Id::new("al_padh"),
                Lead::LayoutIcon(LayoutIcon::PaddingHorizontal, tr("Horizontal padding")),
                (pl == pr).then_some(pl),
                "",
                col,
            );
            app.field(ids, "paddingHorizontal", e, |v| json!(v.max(0.0)));
            let e = crate::variable_ui::number_field(
                app,
                ui,
                ids,
                "paddingVertical",
                Id::new("al_padv"),
                Lead::LayoutIcon(LayoutIcon::PaddingVertical, tr("Vertical padding")),
                (pt == pb).then_some(pt),
                "",
                col,
            );
            app.field(ids, "paddingVertical", e, |v| json!(v.max(0.0)));
        }
    });
    let toggle =
        layout_button_at(ui, widgets::section_action_rect(padding_row, 0), LayoutIcon::PaddingIndividual, individual, tr("Individual padding"));
    crate::a11y::toggle(&toggle, tr("Individual padding"), individual);
    if toggle.clicked() {
        individual = !individual;
        ui.data_mut(|d| d.insert_temp(individual_id, individual));
    }
    if individual {
        ui.add_space(8.0);
        row(ui, 24.0, |ui| {
            padding_side_field(app, ui, ids, "paddingRight", LayoutIcon::PaddingRight, tr("Right padding"), pr, col);
            padding_side_field(app, ui, ids, "paddingBottom", LayoutIcon::PaddingBottom, tr("Bottom padding"), pb, col);
        });
    }
    if l.mode != LayoutMode::Grid && l.wrap {
        // Wrapped rows (horizontal) or columns (vertical): the gap between them.
        let vertical = l.mode == LayoutMode::Vertical;
        let name = if vertical { tr("Column gap") } else { tr("Row gap") };
        ui.add_space(8.0);
        row(ui, 24.0, |ui| {
            let tip = if vertical { tr("Gap between columns") } else { tr("Gap between rows") };
            let spread = l.content_align == ContentAlign::SpaceBetween;
            let shown = (!spread).then_some(l.cross_gap);
            let e = crate::variable_ui::number_field_or(
                app,
                ui,
                ids,
                "crossGap",
                Id::new("al_crossgap"),
                shown,
                col,
                tr("Auto"),
                Lead::LayoutIcon(LayoutIcon::GapVertical, tip),
                "",
            );
            // A typed gap is a fixed gap.
            app.field_run(e, "node.set", |v| json!({"ids": ids, "props": {"crossGap": v.max(0.0), "contentAlign": "auto"}}));
            let label = if spread { tr("Space between") } else { tr("Fixed gap") };
            let resp = crate::primitives::Select::trigger(ui, label, col, None);
            crate::a11y::combo(&resp, name, label, crate::a11y::menu_open(&resp));
            menu_popup(&resp).show(|ui| {
                for (l, v) in [(tr("Fixed"), "auto"), (tr("Auto (space between)"), "spaceBetween")] {
                    if menu_item(ui, l, "") {
                        app.set_props(ids, json!({"contentAlign": v}));
                        ui.close();
                    }
                }
            });
        });
    }
    if l.mode == LayoutMode::Grid {
        widgets::sublabels(ui, tr("Columns"), Some(tr("Rows")));
        row(ui, 24.0, |ui| {
            let e = widgets::number_field(ui, Id::new("al_cols"), Lead::None, Some(f64::from(l.grid_columns)), "", col, 1.0);
            app.field(ids, "gridColumns", e, |v| json!(v.max(1.0)));
            let e = widgets::number_field(ui, Id::new("al_rows"), Lead::None, Some(f64::from(l.grid_rows)), "", col, 1.0);
            app.field(ids, "gridRows", e, |v| json!(v.max(1.0)));
        });
    }
}

fn padding_side_field(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], key: &str, icon: LayoutIcon, name: &str, value: f64, width: f32) {
    let edit =
        crate::variable_ui::number_field(app, ui, ids, key, Id::new(("al_padding", key)), Lead::LayoutIcon(icon, name), Some(value), "", width);
    app.field(ids, key, edit, |v| json!(v.max(0.0)));
}

/// Track sizes of a single grid frame: one row per column / row with its size and sizing.
fn tracks(app: &mut LizardApp, ui: &mut Ui, doc: &Document, ids: &[NodeId]) {
    let Some(id) = ids.first().copied() else { return };
    let Some(l) = doc.node(id).and_then(|n| n.layout.clone()) else { return };
    let Some(g) = lizard_layout::grid_geometry(doc, id) else { return };
    let col = widgets::col_width(ui.available_width());
    for (axis, specs, ranges) in [("columns", &l.column_tracks, &g.columns), ("rows", &l.row_tracks, &g.rows)] {
        widgets::sublabels(ui, if axis == "columns" { tr("Column sizes") } else { tr("Row sizes") }, None);
        for (i, (a, b)) in ranges.iter().enumerate().take(MAX_TRACK_ROWS) {
            let track = specs.get(i).copied().unwrap_or_default();
            let size = ((b - a) * 100.0).round() / 100.0;
            let base = json!({"ids": [id], "axis": axis, "index": i});
            let with = |sizing: &str, value: Option<f64>| {
                let mut p = base.clone();
                p["sizing"] = json!(sizing);
                if let Some(v) = value {
                    p["value"] = json!(v);
                }
                p
            };
            row(ui, 32.0, |ui| {
                let label = (i + 1).to_string();
                // Fill tracks edit their fr; fixed and hug tracks show (and set) a px size.
                let (value, suffix) = if track.sizing == Sizing::Fill { (track.value, "fr") } else { (size, "") };
                let e = widgets::number_field(
                    ui,
                    Id::new(("al_track", axis, i)),
                    Lead::Text(&label),
                    Some(value),
                    suffix,
                    col,
                    if suffix.is_empty() { 1.0 } else { 0.1 },
                );
                let fill = track.sizing == Sizing::Fill;
                app.field_run(e, "layout.setTrack", |v| with(if fill { "fill" } else { "fixed" }, Some(v.max(0.0))));
                let (label, icon) = match track.sizing {
                    Sizing::Fixed => (tr("Fixed"), Icon::Fixed),
                    Sizing::Hug => (tr("Hug"), Icon::Hug),
                    Sizing::Fill => (tr("Fill|sizing"), Icon::FillContainer),
                };
                let resp = crate::primitives::Select::trigger(ui, label, col, Some(icon));
                menu_popup(&resp).show(|ui| {
                    for (l, v) in [(tr("Fixed"), "fixed"), (tr("Hug contents"), "hug"), (tr("Fill (fr)"), "fill")] {
                        if menu_item(ui, l, "") {
                            app.run("layout.setTrack", with(v, None));
                            ui.close();
                        }
                    }
                });
            });
        }
    }
}

/// A grid child's cell (single selection: column and row, 1-based; auto placement resets them)
/// and its spans.
fn grid_cell(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let ids = sel.to_vec();
    let col = widgets::col_width(ui.available_width());
    if let [id] = sel
        && let Some(cell) =
            parent_of(doc, *id).and_then(|p| lizard_layout::grid_geometry(doc, p.id)).and_then(|g| g.cells.into_iter().find(|c| c.id == *id))
    {
        let anchored = doc.node(*id).is_some_and(|n| n.child_layout.grid_column.is_some() || n.child_layout.grid_row.is_some());
        widgets::sublabels(ui, tr("Column"), Some(tr("Row")));
        row(ui, 32.0, |ui| {
            // Moving on one axis pins the other where it is, so the child doesn't jump.
            let at = |c: f64, r: f64| json!({"ids": [id], "props": {"gridColumn": (c - 1.0).max(0.0), "gridRow": (r - 1.0).max(0.0)}});
            let (c0, r0) = ((cell.column + 1) as f64, (cell.row + 1) as f64);
            let e = widgets::number_field(ui, Id::new("al_cell_col"), Lead::None, Some(c0), "", col, 1.0);
            app.field_run(e, "node.set", |v| at(v, r0));
            let e = widgets::number_field(ui, Id::new("al_cell_row"), Lead::None, Some(r0), "", col, 1.0);
            app.field_run(e, "node.set", |v| at(c0, v));
            if anchored && icons::button(ui, Icon::Reset, 24.0, false, tr("Place automatically")).clicked() {
                app.set_props(&ids, json!({"gridColumn": null, "gridRow": null}));
            }
        });
    }
    let cs = common(doc, sel, |n| f64::from(n.child_layout.column_span()));
    let rs = common(doc, sel, |n| f64::from(n.child_layout.row_span()));
    widgets::sublabels(ui, tr("Column span"), Some(tr("Row span")));
    row(ui, 32.0, |ui| {
        let e = widgets::number_field(ui, Id::new("al_span_c"), Lead::None, cs, "", col, 1.0);
        app.field(&ids, "gridColumnSpan", e, |v| json!(v.max(1.0)));
        let e = widgets::number_field(ui, Id::new("al_span_r"), Lead::None, rs, "", col, 1.0);
        app.field(&ids, "gridRowSpan", e, |v| json!(v.max(1.0)));
    });
    // Alignment in the cell; Auto follows the frame's alignment.
    widgets::sublabels(ui, tr("Horizontal align"), Some(tr("Vertical align")));
    row(ui, 32.0, |ui| {
        let axes = [
            ("gridAlignH", tr("Horizontal align"), common(doc, sel, |n| n.child_layout.grid_align_h), [tr("Left"), tr("Center"), tr("Right")]),
            ("gridAlignV", tr("Vertical align"), common(doc, sel, |n| n.child_layout.grid_align_v), [tr("Top"), tr("Center"), tr("Bottom")]),
        ];
        for (key, name, cur, [min, center, max]) in axes {
            let options = [(None, tr("Auto")), (Some(CellAlign::Min), min), (Some(CellAlign::Center), center), (Some(CellAlign::Max), max)];
            let label = cur.map_or(tr("Mixed"), |c| options.iter().find(|(v, _)| *v == c).map_or("", |(_, l)| *l));
            let resp = crate::primitives::Select::trigger(ui, label, col, None);
            crate::a11y::combo(&resp, name, label, crate::a11y::menu_open(&resp));
            menu_popup(&resp).show(|ui| {
                for (v, l) in options {
                    if menu_item(ui, l, "") {
                        app.set_props(&ids, json!({key: v}));
                        ui.close();
                    }
                }
            });
        }
    });
}
