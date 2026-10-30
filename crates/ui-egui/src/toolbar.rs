//! The floating bottom toolbar (UI3): tool groups with flyouts, comments, actions.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use figcraft_color::{ImageScaleMode, Paint, PaintKind};
use figcraft_doc::NodeId;
use figcraft_engine::ToolId;
use figcraft_engine::vectoredit::VTool;
use serde_json::json;

use crate::FigApp;
use crate::a11y::{self, Nav, Region, Tip};
use crate::i18n::{tr, tr_noop, trf};
use crate::icons::{self, Icon};
use crate::primitives::{Button, ButtonVariant, ControlWidth, Menu, MenuItem, Surface};
use crate::theme::{Tokens, regular};

struct Group {
    key: &'static str,
    /// The flyout button's name for screen readers.
    name: &'static str,
    tools: &'static [(ToolId, Icon)],
}

const GROUPS: &[Group] = &[
    Group {
        key: "move",
        name: tr_noop("Move tools"),
        tools: &[(ToolId::Move, Icon::Move), (ToolId::Hand, Icon::Hand), (ToolId::Scale, Icon::Scale)],
    },
    Group {
        key: "frame",
        name: tr_noop("Frame tools"),
        tools: &[(ToolId::Frame, Icon::Frame), (ToolId::Section, Icon::Section), (ToolId::Slice, Icon::Slice)],
    },
    Group {
        key: "shape",
        name: tr_noop("Shape tools"),
        tools: &[
            (ToolId::Rectangle, Icon::Rectangle),
            (ToolId::Line, Icon::Line),
            (ToolId::Arrow, Icon::Arrow),
            (ToolId::Ellipse, Icon::Ellipse),
            (ToolId::Polygon, Icon::Polygon),
            (ToolId::Star, Icon::Star),
        ],
    },
    Group {
        key: "pen",
        name: tr_noop("Drawing tools"),
        tools: &[(ToolId::Pen, Icon::Pen), (ToolId::Pencil, Icon::Pencil), (ToolId::ShapeBuilder, Icon::ShapeBuilder)],
    },
    Group { key: "text", name: tr_noop("Text"), tools: &[(ToolId::Text, Icon::Text)] },
    Group { key: "comment", name: tr_noop("Comment"), tools: &[(ToolId::Comment, Icon::Comment)] },
];

const MODE_CELL: f32 = 28.0;
const MODE_GAP: f32 = 2.0;
const MODE_SEGMENT_WIDTH: f32 = 4.0 * MODE_CELL + 3.0 * MODE_GAP + 4.0;
const MODE_SECTION_WIDTH: f32 = 138.0;
const TOOL_GAP: f32 = 8.0;
const FLYOUT_WIDTH: f32 = 16.0;

fn toolbar_icon_index(icon: Icon) -> Option<usize> {
    match icon {
        Icon::Move => Some(0),
        Icon::Frame => Some(1),
        Icon::Rectangle => Some(2),
        Icon::Pen => Some(3),
        Icon::Text => Some(4),
        Icon::Comment => Some(5),
        Icon::Actions => Some(6),
        Icon::Chevron => Some(7),
        _ => None,
    }
}

fn paint_tool_icon(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    if let Some(index) = toolbar_icon_index(icon) {
        icons::paint_toolbar(ui.painter(), rect, index, color);
    } else {
        icons::paint(ui.painter(), rect, icon, color);
    }
}

fn remembered(ui: &Ui, key: &str, default: ToolId) -> ToolId {
    ui.data(|d| d.get_temp::<u8>(egui::Id::new(("tb_last", key)))).and_then(|i| ToolId::ALL.get(i as usize).copied()).unwrap_or(default)
}

pub fn show(app: &mut FigApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let canvas = app.canvas.rect;
    app.ui.actions_rect = None;
    // A file you can't edit now (view only, or on its way to a server): moving around only.
    if let Some(why) = app.session.active_read_only() {
        view_only_toolbar(app, ui, why);
        return;
    }
    // Vector editing keeps the main tools available and adds its own toolbar above them.
    if let Some(tool) = app.session.ds().ok().and_then(|d| d.vector_edit.as_ref()).map(|v| v.tool) {
        main_toolbar(app, ui, canvas, &t);
        edit_toolbar(app, ui, tool);
        return;
    }
    // So does the shape builder, for its merge/remove modes.
    if app.session.tool.tool == ToolId::ShapeBuilder {
        builder_toolbar(app, ui);
        return;
    }
    main_toolbar(app, ui, canvas, &t);
    #[cfg(not(target_arch = "wasm32"))]
    if app.pending_image_vectorize.is_some() {
        crate::image_vectorize::toolbar(app, ui);
        return;
    }
    if let Some((id, index, paint)) = selected_image_fill(app) {
        image_toolbar(app, ui, id, index, paint);
    }
}

