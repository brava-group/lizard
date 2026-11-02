//! Selection colors lives below Effects. A picker edits frozen paint slots, so color groups
//! merging while scrubbing cannot accidentally recolor other objects.

use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Ui, pos2, vec2};
use lizard_color::Color;
use lizard_doc::{Document, NodeId, StyleKind};
use lizard_engine::selection_colors::{PaintTarget, SelectionColor, collect};
use serde_json::json;

use crate::a11y::Tip;
use crate::a11y;
use crate::colorpicker::PickerTarget;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::primitives::{ColorControl, ColorValue};
use crate::widgets::FieldEdit;
use crate::{LizardApp, widgets};

fn header(ui: &mut Ui, colors: &[SelectionColor], open: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let title = tr("Selection colors");
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    a11y::disclosure(&response, title, open);
    let hovered = response.hovered();
    let ink = if hovered && t.dark { Color32::WHITE } else if hovered { t.text } else { t.text_secondary };
    ui.painter().text(pos2(rect.left() + widgets::PAD + 18.0, rect.center().y), Align2::LEFT_CENTER, title, crate::theme::property_section_title(), ink);
    if hovered {
        let chevron = Rect::from_center_size(pos2(rect.left() + widgets::PAD + 5.0, rect.center().y), vec2(10.0, 10.0));
        icons::paint(ui.painter(), chevron, if open { Icon::Chevron } else { Icon::ChevronRight }, ink);
    }
    if !open {
        let extra = colors.len().saturating_sub(3);
        let mut right = rect.right() - widgets::PAD;
        if extra > 0 {
            let label = format!("+{extra}");
            let label_rect = ui.painter().text(pos2(right, rect.center().y), Align2::RIGHT_CENTER, label, crate::theme::regular(11.0), t.text_secondary);
            right = label_rect.left() - 8.0;
        }
        for color in colors.iter().take(3).rev() {
            let swatch = Rect::from_center_size(pos2(right - 8.0, rect.center().y), vec2(16.0, 16.0));
            widgets::paint_swatch(ui.painter(), swatch, &color.paint);
            right = swatch.left() - 8.0;
        }
    }
    response.clicked()
}

#[derive(Clone)]
struct Cache {
    document: u64,
    doc: Arc<Document>,
    ids: Vec<NodeId>,
    colors: Arc<Vec<SelectionColor>>,
}

fn colors(ui: &Ui, document: u64, doc: &Arc<Document>, ids: &[NodeId], interacting: bool) -> Arc<Vec<SelectionColor>> {
    let key = Id::new("selection_colors_cache");
    if let Some(cache) = ui.data(|d| d.get_temp::<Cache>(key))
        && cache.document == document
        && cache.ids == ids
        && (interacting || Arc::ptr_eq(&cache.doc, doc))
    {
        return cache.colors;
    }
    // A drag replaces the document snapshot every frame. Keep its existing rows until the
    // gesture ends; a cold cache or a changed document/selection waits too. Engine commands
    // still discover and change the complete selection, independently of this panel cache.
    if interacting {
        return Arc::new(Vec::new());
    }
    let colors = Arc::new(collect(doc, ids));
    ui.data_mut(|d| d.insert_temp(key, Cache { document, doc: doc.clone(), ids: ids.to_vec(), colors: colors.clone() }));
    colors
}

pub(crate) fn open_picker(app: &mut LizardApp, targets: Vec<PaintTarget>) {
    let Ok(ds) = app.session.ds() else { return };
    app.ui.picker = Some(PickerTarget::SelectionColor { targets, selection: ds.selection.clone(), document: ds.uid });
}

