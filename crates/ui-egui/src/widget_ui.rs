//! Widgets (the layers plugins draw) in the UI: the property menu of the selected widget, as a
//! bar beside it on the canvas and as the same items in the Design panel; the tooltip of the part
//! under the pointer; and the widgets' timers, which wake the window when the next one is due and
//! at no other time. Everything acts through the engine's `widget.*` commands.

use egui::{Context, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use figcraft_color::{Color, Paint};
use figcraft_doc::{Document, NodeId};
use figcraft_engine::widgets::{self as engine, MenuItem, MenuKind};
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::tr;
use crate::right::{menu_item, menu_popup};
use crate::theme::{ACCENT, Tokens};
use crate::widgets::{self, FIELD_H, PAD};

/// How far the bar sits from the widget, and how much room the size label under a selection takes.
const GAP: f32 = 12.0;
const SIZE_LABEL: f32 = 24.0;
/// Width of a dropdown in the menu, and the side of a colour.
const DROPDOWN_W: f32 = 96.0;
const SWATCH: f32 = 18.0;

/// Fire the widget timers that are due (their plugins are called back) and ask for the frame the
/// next one is due at. Nothing here draws a frame sooner.
pub fn tick(app: &mut FigApp, ctx: &Context) {
    // While a prototype is presented the file's timers wait: a callback would change the file,
    // and the prototype (which plays its own copy, with its own timers) would start over.
    if app.ui.present.is_some() || engine::next_timer(&app.session).is_none() {
        return;
    }
    let errors = engine::run_timers(&mut app.session);
    app.handle_ui_requests();
    if let Some(e) = errors.into_iter().next_back() {
        app.toast(e);
    }
    if let Some(secs) = engine::next_timer(&app.session) {
        crate::repaint_in(ctx, secs);
    }
}

/// The one selected widget and its property menu, when it has one.
fn selected(app: &FigApp) -> Option<(NodeId, Vec<MenuItem>)> {
    let ds = app.session.ds().ok()?;
    let [w] = ds.selection.as_slice() else { return None };
    let menu = engine::menu_of(ds.document_itself(), *w);
    (!menu.is_empty()).then_some((*w, menu))
}

/// Choose item `name` of widget `w`'s menu: its plugin is called back; a link opens.
fn choose(app: &mut FigApp, ctx: &Context, w: NodeId, name: &str, value: Value) {
    let Some(out) = app.run("widget.menuAction", json!({"id": w, "item": name, "value": value})) else { return };
    // Opened only on this click, and only what the engine checked is a web address.
    if let Some(url) = out["url"].as_str() {
        ctx.open_url(egui::OpenUrl::new_tab(url));
    }
}

/// The menu's items in a row, each named for screen readers. Returns the item used and its new
/// value (null for an action or a link).
fn items(ui: &mut Ui, menu: &[MenuItem], t: &Tokens) -> Option<(String, Value)> {
    let mut used = None;
    for item in menu {
        let label = item.title();
        let value = item.value.as_ref();
        let picked = match item.kind {
            MenuKind::Action => widgets::button(ui, label, false, true).clicked().then_some(Value::Null),
            MenuKind::Link => {
                let resp = widgets::button(ui, &format!("{label} ↗"), false, true);
                crate::a11y::link(&resp, label);
                resp.on_hover_text(item.url.as_str()).clicked().then_some(Value::Null)
            }
            MenuKind::Toggle => {
                let on = value.and_then(Value::as_bool).unwrap_or(false);
                widgets::checkbox(ui, on, label).then_some(Value::Bool(!on))
            }
            MenuKind::Dropdown => {
                let shown = value.and_then(Value::as_str).unwrap_or_default();
                widgets::label(ui, label, t.text_secondary);
                let resp = widgets::dropdown(ui, shown, DROPDOWN_W, None);
                crate::a11y::combo(&resp, label, shown, crate::a11y::menu_open(&resp));
                let mut picked = None;
                menu_popup(&resp).show(|ui| {
                    for o in &item.options {
                        if menu_item(ui, o, "") {
                            picked = Some(json!(o));
                            ui.close();
                        }
                    }
                });
                picked
            }
            MenuKind::Color => {
                widgets::label(ui, label, t.text_secondary);
                let mut picked = None;
                for hex in &item.options {
                    let resp = widgets::swatch(ui, &Paint::solid(Color::from_hex(hex).unwrap_or(Color::BLACK)), SWATCH);
                    crate::a11y::radio(&resp, &format!("{label}: {hex}"), value.and_then(Value::as_str) == Some(hex));
                    if value.and_then(Value::as_str) == Some(hex) {
                        ui.painter().rect_stroke(resp.rect.expand(2.0), CornerRadius::same(4), Stroke::new(1.5, ACCENT), StrokeKind::Outside);
                    }
                    if resp.on_hover_text(hex.as_str()).clicked() {
                        picked = Some(json!(hex));
                    }
                }
                picked
            }
            MenuKind::Separator => {
                let (r, _) = ui.allocate_exact_size(vec2(1.0, FIELD_H), Sense::hover());
                ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, t.border));
                None
            }
        };
        if let Some(v) = picked {
            used = Some((item.name.clone(), v));
        }
    }
    used
}