/// The main toolbar pill's width: the Actions palette matches it so the two read as one bar.
pub(crate) fn main_width() -> f32 {
    // Five 32 + 1 + 16 tool groups, Comment and Actions, six 8 px gaps, 8 px side padding.
    16.0 + 5.0 * (32.0 + 1.0 + FLYOUT_WIDTH) + 2.0 * 32.0 + 6.0 * TOOL_GAP + MODE_SECTION_WIDTH
}

/// The main toolbar pill: the tool groups with their flyouts, Comment and Actions, then the
/// Draw · Design · Prototype · Inspect mode switcher.
fn main_toolbar(app: &mut FigApp, ui: &mut Ui, canvas: Rect, t: &Tokens) {
    let cell = 32.0;
    let width = main_width();
    let rect = pill(ui, &mut app.ui.nav, canvas, width, t);
    let current = match app.session.ds().ok().and_then(|ds| ds.vector_edit.as_ref()).map(|ve| ve.tool) {
        Some(VTool::Pen) => ToolId::Pen,
        _ => app.session.tool.tool,
    };
    let mut x = rect.min.x + 8.0;
    let cy = rect.center().y;
    for g in GROUPS {
        let in_group = g.tools.iter().any(|(tid, _)| *tid == current);
        let shown = if in_group { current } else { remembered(ui, g.key, g.tools.first().map(|x| x.0).unwrap_or(ToolId::Move)) };
        let icon = g.tools.iter().find(|(tid, _)| *tid == shown).map(|(_, i)| *i).unwrap_or(Icon::Move);
        let r = Rect::from_center_size(pos2(x + cell / 2.0, cy), vec2(cell, cell));
        let resp = tool_cell(ui, t, r, ("tool", g.key), icon, in_group, &format!("{}  {}", tr(shown.label()), pretty_shortcut(shown.shortcut())));
        if in_group {
            a11y::prefer(&mut app.ui.nav, Region::Toolbar, resp.id);
        }
        if resp.clicked() {
            // Enter (a keyboard click) on the shape or frame tool already chosen places one; the
            // canvas takes the keyboard.
            if in_group && shown.draws_shape() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                app.run("tool.place", json!({}));
                app.ui.nav.go(Region::Canvas);
            } else {
                app.run("tool.set", json!({"tool": shown.id()}));
            }
        }
        x += cell;
        // Flyout chevron.
        if g.tools.len() > 1 || g.key == "text" {
            x += 1.0;
            let cr = Rect::from_min_size(pos2(x, cy - cell / 2.0), vec2(FLYOUT_WIDTH, cell));
            let cresp = ui.interact(cr, ui.id().with(("flyout", g.key)), Sense::click());
            let open = app.ui.tool_flyout == Some(g.key);
            a11y::menu_button(&cresp, tr(g.name), open);
            if open {
                a11y::popup(&mut app.ui.nav, cresp.id, flyout_id(g.key));
            }
            if cresp.hovered() {
                ui.painter().rect_filled(cr, CornerRadius::same(4), t.hover);
            }
            paint_tool_icon(
                ui,
                Rect::from_center_size(cr.center(), vec2(24.0, 24.0)),
                Icon::Chevron,
                if t.dark { Color32::WHITE } else { t.icon_secondary },
            );
            if cresp.clicked() {
                app.ui.tool_flyout = if app.ui.tool_flyout == Some(g.key) { None } else { Some(g.key) };
            }
            if app.ui.tool_flyout == Some(g.key) {
                flyout(app, ui, g, &cresp, pos2(r.min.x, rect.min.y - 8.0));
            }
            x += FLYOUT_WIDTH;
        }
        x += TOOL_GAP;
    }
    let actions = Rect::from_center_size(pos2(x + cell / 2.0, cy), vec2(cell, cell));
    let resp = ui.interact(actions, ui.id().with("actions"), Sense::click()).tip(crate::shortcuts::tip(tr("Actions"), "app.actions"));
    app.ui.actions_rect = Some(actions);
    let actions_open = app.ui.palette.is_some();
    if actions_open {
        ui.painter().rect_filled(actions, CornerRadius::same(5), toolbar_accent(t));
    } else if resp.hovered() {
        ui.painter().rect_filled(actions, CornerRadius::same(5), t.hover);
    }
    paint_tool_icon(ui, actions.shrink(4.0), Icon::Actions, if actions_open || t.dark { Color32::WHITE } else { t.icon });
    if resp.clicked() {
        crate::menus::toggle_palette(&mut app.ui);
    }
    let mode_x = rect.max.x - MODE_SECTION_WIDTH;
    ui.painter().line_segment(
        [pos2(mode_x, rect.min.y), pos2(mode_x, rect.max.y)],
        Stroke::new(1.0, if t.dark { Color32::from_rgb(0x3e, 0x3c, 0x3e) } else { t.border }),
    );
    // Mode switcher: Draw · Design · Prototype · Inspect (code).
    let modes = [tr("Draw"), tr("Design"), tr("Prototype"), tr("Inspect")];
    let active = match app.ui.right_tab {
        crate::RightTab::Design if app.session.tool.tool == ToolId::Pencil => 0,
        crate::RightTab::Design => 1,
        crate::RightTab::Prototype => 2,
        crate::RightTab::Inspect => 3,
    };
    let seg = Rect::from_min_size(pos2(mode_x + 8.0, cy - 16.0), vec2(MODE_SEGMENT_WIDTH, 32.0));
    ui.painter().rect_filled(seg, CornerRadius::same(5), if t.dark { Color32::from_rgb(0x44, 0x44, 0x44) } else { t.field });
    let mode_rect = |i: f32| Rect::from_min_size(pos2(seg.min.x + 2.0 + i * (MODE_CELL + MODE_GAP), seg.min.y + 2.0), vec2(MODE_CELL, MODE_CELL));
    let responses =
        std::array::from_fn::<_, 4, _>(|i| ui.interact(mode_rect(i as f32), ui.id().with(("mode", i)), Sense::click()).on_hover_text(modes[i]));
    for (i, resp) in responses.iter().enumerate() {
        if resp.hovered() && i != active {
            let r = mode_rect(i as f32);
            ui.painter().rect_filled(r, CornerRadius::same(3), t.pressed);
        }
    }
    // One shared selected surface slides between the four radio options.
    let selected = ui.ctx().animate_value_with_time(ui.id().with("mode_selected_position"), active as f32, 0.18);
    let selected_rect = mode_rect(selected);
    ui.painter().rect_filled(selected_rect, CornerRadius::same(3), if t.dark { Color32::from_rgb(0x2c, 0x2c, 0x2c) } else { t.panel });
    ui.painter().rect_stroke(
        selected_rect,
        CornerRadius::same(3),
        Stroke::new(1.0, if t.dark { Color32::from_rgb(0x37, 0x37, 0x37) } else { t.border }),
        StrokeKind::Inside,
    );
    if responses[active].hovered() {
        ui.painter().rect_filled(selected_rect.shrink(1.0), CornerRadius::same(3), t.pressed);
    }
    for (i, (tip, resp)) in modes.iter().zip(responses.iter()).enumerate() {
        let r = mode_rect(i as f32);
        a11y::toggle(resp, tip, i == active);
        icons::paint_toolbar(
            ui.painter(),
            Rect::from_center_size(r.center(), vec2(24.0, 24.0)),
            8 + i,
            if i == active {
                if t.dark { Color32::from_rgb(0x7c, 0xc4, 0xf8) } else { t.accent_text }
            } else if t.dark {
                Color32::from_white_alpha(178)
            } else {
                t.icon_secondary
            },
        );
        if resp.clicked() {
            match i {
                0 => {
                    app.ui.right_tab = crate::RightTab::Design;
                    app.run("tool.set", json!({"tool": "pencil"}));
                }
                1 => {
                    app.ui.right_tab = crate::RightTab::Design;
                    if app.session.tool.tool == ToolId::Pencil {
                        app.run("tool.set", json!({"tool": "move"}));
                    }
                }
                2 => app.ui.right_tab = crate::RightTab::Prototype,
                _ => app.ui.right_tab = crate::RightTab::Inspect,
            }
        }
    }
    // Floating help button at the bottom right of the canvas.
    let hr = Rect::from_center_size(pos2(canvas.max.x - 32.0, canvas.max.y - 32.0), vec2(32.0, 32.0));
    let hresp = ui.interact(hr, ui.id().with("help"), Sense::click()).tip(tr("Help and resources"));
    ui.painter().circle_filled(hr.center() + vec2(0.0, 1.0), 17.0, Color32::from_black_alpha(20));
    ui.painter().circle_filled(hr.center(), 16.0, t.panel);
    ui.painter().text(hr.center(), Align2::CENTER_CENTER, "?", crate::theme::regular(15.0), t.text);
    crate::right::menu_popup(&hresp).show(|ui| {
        for (l, c) in [
            (tr("Getting started"), "help.gettingStarted"),
            (tr("Keyboard shortcuts"), "help.shortcuts"),
            (tr("Help page"), "help.page"),
            (tr("About Lizard"), "help.about"),
        ] {
            if crate::right::menu_item(ui, l, crate::shortcuts::shortcut(c).unwrap_or("")) {
                app.run(c, json!({}));
                ui.close();
            }
        }
    });
}