pub fn section(app: &mut LizardApp, ui: &mut Ui, doc: &Arc<Document>, ids: &[NodeId]) {
    let Ok(ds) = app.session.ds() else { return };
    let interacting = ds.interaction.is_some();
    let colors = colors(ui, ds.uid, doc, ids, interacting);
    if colors.len() < 2 {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let open_id = Id::new(("selection_colors_open", ds.uid));
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    if header(ui, &colors, open) {
        open = !open;
        ui.data_mut(|d| d.insert_temp(open_id, open));
    }
    if !open {
        widgets::separator(ui);
        return;
    }
    // Bound the panel's height; a document with many distinct colors stays scrollable.
    ui.add_enabled_ui(!interacting, |ui| {
        egui::ScrollArea::vertical().id_salt("selection_colors_rows").max_height(240.0).show_rows(ui, 32.0, colors.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for group in &colors[range] {
                ui.push_id((&group.targets, "selection_color"), |ui| {
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                    let control = Rect::from_min_size(pos2(row.left() + 16.0, row.top() + 4.0), vec2((row.width() - 84.0).max(64.0), 24.0));
                    let linked_name = group
                        .style
                        .as_ref()
                        .and_then(|id| doc.style(id).map(|s| s.name.as_str()))
                        .or_else(|| group.variable.as_ref().and_then(|id| doc.variables.get(id).map(|v| v.name.as_str())));
                    let hex = group.paint.solid_color().map(|color| color.to_hex());
                    let paint_name = widgets::paint_name(&group.paint);
                    let value = match (linked_name, hex.as_deref()) {
                        (Some(name), _) => ColorValue::Label(name),
                        (None, Some(hex)) => ColorValue::EditableHex(hex),
                        (None, None) => ColorValue::Label(&paint_name),
                    };
                    let change = ColorControl::new(ui.id().with("control"), &group.paint, value, f64::from(group.paint.opacity) * 100.0)
                        .label_opens_picker()
                        .show_at(ui, control);
                    if change.open_picker {
                        open_picker(app, group.targets.clone());
                    }
                    if let Some(hex) = change.edited_hex {
                        if let Some(color) = Color::from_hex(&hex) {
                            let mut paint = group.paint.clone();
                            paint.kind = lizard_color::PaintKind::Solid { color: color.with_alpha(1.0) };
                            if color.a < 1.0 {
                                paint.opacity = color.a;
                            }
                            app.run("selection.setPaint", json!({"targets": group.targets, "paint": paint}));
                        } else {
                            app.toast(tr("Invalid color"));
                        }
                    }
                    if let Some(edit) = change.opacity_edit {
                        let (value, done) = match edit {
                            FieldEdit::Set(value) => (value, None),
                            FieldEdit::Scrub(value, done) => (value, Some(done)),
                        };
                        if value.is_finite() {
                            let mut paint = group.paint.clone();
                            paint.opacity = (value / 100.0).clamp(0.0, 1.0) as f32;
                            let params = json!({"targets": group.targets, "paint": paint});
                            if let Some(done) = done {
                                let _ = app.session.begin_interaction("Opacity");
                                let _ = app.session.execute("selection.setPaint", &params);
                                if done {
                                    let _ = app.session.commit_interaction();
                                }
                            } else {
                                app.run("selection.setPaint", params);
                            }
                        }
                    }
                    let styles_rect = Rect::from_min_size(pos2(row.right() - 60.0, control.top()), vec2(24.0, 24.0));
                    let select_rect = Rect::from_min_size(pos2(row.right() - 32.0, control.top()), vec2(24.0, 24.0));
                    let styles = ui.interact(styles_rect, ui.id().with("styles"), Sense::click()).tip(tr("Color styles"));
                    let select = ui.interact(select_rect, ui.id().with("select_layers"), Sense::click()).tip(tr("Select layers with this color"));
                    a11y::button(&styles, tr("Color styles"));
                    a11y::button(&select, tr("Select layers with this color"));
                    if ui.rect_contains_pointer(row) || styles.has_focus() || select.has_focus() {
                        for (rect, response, icon) in [(styles_rect, &styles, Icon::Styles), (select_rect, &select, Icon::Move)] {
                            if response.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(5), t.hover);
                            }
                            icons::paint(ui.painter(), rect.shrink(4.0), icon, t.icon);
                        }
                    }
                    crate::right::menu_popup(&styles).show(|ui| {
                        let mut any = false;
                        for style in &doc.styles {
                            if !matches!(style.kind, StyleKind::Paint { .. }) {
                                continue;
                            }
                            any = true;
                            if crate::right::menu_item(ui, &style.name, "") {
                                app.run("selection.applyColorStyle", json!({"targets": group.targets, "style": style.id}));
                                app.ui.picker = None;
                                ui.close();
                            }
                        }
                        if !any {
                            widgets::label(ui, tr("No color styles"), t.text_secondary);
                        }
                    });
                    if select.clicked() {
                        app.run("selection.selectColor", json!({"targets": group.targets}));
                    }
                });
            }
        });
    });
    ui.add_space(12.0);
    widgets::separator(ui);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoServices;
    use egui::{Event, Pos2, RawInput, Rect, Shape, vec2};
    use lizard_engine::Session;

    struct Panel {
        ctx: egui::Context,
        app: LizardApp,
        time: f64,
    }

    impl Panel {
        fn new() -> (Self, NodeId, NodeId) {
            let mut s = Session::new();
            let mut ids = Vec::new();
            for color in ["#ff0000", "#0000ff"] {
                let value = s.execute("shape.rectangle", &json!({"width":40,"height":40,"props":{"fill":color}})).unwrap();
                ids.push(NodeId(value["id"].as_u64().unwrap()));
            }
            s.execute("select.set", &json!({"ids":ids})).unwrap();
            let mut p = Self { ctx: egui::Context::default(), app: LizardApp::new(s, Box::new(NoServices)), time: 0.0 };
            crate::theme::install_fonts(&p.ctx, false, 0);
            crate::theme::apply(&p.ctx, crate::theme::Tokens::light());
            for _ in 0..3 {
                p.frame(vec![]);
            }
            (p, ids[0], ids[1])
        }
        fn frame(&mut self, events: Vec<Event>) -> Vec<(String, Pos2)> {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(280.0, 600.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| {
                let ds = self.app.session.ds().unwrap();
                let doc = ds.doc.clone();
                let ids = ds.selection.clone();
                section(&mut self.app, ui, &doc, &ids);
            });
            output.textures_delta.clear();
            fn texts(shape: &Shape, out: &mut Vec<(String, Pos2)>) {
                match shape {
                    Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos + t.galley.size() / 2.0)),
                    Shape::Vec(v) => {
                        for s in v {
                            texts(s, out)
                        }
                    }
                    _ => {}
                }
            }
            let mut text = Vec::new();
            for s in &output.shapes {
                texts(&s.shape, &mut text);
            }
            text
        }
        fn click(&mut self, pos: Pos2) {
            for pressed in [true, false] {
                self.frame(vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() },
                ]);
            }
        }
        fn expand(&mut self) {
            let title = self.frame(vec![]).into_iter().find(|(s, _)| s == "Selection colors").unwrap().1;
            self.click(title);
        }
    }

    #[test]
    fn selection_colors_panel_opens_the_existing_picker_and_hides_for_one_color() {
        let (mut p, a, b) = Panel::new();
        let texts = p.frame(vec![]);
        assert!(texts.iter().any(|(s, _)| s == "Selection colors"));
        assert!(!texts.iter().any(|(s, _)| s == "FF0000"));
        p.expand();
        let texts = p.frame(vec![]);
        let red = texts.iter().find(|(s, _)| s == "FF0000").unwrap().1;
        p.click(Pos2::new(28.0, red.y));
        let Some(PickerTarget::SelectionColor { targets, .. }) = &p.app.ui.picker else { panic!("the swatch must open its picker") };
        assert_eq!(targets.iter().map(|t| t.id).collect::<Vec<_>>(), [a]);
        p.app.run("fill.set", json!({"id":b,"color":"#ff0000"}));
        assert!(!p.frame(vec![]).iter().any(|(s, _)| s == "Selection colors"));
    }

    #[test]
    fn panel_keeps_cached_colors_during_interaction_and_refreshes_after_commit() {
        let (mut p, a, _) = Panel::new();
        p.expand();
        let key = Id::new("selection_colors_cache");
        let before = p.ctx.data(|d| d.get_temp::<Cache>(key)).unwrap();
        p.app.session.begin_interaction("Color").unwrap();
        for hex in ["#00ff00", "#ffff00", "#00ffff"] {
            p.app.run("fill.set", json!({"id":a,"color":hex}));
            p.frame(vec![]);
            let during = p.ctx.data(|d| d.get_temp::<Cache>(key)).unwrap();
            assert!(Arc::ptr_eq(&before.colors, &during.colors), "a live edit must not rediscover paints");
        }
        p.app.session.commit_interaction().unwrap();
        let texts = p.frame(vec![]);
        assert!(texts.iter().any(|(s, _)| s == "00FFFF"));
        let after = p.ctx.data(|d| d.get_temp::<Cache>(key)).unwrap();
        assert!(!Arc::ptr_eq(&before.colors, &after.colors));
    }

    #[test]
    fn panel_defers_discovery_without_a_matching_cache_during_interaction() {
        let (mut p, a, _) = Panel::new();
        p.expand();
        let key = Id::new("selection_colors_cache");
        p.ctx.data_mut(|d| d.remove::<Cache>(key));
        p.app.session.begin_interaction("Color").unwrap();
        p.app.run("fill.set", json!({"id":a,"color":"#00ff00"}));
        assert!(!p.frame(vec![]).iter().any(|(s, _)| s == "Selection colors"));
        assert!(p.ctx.data(|d| d.get_temp::<Cache>(key)).is_none());
        p.app.session.cancel_interaction().unwrap();
        assert!(p.frame(vec![]).iter().any(|(s, _)| s == "FF0000"));
    }

    #[test]
    fn select_color_button_selects_only_the_matching_layers() {
        let (mut p, a, _) = Panel::new();
        p.expand();
        let red = p.frame(vec![]).into_iter().find(|(s, _)| s == "FF0000").unwrap().1;
        p.click(Pos2::new(260.0, red.y));
        assert_eq!(p.app.session.selection(), [a]);
        assert!(p.app.ui.last_error.is_none());
    }

    #[test]
    fn panel_displays_the_linked_style_name_and_the_other_color() {
        let (mut p, a, b) = Panel::new();
        p.expand();
        p.app.run("style.create", json!({"type":"paint","name":"Brand","value":"#ff0000"}));
        p.app.run("style.apply", json!({"id":a,"style":"Brand","target":"fill"}));
        let texts = p.frame(vec![]);
        assert!(texts.iter().any(|(s, _)| s == "Brand"));
        assert!(texts.iter().any(|(s, _)| s == "0000FF"));
        let linked = texts.iter().find(|(s, _)| s == "Brand").unwrap().1;
        p.click(linked);
        assert!(matches!(p.app.ui.picker, Some(PickerTarget::SelectionColor { .. })));
        let colors = collect(p.app.session.doc().unwrap(), &[a, b]);
        assert_eq!(colors.len(), 2);
    }
}