/// The selected widget's property menu as a bar on the canvas (`ui` is the canvas's: windows and
/// popups lie over the bar): centred above the widget, or under it when there is no room above,
/// and kept inside the canvas. Hidden while the widget is dragged or its text is typed.
pub fn menu_bar(app: &mut FigApp, ui: &mut Ui, canvas: Rect) {
    let Some((w, menu)) = selected(app) else { return };
    let Ok(ds) = app.session.ds() else { return };
    if ds.interaction.is_some() || ds.text_edit.is_some() || ds.preview.is_some() {
        return;
    }
    let Some(b) = ds.doc.world_bounds(w) else { return };
    let on_screen = Rect::from_two_pos(app.to_screen(b.x0, b.y0), app.to_screen(b.x1, b.y1));
    if !canvas.intersects(on_screen) {
        return;
    }
    // Placed by the size it had in the last frame (drawn again when that changes).
    let id = Id::new("widget_menu");
    let size: egui::Vec2 = ui.data(|d| d.get_temp(id)).unwrap_or(vec2(320.0, FIELD_H + PAD));
    let within = canvas.shrink(8.0);
    let above = on_screen.min.y - GAP - size.y >= within.min.y;
    let y = if above { on_screen.min.y - GAP - size.y } else { on_screen.max.y + GAP + SIZE_LABEL };
    let x = (on_screen.center().x - size.x / 2.0).min(within.max.x - size.x).max(within.min.x);
    let at = Rect::from_min_size(pos2(x, y), size);
    // The bar takes the pointer: a press on it isn't one on the canvas under it.
    ui.interact(at, id.with("bar"), Sense::click_and_drag());
    let t = Tokens::get(ui.ctx());
    let room = Rect::from_min_max(at.min, pos2(within.max.x.max(at.max.x), at.max.y));
    let drawn = ui.scope_builder(egui::UiBuilder::new().id_salt(id).max_rect(room).layout(egui::Layout::left_to_right(egui::Align::Min)), |ui| {
        crate::a11y::container(ui, egui::accesskit::Role::Toolbar, tr("Widget"));
        widgets::bar(ui, |ui| items(ui, &menu, &t))
    });
    if drawn.response.rect.size() != size {
        ui.data_mut(|d| d.insert_temp(id, drawn.response.rect.size()));
        ui.ctx().request_repaint();
    }
    if let Some((name, value)) = drawn.inner {
        let ctx = ui.ctx().clone();
        choose(app, &ctx, w, &name, value);
    }
}

/// The selected widget's property menu in the Design panel (the same items as the bar's). Returns
/// whether it drew anything.
pub fn design_section(app: &mut FigApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) -> bool {
    let [w] = sel else { return false };
    let menu = engine::menu_of(doc, *w);
    if menu.is_empty() {
        return false;
    }
    let t = Tokens::get(ui.ctx());
    widgets::section_header(ui, tr("Widget"), &[], false);
    // Buttons and toggles share rows (wrapping); a dropdown or a row of colours has its own,
    // with its label.
    let plain = |i: &MenuItem| matches!(i.kind, MenuKind::Action | MenuKind::Toggle | MenuKind::Link);
    let mut used = None;
    for group in menu.chunk_by(|a, b| plain(a) && plain(b)) {
        let hit = match group.first().map(|i| i.kind) {
            None | Some(MenuKind::Separator) => None,
            Some(MenuKind::Dropdown | MenuKind::Color) => {
                let mut hit = None;
                crate::right::row(ui, widgets::ROW_H, |ui| hit = items(ui, group, &t));
                hit
            }
            Some(_) => {
                let inset = ui.available_rect_before_wrap().shrink2(vec2(PAD, 0.0));
                let wrapped = ui.scope_builder(egui::UiBuilder::new().max_rect(inset), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                        items(ui, group, &t)
                    })
                    .inner
                });
                ui.add_space(4.0);
                wrapped.inner
            }
        };
        used = hit.or(used);
    }
    ui.add_space(8.0);
    if let Some((name, value)) = used {
        let ctx = ui.ctx().clone();
        choose(app, &ctx, *w, &name, value);
    }
    true
}

/// What the pointer over the canvas (its response) means for widgets: the tooltip of the part
/// under it; and once it has left the canvas (for a panel, a window over it, the menu bar), no
/// part is under it, so none keeps its hover style.
pub fn pointer(app: &mut FigApp, canvas: &egui::Response) {
    if canvas.hovered() {
        if let Some(tip) = app.session.tool.overlay.widget_tip.as_deref() {
            canvas.clone().on_hover_text_at_pointer(tip);
        }
    } else if !canvas.ctx.input(|i| i.pointer.any_down()) {
        engine::set_hover(&mut app.session, None);
        app.session.tool.overlay.widget_tip = None;
    }
}