/// The pill shown for a file you can't edit now: Move and Hand, and a badge saying why ("View
/// only", or "Publishing…" until the file is on the server).
fn view_only_toolbar(app: &mut FigApp, ui: &mut Ui, why: &'static str) {
    let t = Tokens::get(ui.ctx());
    // The editing tools are off: the tool falls back to moving around.
    if !matches!(app.session.tool.tool, ToolId::Move | ToolId::Hand) {
        app.run("tool.set", json!({"tool": "move"}));
    }
    let label = if why == figcraft_engine::live::PUBLISHING { tr("Publishing…") } else { tr("View only") };
    let cell = 32.0;
    let text = ui.painter().layout_no_wrap(label.to_string(), crate::theme::semibold(11.5), t.text);
    let badge_w = 12.0 + 14.0 + 6.0 + text.size().x + 12.0;
    let width = 2.0 * (cell + 4.0) + 8.0 + 12.0 + badge_w + 8.0;
    let rect = pill(ui, &mut app.ui.nav, app.canvas.rect, width, &t);
    let cy = rect.center().y;
    let mut x = rect.min.x + 8.0;
    for (tool, icon) in [(ToolId::Move, Icon::Move), (ToolId::Hand, Icon::Hand)] {
        let r = Rect::from_center_size(pos2(x + cell / 2.0, cy), vec2(cell, cell));
        let active = app.session.tool.tool == tool;
        let resp = tool_cell(ui, &t, r, ("tool", tool.id()), icon, active, &format!("{}  {}", tr(tool.label()), pretty_shortcut(tool.shortcut())));
        if active {
            a11y::prefer(&mut app.ui.nav, Region::Toolbar, resp.id);
        }
        if resp.clicked() {
            app.run("tool.set", json!({"tool": tool.id()}));
        }
        x += cell + 4.0;
    }
    divider(ui, &t, x, cy);
    let badge = Rect::from_min_size(pos2(x + 12.0, cy - 13.0), vec2(badge_w, 26.0));
    let resp = ui.interact(badge, ui.id().with("view_only"), Sense::hover()).on_hover_text(why);
    a11y::status(&resp, label, why);
    ui.painter().rect_filled(badge, CornerRadius::same(13), t.field);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(badge.min.x + 12.0 + 7.0, cy), vec2(14.0, 14.0)), Icon::Eye, t.icon_secondary);
    ui.painter().galley(pos2(badge.min.x + 12.0 + 14.0 + 6.0, cy - text.size().y / 2.0), text, t.text);
}

const VECTOR_EDIT_BUTTONS: [(VTool, Icon, &str, f32); 6] = [
    (VTool::Move, Icon::Move, "Move", 68.0),
    (VTool::Lasso, Icon::Lasso, "Lasso", 70.0),
    (VTool::Paint, Icon::PaintBucket, "Paint vector", 68.0),
    (VTool::Bend, Icon::Bend, "Bend", 70.0),
    (VTool::Cut, Icon::Cut, "Cut vector", 68.0),
    (VTool::Eraser, Icon::Eraser, "Erase vector", 74.0),
];

pub(crate) fn vector_edit_toolbar_width() -> f32 {
    // Surface margins, six tool buttons, three separators, More and Close.
    16.0 + VECTOR_EDIT_BUTTONS.iter().map(|(_, _, _, width)| width).sum::<f32>() + 3.0 * 17.0 + 68.0 + 24.0
}

fn vector_edit_separator(ui: &mut Ui, t: &Tokens) {
    ui.add_space(8.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 24.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, t.border));
    ui.add_space(8.0);
}

/// A second toolbar above the main toolbar while a vector is being edited.
fn edit_toolbar(app: &mut FigApp, ui: &mut Ui, current: VTool) {
    let t = Tokens::get(ui.ctx());
    let canvas = app.canvas.rect;
    let width = vector_edit_toolbar_width();
    let main = pill_rect(canvas, main_width());
    let top_left = pos2(canvas.center().x - width / 2.0, main.min.y - 8.0 - 40.0);
    // Holding Cmd with the Move tool bends: show it.
    let held = current == VTool::Move && ui.input(|i| i.modifiers.command);
    Menu::area(egui::Id::new("vector_edit_toolbar"), top_left).show(ui.ctx(), |ui| {
        let bounds = Rect::from_min_size(top_left, vec2(width, 40.0));
        ui.interact(bounds, ui.id().with("vector_edit_toolbar_background"), Sense::CLICK | Sense::DRAG);
        a11y::area(&mut app.ui.nav, Region::Toolbar, bounds);
        a11y::container_bounds(ui, bounds);
        Surface::palette().show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                for (index, (tool, icon, label, width)) in VECTOR_EDIT_BUTTONS.into_iter().enumerate() {
                    if index == 2 {
                        vector_edit_separator(ui, &t);
                    }
                    let active = (tool == current && !held) || (held && tool == VTool::Bend);
                    let tip = if tool == VTool::Bend {
                        trf("{tool}  hold {key}", &[("tool", &tr(tool.label())), ("key", &pretty_shortcut(tool.shortcut()))])
                    } else {
                        format!("{}  {}", tr(tool.label()), pretty_shortcut(tool.shortcut()))
                    };
                    let response = Button::new(tr(label))
                        .leading(icon)
                        .variant(if active { ButtonVariant::Primary } else { ButtonVariant::Ghost })
                        .width(ControlWidth::Fixed(width))
                        .show(ui)
                        .on_hover_text(tip);
                    if active {
                        a11y::prefer(&mut app.ui.nav, Region::Toolbar, response.id);
                    }
                    if response.clicked() {
                        app.run("vector.edit.tool", json!({"tool": tool.id()}));
                    }
                }
                vector_edit_separator(ui, &t);
                let more = Button::new(tr("More"))
                    .trailing(Icon::Chevron)
                    .variant(if matches!(current, VTool::Pen | VTool::Width) { ButtonVariant::Primary } else { ButtonVariant::Ghost })
                    .width(ControlWidth::Fixed(68.0))
                    .show(ui);
                Menu::popup(&more).width(164.0).align(egui::RectAlign::TOP_START).show(|ui| {
                    for (tool, icon) in [(VTool::Pen, Icon::Pen), (VTool::Width, Icon::VariableWidth)] {
                        let shortcut = pretty_shortcut(tool.shortcut());
                        if MenuItem::new(tr(tool.label())).leading(icon).checked(tool == current).shortcut(&shortcut).show(ui).clicked() {
                            app.run("vector.edit.tool", json!({"tool": tool.id()}));
                            ui.close();
                        }
                    }
                });
                vector_edit_separator(ui, &t);
                if Button::icon(Icon::Close, tr("Done editing")).width(ControlWidth::Fixed(24.0)).show(ui).on_hover_text("Esc").clicked() {
                    app.run("vector.edit.end", json!({}));
                }
            });
        });
    });
}

#[derive(Clone, Copy)]
enum ImageAction {
    Crop,
    SelectArea,
    RemoveBackground,
    EditPrompt,
    Expand,
    Enhance,
    Vectorize,
}

const IMAGE_BUTTONS: [(ImageAction, Icon, &str, f32); 4] = [
    (ImageAction::Crop, Icon::ImageCrop, "Crop", 66.0),
    (ImageAction::SelectArea, Icon::ImageSelectArea, "Select area", 112.0),
    (ImageAction::RemoveBackground, Icon::ImageRemoveBackground, "Remove background", 166.0),
    (ImageAction::EditPrompt, Icon::ImageEditPrompt, "Edit with prompt", 128.0),
];

pub(crate) fn image_toolbar_width() -> f32 {
    16.0 + IMAGE_BUTTONS.iter().map(|(_, _, _, width)| width).sum::<f32>() + 17.0 + 62.0
}

fn selected_image_fill(app: &FigApp) -> Option<(NodeId, usize, Paint)> {
    let selection = app.session.selection();
    let [id] = selection.as_slice() else { return None };
    app.session
        .doc()
        .ok()?
        .node(*id)?
        .fills
        .iter()
        .enumerate()
        .find(|(_, fill)| matches!(fill.kind, PaintKind::Image(_)))
        .map(|(index, fill)| (*id, index, fill.clone()))
}

fn image_action(app: &mut FigApp, ui: &Ui, id: NodeId, index: usize, mut paint: Paint, action: ImageAction) {
    if let ImageAction::Crop = action {
        if let PaintKind::Image(image) = &mut paint.kind
            && image.scale_mode != ImageScaleMode::Crop
        {
            image.scale_mode = ImageScaleMode::Crop;
            app.run("fill.set", json!({"ids": [id], "index": index, "paint": paint}));
        }
        app.ui.picker = Some(crate::colorpicker::PickerTarget::Paint { fills: true, index, ids: vec![id] });
        return;
    }
    if let ImageAction::SelectArea = action {
        app.toast(tr("Image area selection is not available yet"));
        return;
    }
    if let ImageAction::RemoveBackground = action {
        #[cfg(not(target_arch = "wasm32"))]
        crate::background_removal::start(app, ui.ctx(), id, index);
        #[cfg(target_arch = "wasm32")]
        app.toast(tr("Local background removal is available in the desktop app"));
        return;
    }
    if let ImageAction::Vectorize = action {
        #[cfg(not(target_arch = "wasm32"))]
        crate::image_vectorize::start(app, ui.ctx(), id, index);
        #[cfg(target_arch = "wasm32")]
        app.toast(tr("Image vectorization is available in the desktop app"));
        return;
    }
    let prompt = match action {
        ImageAction::EditPrompt => "Edit the selected image: ",
        ImageAction::Expand => "Expand the selected image while preserving its content.",
        ImageAction::Enhance => "Improve the resolution of the selected image.",
        ImageAction::Crop | ImageAction::SelectArea | ImageAction::RemoveBackground | ImageAction::Vectorize => return,
    };
    app.select_left_tab(crate::LeftTab::Agent);
    if app.ui.agent.draft.is_empty() {
        app.ui.agent.draft = tr(prompt).to_string();
    }
    ui.memory_mut(|memory| memory.request_focus(egui::Id::new("agent_prompt")));
}

/// Contextual image actions above the normal toolbar while one image fill is selected.
fn image_toolbar(app: &mut FigApp, ui: &mut Ui, id: NodeId, index: usize, paint: Paint) {
    let t = Tokens::get(ui.ctx());
    let canvas = app.canvas.rect;
    let width = image_toolbar_width();
    let main = pill_rect(canvas, main_width());
    let top_left = pos2(canvas.center().x - width / 2.0, main.min.y - 48.0);
    Menu::area(egui::Id::new("image_toolbar"), top_left).show(ui.ctx(), |ui| {
        let bounds = Rect::from_min_size(top_left, vec2(width, 40.0));
        ui.interact(bounds, ui.id().with("image_toolbar_background"), Sense::CLICK | Sense::DRAG);
        a11y::area(&mut app.ui.nav, Region::Toolbar, bounds);
        a11y::container_bounds(ui, bounds);
        Surface::palette().show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                for (action, icon, label, width) in IMAGE_BUTTONS {
                    if Button::new(tr(label)).leading(icon).variant(ButtonVariant::Ghost).width(ControlWidth::Fixed(width)).show(ui).clicked() {
                        image_action(app, ui, id, index, paint.clone(), action);
                    }
                }
                vector_edit_separator(ui, &t);
                let more =
                    Button::new(tr("More")).trailing(Icon::Chevron).variant(ButtonVariant::Secondary).width(ControlWidth::Fixed(62.0)).show(ui);
                Menu::popup(&more).width(171.0).align(egui::RectAlign::TOP_START).show(|ui| {
                    for (action, icon, label) in [
                        (ImageAction::Expand, Icon::ImageExpand, "Expand"),
                        (ImageAction::Enhance, Icon::ImageEnhance, "Enhance resolution"),
                        (ImageAction::Vectorize, Icon::ImageVectorize, "Vectorize"),
                    ] {
                        if MenuItem::new(tr(label)).leading(icon).show(ui).clicked() {
                            image_action(app, ui, id, index, paint.clone(), action);
                            ui.close();
                        }
                    }
                });
            });
        });
    });
}

/// The pill shown while the shape builder is active: Merge and Remove modes, a hint and Done.
fn builder_toolbar(app: &mut FigApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let cell = 32.0;
    let remove = app.session.tool.builder_remove;
    let has_regions = figcraft_engine::shapebuilder::current(&app.session).is_some_and(|a| !a.regions.is_empty());
    let alt = pretty_shortcut("Alt");
    let hint = if !has_regions {
        tr("Select overlapping shapes").to_string()
    } else if remove {
        trf("Click or drag to remove · {alt} merges", &[("alt", &alt)])
    } else {
        trf("Click or drag to merge · {alt} removes", &[("alt", &alt)])
    };
    let galley = ui.painter().layout_no_wrap(hint, regular(12.0), t.text_secondary);
    let width = 2.0 * (cell + 4.0) + 8.0 + 12.0 + galley.size().x + 12.0 + 12.0 + DONE_W + 8.0;
    let rect = pill(ui, &mut app.ui.nav, app.canvas.rect, width, &t);
    let cy = rect.center().y;
    let mut x = rect.min.x + 8.0;
    for (mode, icon, tip) in [("merge", Icon::ShapeBuilder, tr("Merge regions")), ("remove", Icon::ShapeBuilderRemove, tr("Remove regions"))] {
        let r = Rect::from_center_size(pos2(x + cell / 2.0, cy), vec2(cell, cell));
        let active = (mode == "remove") == remove;
        let resp = tool_cell(ui, &t, r, ("sbmode", mode), icon, active, tip);
        if active {
            a11y::prefer(&mut app.ui.nav, Region::Toolbar, resp.id);
        }
        if resp.clicked() {
            app.run("shapeBuilder.mode", json!({"mode": mode}));
        }
        x += cell + 4.0;
    }
    divider(ui, &t, x, cy);
    x += 12.0;
    let w = galley.size().x;
    ui.painter().galley(pos2(x, cy - galley.size().y / 2.0), galley, t.text_secondary);
    x += w + 12.0;
    if done_button(ui, &t, x, cy, &format!("{}  {}", tr("Done"), pretty_shortcut("Enter"))) {
        app.run("tool.set", json!({"tool": "move"}));
    }
}

/// Where a toolbar pill `width` wide sits: centred 12 px above the canvas's bottom edge.
pub(crate) fn pill_rect(canvas: Rect, width: f32) -> Rect {
    Rect::from_center_size(pos2(canvas.center().x, canvas.max.y - 12.0 - 24.0), vec2(width, 48.0))
}

/// The floating pill (shadow, panel, border) `width` wide at the bottom centre of the canvas: the
/// toolbar region keyboard navigation moves through.
fn pill(ui: &Ui, nav: &mut Nav, canvas: Rect, width: f32, t: &Tokens) -> Rect {
    let rect = pill_rect(canvas, width);
    // The pill takes the press and the drag over it: its buttons sense clicks only, and egui gives
    // a drag the canvas under them would sense to the canvas, so the pen placed an anchor there.
    // Not focusable: the keyboard walks the buttons, not the pill.
    ui.interact(rect, ui.id().with("pill"), Sense::CLICK | Sense::DRAG);
    a11y::area(nav, Region::Toolbar, rect);
    a11y::container_bounds(ui, rect);
    let painter = ui.painter();
    for (i, a) in [(3.0, 12u8), (1.0, 30)] {
        painter.rect_filled(rect.expand(i).translate(vec2(0.0, 1.0)), CornerRadius::same((12.0 + i) as u8), Color32::from_black_alpha(a));
    }
    painter.rect_filled(rect, CornerRadius::same(12), if t.dark { Color32::from_rgb(0x27, 0x27, 0x27) } else { t.panel });
    painter.line_segment(
        [pos2(rect.min.x + 12.0, rect.min.y + 0.5), pos2(rect.max.x - 12.0, rect.min.y + 0.5)],
        Stroke::new(1.0, if t.dark { Color32::from_rgb(0x3e, 0x3c, 0x3e) } else { t.border }),
    );
    rect
}

fn toolbar_accent(t: &Tokens) -> Color32 {
    if t.dark { Color32::from_rgb(0x0c, 0x8c, 0xe9) } else { t.accent }
}

/// A square tool button: accent when active, tinted on hover; `tip` is its tooltip and name.
fn tool_cell(ui: &mut Ui, t: &Tokens, r: Rect, id: impl egui::AsIdSalt, icon: Icon, active: bool, tip: &str) -> egui::Response {
    let resp = ui.interact(r, ui.id().with(id), Sense::click()).on_hover_ui(|ui| {
        ui.label(tip);
    });
    a11y::toggle(&resp, tip, active);
    let bg = if active {
        toolbar_accent(t)
    } else if resp.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(r, CornerRadius::same(5), bg);
    paint_tool_icon(ui, r.shrink(4.0), icon, if active || t.dark { Color32::WHITE } else { t.icon });
    resp
}

/// A vertical separator just after `x`.
fn divider(ui: &Ui, t: &Tokens, x: f32, cy: f32) {
    ui.painter().line_segment([pos2(x + 2.0, cy - 14.0), pos2(x + 2.0, cy + 14.0)], Stroke::new(1.0, t.border));
}

const DONE_W: f32 = 56.0;

/// A divider at `x` and the accent Done button after it; true when clicked.
fn done_button(ui: &mut Ui, t: &Tokens, x: f32, cy: f32, tip: &str) -> bool {
    divider(ui, t, x, cy);
    let done = Rect::from_min_size(pos2(x + 12.0, cy - 14.0), vec2(DONE_W, 28.0));
    let resp = ui.interact(done, ui.id().with("tool_done"), Sense::click()).tip(tip);
    ui.painter().rect_filled(done, CornerRadius::same(6), if resp.hovered() { t.accent_hover } else { t.accent });
    ui.painter().text(done.center(), Align2::CENTER_CENTER, tr("Done"), crate::theme::semibold(11.5), t.accent_on);
    resp.clicked()
}

fn flyout(app: &mut FigApp, ui: &mut Ui, g: &Group, anchor: &egui::Response, bottom_left: egui::Pos2) {
    let w = match g.key {
        "move" => 202.0,
        "frame" => 143.0,
        "shape" => 206.0,
        "pen" => 138.0,
        "text" => 169.0,
        _ => 202.0,
    };
    let mut chosen = None;
    crate::primitives::Menu::popup(anchor)
        .id(flyout_id(g.key))
        .at_position(bottom_left)
        .width(w)
        .open_memory(true)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            for (tid, icon) in g.tools {
                let shortcut = pretty_shortcut(tid.shortcut());
                let resp = crate::primitives::MenuItem::new(tr(tid.label()))
                    .checked(*tid == app.session.tool.tool)
                    .leading(*icon)
                    .shortcut(&shortcut)
                    .show(ui);
                if resp.clicked() {
                    chosen = Some(*tid);
                }
            }
        });
    if !egui::Popup::is_id_open(ui.ctx(), flyout_id(g.key)) {
        app.ui.tool_flyout = None;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.tool_flyout = None;
    }
    if let Some(tid) = chosen {
        if let Some(i) = ToolId::ALL.iter().position(|x| *x == tid) {
            ui.data_mut(|d| d.insert_temp(egui::Id::new(("tb_last", g.key)), i as u8));
        }
        app.run("tool.set", json!({"tool": tid.id()}));
        app.ui.tool_flyout = None;
    }
}

/// A tool group's flyout (an egui area).
fn flyout_id(key: &str) -> egui::Id {
    egui::Id::new(("flyout_area", key))
}

/// "Shift+Cmd+K" → "⇧⌘K" on macOS, "Ctrl+Shift+K" elsewhere.
pub fn pretty_shortcut(s: &str) -> String {
    figcraft_engine::keymap::display(s, crate::shortcuts::platform())
}
