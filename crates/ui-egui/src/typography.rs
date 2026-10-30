//! Typography section of the Design panel: font family (searchable, previewed, recent first),
//! style, size (with common sizes), line height, letter spacing, paragraph spacing, resizing,
//! alignment, and the Type settings popover (decoration, case, lists, indent, truncation, OpenType
//! features and variable font axes). While text is being edited with a range selected, character
//! edits apply to the range (`text.style`, `text.setVariations`); otherwise they apply to whole
//! layers. Differing values show "Mixed".

use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use figcraft_doc::*;
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::{tr, tr_noop, trf};
use crate::icons::{self, Icon};
use crate::right::{common, menu_popup, row};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets::{self, FIELD_H, FieldEdit, Lead};

/// Font sizes offered by the size dropdown.
pub(crate) const SIZES: [f64; 15] = [10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 20.0, 24.0, 32.0, 36.0, 40.0, 48.0, 64.0, 96.0, 128.0];
const TYPE_SETTINGS_BASIC_HEIGHT: f32 = 144.0 + 108.0 + 192.0;

/// Shown for differing values (translated where shown: `tr(MIXED)`).
const MIXED: &str = tr_noop("Mixed");

/// The Type settings tabs (Variable only for variable fonts).
const TABS: [&str; 3] = [tr_noop("Basics"), tr_noop("Details"), tr_noop("Variable")];

/// The value shared by every style, or None when they differ.
fn uniform<T: PartialEq + Clone>(styles: &[TextStyle], f: impl Fn(&TextStyle) -> T) -> Option<T> {
    let mut it = styles.iter();
    let first = f(it.next()?);
    if it.all(|s| f(s) == first) { Some(first) } else { None }
}

/// The character styles the panel shows: the edited range's spans, else every span of every
/// selected text layer.
pub(crate) fn target_styles(doc: &Document, ids: &[NodeId], range: Option<(NodeId, usize, usize)>) -> Vec<TextStyle> {
    let mut out: Vec<TextStyle> = Vec::new();
    let mut push = |s: &TextStyle| {
        if !out.contains(s) {
            out.push(s.clone());
        }
    };
    for id in ids {
        let Some(t) = doc.node(*id).and_then(|n| n.kind.text()) else { continue };
        match range {
            Some((rid, a, b)) if rid == *id && a < b => {
                for (s, e, st) in t.spans() {
                    if s < b && e > a {
                        push(st);
                    }
                }
            }
            _ => {
                for (_, _, st) in t.spans() {
                    push(st);
                }
            }
        }
    }
    out
}

pub fn typography(app: &mut FigApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let ids: Vec<NodeId> = sel.iter().copied().filter(|i| matches!(doc.node(*i).map(|n| &n.kind), Some(NodeKind::Text(_)))).collect();
    if ids.is_empty() {
        return;
    }
    // While editing a range, character edits apply to it.
    let range = figcraft_engine::textedit::selected_range(&app.session);
    let editing = range.is_some();
    let styles = target_styles(doc, &ids, range);
    let apply_style = |app: &mut FigApp, key: &str, v: Value| commit_text(app, &ids, range, json!({ key: v }));

    let hdr = widgets::section_header(ui, tr("Typography"), &[(Icon::Styles, tr("Text styles"))], false);
    if hdr == Some(0) {
        crate::style_picker::open(app, ui, &ids, crate::style_picker::StyleSlot::Text);
    }
    crate::variable_ui::bound_row(app, ui, &ids, "characters", VariableType::String, Lead::Icon(Icon::Text, tr("Text")), f32::INFINITY);
    // A text style: its chip stands for the font rows (they show again when the chars differ).
    crate::style_picker::chip(app, ui, doc, &ids, crate::style_picker::StyleSlot::Text);
    if !matches!(crate::style_picker::text_link(app, doc, &ids), Some(Some(_))) {
        font_rows(app, ui, &ids, &styles, editing, &apply_style);
    }
    paragraph_rows(app, ui, doc, &ids, &styles, &apply_style);
    path_row(app, ui, doc, sel, &ids);
    ui.add_space(12.0);
}

/// Text on a path: where the text starts, which side it runs on, and taking it off the path; or,
/// with a text and a path selected, putting the text on the path.
fn path_row(app: &mut FigApp, ui: &mut Ui, doc: &Document, sel: &[NodeId], ids: &[NodeId]) {
    let paths: Option<Vec<(f64, bool)>> =
        ids.iter().map(|i| doc.node(*i).and_then(|n| n.kind.text()).and_then(|t| t.path.as_ref()).map(|p| (p.start(), p.flip))).collect();
    let Some(paths) = paths else {
        let other = sel.iter().find(|i| !ids.contains(i)).and_then(|i| doc.node(*i));
        if ids.len() == 1 && sel.len() == 2 && other.is_some_and(|n| figcraft_doc::geometry::stroke_geometry(n).is_some()) {
            row(ui, 32.0, |ui| {
                if widgets::button(ui, tr("Put text on path"), false, true).clicked() {
                    app.run("text.onPath", json!({ "ids": sel }));
                }
            });
        }
        return;
    };
    let start = paths.first().map(|p| p.0).filter(|s| paths.iter().all(|p| p.0 == *s));
    let flip = paths.iter().all(|p| p.1);
    row(ui, 32.0, |ui| {
        let lead = Lead::Icon(Icon::Pen, tr("Start on path"));
        match widgets::number_field(ui, Id::new("ty_path_start"), lead, start.map(|s| s * 100.0), "%", 96.0, 1.0) {
            Some(FieldEdit::Set(v)) => {
                app.run("text.pathOptions", json!({"ids": ids, "start": v / 100.0}));
            }
            Some(FieldEdit::Scrub(v, done)) => live(app, "Start on path", "text.pathOptions", json!({"ids": ids, "start": v / 100.0}), done),
            None => {}
        }
        if icons::button(ui, Icon::FlipV, 24.0, flip, tr("Flip text to the other side")).clicked() {
            app.run("text.pathOptions", json!({"ids": ids, "flip": !flip}));
        }
        if icons::button(ui, Icon::Detach, 24.0, false, tr("Remove text from path")).clicked() {
            app.run("text.removeFromPath", json!({ "ids": ids }));
        }
    });
}

/// Set character `props` on the selected chars of the text being edited, else on whole layers
/// (through `node.set`, which records overrides inside instances).
fn commit_text(app: &mut FigApp, ids: &[NodeId], range: Option<(NodeId, usize, usize)>, props: Value) {
    match range {
        Some((_, a, b)) if a != b => {
            app.run("text.style", text_style_params(ids, range, props));
        }
        _ => app.set_props(ids, props),
    }
}

/// `text.style` params for `props` on the selected chars of the text being edited, else on the
/// whole of every layer in `ids` (previews; commits go through [`commit_text`]).
fn text_style_params(ids: &[NodeId], range: Option<(NodeId, usize, usize)>, mut props: Value) -> Value {
    match range {
        Some((id, a, b)) if a != b => {
            props["id"] = json!(id);
            props["start"] = json!(a);
            props["end"] = json!(b);
        }
        _ => props["ids"] = json!(ids),
    }
    props
}

/// The font rows: family, style and size, line height and letter spacing.
fn font_rows(app: &mut FigApp, ui: &mut Ui, ids: &[NodeId], styles: &[TextStyle], editing: bool, apply_style: &dyn Fn(&mut FigApp, &str, Value)) {
    let t = Tokens::get(ui.ctx());
    let range = figcraft_engine::textedit::selected_range(&app.session);

    // Font family.
    let family = uniform(styles, |s| s.font_family.clone());
    let fstyle = uniform(styles, |s| s.font_style.clone());
    row(ui, 32.0, |ui| {
        let panel_id = Id::new("ty_font_panel");
        let open = crate::primitives::FloatingPanel::toggle(ui.ctx(), panel_id, false);
        let response = crate::primitives::Button::new(family.as_deref().unwrap_or(tr(MIXED)))
            .variant(crate::primitives::ButtonVariant::Secondary)
            .behavior(crate::primitives::ButtonBehavior::Select { expanded: open })
            .width(crate::primitives::ControlWidth::Fill)
            .align(crate::primitives::ButtonAlign::Start)
            .show(ui);
        if crate::primitives::FloatingPanel::toggle(ui.ctx(), panel_id, response.clicked()) {
            let ctx = ui.ctx().clone();
            let initial = pos2(response.rect.left(), response.rect.bottom() + 4.0);
            let (picked, mut close) = font_family_panel(&ctx, panel_id, initial, family.as_deref(), response.clicked());
            if let Some(font) = picked.picked {
                remember_family(&ctx, &font);
                apply_style(app, "fontFamily", json!(font));
                close = true;
            } else if let Some(hovered) = picked.hovered.or(picked.highlighted) {
                app.preview_text(&ctx, "text.style", text_style_params(ids, range, json!({"fontFamily": hovered})));
            }
            if close {
                crate::primitives::FloatingPanel::dismiss(&ctx, panel_id);
            }
        }
    });

    // Style + size.
    let half = widgets::col_width(ui.available_width());
    row(ui, 32.0, |ui| {
        let resp = crate::primitives::Select::trigger_with_variant(
            ui,
            fstyle.as_deref().unwrap_or(tr(MIXED)),
            half,
            None,
            crate::primitives::SelectVariant::Outlined,
        );
        let fam = family.clone();
        menu_popup(&resp).width(200.0).show(|ui| match &fam {
            Some(fam) => {
                let list = figcraft_text::family_styles(fam);
                if list.is_empty() {
                    widgets::label(ui, tr("Font not installed"), t.text_tertiary);
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for s in list {
                        let mark = if fstyle.as_deref() == Some(s.as_str()) { "✓ " } else { "   " };
                        let (clicked, hovered) = menu_item_hover(ui, &format!("{mark}{s}"));
                        if clicked {
                            apply_style(app, "fontStyle", json!(s));
                            ui.close();
                        } else if hovered {
                            // The canvas shows the style (a variable font's named instance too).
                            app.preview_text(ui.ctx(), "text.style", text_style_params(ids, range, json!({"fontStyle": s})));
                        }
                    }
                });
            }
            None => widgets::label(ui, tr("Pick one font family first"), t.text_tertiary),
        });
        let size = uniform(styles, |s| s.font_size);
        // The whole layer's size can follow a variable; a selected range just takes a value.
        let size_field = if editing {
            crate::primitives::NumberField::new(Id::new("ty_size"), Lead::Named(tr("Font size")), size, "")
                .options(&SIZES)
                .show(ui, crate::primitives::ControlWidth::Fixed(half))
        } else {
            crate::variable_ui::number_field_with_options(app, ui, ids, "fontSize", Id::new("ty_size"), size, half, &SIZES)
        };
        match size_field.edit.and_then(|edit| edit.settled()) {
            Some(FieldEdit::Set(v)) => apply_style(app, "fontSize", json!(v.clamp(1.0, 5000.0))),
            // The text editing session already groups undo; just apply.
            Some(FieldEdit::Scrub(v, _)) if editing => apply_style(app, "fontSize", json!(v.clamp(1.0, 5000.0))),
            Some(FieldEdit::Scrub(v, done)) => {
                live(app, "Font size", "node.set", json!({"ids": ids, "props": {"fontSize": v.clamp(1.0, 5000.0)}}), done)
            }
            None => {}
        }
        if let Some(value) = size_field.selected_option {
            apply_style(app, "fontSize", json!(value));
        }
    });

    // Line height + letter spacing.
    row(ui, 32.0, |ui| {
        let lh = uniform(styles, |s| s.line_height);
        let lh_label = lh.map(|l| l.label()).unwrap_or_else(|| tr(MIXED).into());
        let field = SpacingField { field: "lineHeight", id: Id::new("ty_lh"), glyph: LeadGlyph::LineHeight, tip: tr("Line height") };
        let scrub = line_height_scrub_start(lh, styles.first()).map(|start| (start, 0.1));
        if let Some(edit) = spacing_field(app, ui, ids, &field, &lh_label, half, editing, scrub) {
            let value = match &edit {
                SpacingEdit::Typed(text) => parse_line_height(text, lh),
                SpacingEdit::Scrub(percent, _) => Some(json!(format!("{:.1}%", (percent.max(0.0) * 10.0).round() / 10.0))),
            };
            if let Some(value) = value {
                commit_spacing(app, ids, range, &field, &edit, value);
            }
        }
        let ls = uniform(styles, |s| s.letter_spacing);
        let ls_label = ls.map(|l| l.label()).unwrap_or_else(|| tr(MIXED).into());
        let field = SpacingField { field: "letterSpacing", id: Id::new("ty_ls"), glyph: LeadGlyph::LetterSpacing, tip: tr("Letter spacing") };
        if let Some(edit) = spacing_field(app, ui, ids, &field, &ls_label, half, editing, None)
            && let Some(v) = parse_letter_spacing(&edit.text(), ls)
        {
            commit_spacing(app, ids, range, &field, &edit, v);
        }
    });
}

/// Paragraph spacing, resizing, alignment and the Type settings popover.
fn paragraph_rows(
    app: &mut FigApp,
    ui: &mut Ui,
    doc: &Document,
    ids: &[NodeId],
    styles: &[TextStyle],
    apply_style: &dyn Fn(&mut FigApp, &str, Value),
) {
    // Alignment + Type settings.
    let align = common(doc, ids, |n| n.kind.text().map(|t| (t.align_h, t.align_v))).flatten();
    row(ui, 32.0, |ui| {
        let group_width = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(72.0);
        let items = [
            crate::primitives::Segment::icon(Icon::TypeAlignLeft, tr("Align left")),
            crate::primitives::Segment::icon(Icon::TypeAlignCenter, tr("Align center")),
            crate::primitives::Segment::icon(Icon::TypeAlignRight, tr("Align right")),
        ];
        let active = align.and_then(|(h, _)| match h {
            TextAlignH::Left => Some(0),
            TextAlignH::Center => Some(1),
            TextAlignH::Right => Some(2),
            TextAlignH::Justified => None,
        });
        if let Some(i) = crate::primitives::SegmentedControl::new(&items, active).width(crate::primitives::ControlWidth::Fixed(group_width)).show(ui)
        {
            let v = ["left", "center", "right"].get(i).copied().unwrap_or("left");
            app.set_props(ids, json!({"textAlign": v}));
        }
        let items = [
            crate::primitives::Segment::icon(Icon::TypeAlignTop, tr("Align top")),
            crate::primitives::Segment::icon(Icon::TypeAlignMiddle, tr("Align middle")),
            crate::primitives::Segment::icon(Icon::TypeAlignBottom, tr("Align bottom")),
        ];
        let active = align.map(|(_, v)| match v {
            TextAlignV::Top => 0,
            TextAlignV::Middle => 1,
            TextAlignV::Bottom => 2,
        });
        if let Some(i) = crate::primitives::SegmentedControl::new(&items, active).width(crate::primitives::ControlWidth::Fixed(group_width)).show(ui)
        {
            let v = ["top", "middle", "bottom"].get(i).copied().unwrap_or("top");
            app.set_props(ids, json!({"textAlignVertical": v}));
        }
        let open_id = Id::new("ty_settings_open");
        let open = crate::primitives::FloatingPanel::toggle(ui.ctx(), open_id, false);
        let more = crate::primitives::Button::icon(Icon::TypeSettings, tr("Type settings"))
            .icon_size(24.0)
            .behavior(crate::primitives::ButtonBehavior::Toggle { selected: open })
            .show(ui);
        if crate::primitives::FloatingPanel::toggle(ui.ctx(), open_id, more.clicked()) {
            let ctx = ui.ctx().clone();
            let initial = pos2(more.rect.left() - 248.0, more.rect.top());
            let tab_id = Id::new("ty_settings_tab");
            let variable = variable_face(styles).is_some();
            if type_settings_panel(&ctx, open_id, tab_id, initial, variable, |ui, tab| {
                type_settings(app, ui, doc, ids, styles, apply_style, tab);
            }) {
                crate::primitives::FloatingPanel::dismiss(&ctx, open_id);
            }
        }
    });
}

/// A menu item that also reports the pointer being over it: (clicked, hovered).
fn menu_item_hover(ui: &mut Ui, label: &str) -> (bool, bool) {
    let r = crate::right::menu_row(ui, label, "", 0.0, |_, _, _| {});
    (r.clicked(), r.hovered())
}

/// Run `cmd` as a live preview while scrubbing; the gesture becomes one undo step when `done`.
/// (While text is edited, run commands directly: the editing session already groups undo.)
pub(crate) fn live(app: &mut FigApp, label: &str, cmd: &str, params: Value, done: bool) {
    let _ = app.session.begin_interaction(label);
    if let Err(e) = app.session.execute(cmd, &params) {
        app.toast(e.to_string());
    }
    if done {
        let _ = app.session.commit_interaction();
    }
}

/// The variable font the shown styles use (one family, one variable face), if any.
fn variable_face(styles: &[TextStyle]) -> Option<Arc<figcraft_text::Face>> {
    let family = uniform(styles, |s| s.font_family.clone())?;
    let face = figcraft_text::find_face(&family, &styles.first()?.font_style)?;
    (face.is_variable() && face.family.eq_ignore_ascii_case(&family)).then_some(face)
}

/// Common three-tab floating shell for advanced type settings.
pub(crate) fn type_settings_panel(
    ctx: &egui::Context,
    id: Id,
    tab_id: Id,
    initial: egui::Pos2,
    variable: bool,
    add_body: impl FnOnce(&mut Ui, usize),
) -> bool {
    let max_height = (ctx.content_rect().height() - 100.0).max(200.0);
    let mut close = false;
    let mut tab: usize = ctx.data(|data| data.get_temp(tab_id)).unwrap_or(0);
    if tab == 2 && !variable {
        tab = 0;
    }
    crate::primitives::FloatingPanel::new(id, initial).width(crate::primitives::FloatingPanelWidth::Compact).show_with_header(ctx, |ui, header| {
        let t = Tokens::get(ui.ctx());
        ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));
        let tabs = [
            crate::primitives::Tab::new(tr(TABS[0])),
            crate::primitives::Tab::new(tr(TABS[1])),
            crate::primitives::Tab::new(tr(TABS[2])).disabled(!variable),
        ];
        let tab_rect = Rect::from_min_size(pos2(header.left() + 4.0, header.top() + 4.0), vec2(200.0, 32.0));
        let mut tab_ui = ui.new_child(egui::UiBuilder::new().max_rect(tab_rect));
        if let Some(next) =
            crate::primitives::Tabs::new(tab_id, &tabs, Some(tab)).width(crate::primitives::ControlWidth::Fixed(200.0)).show(&mut tab_ui)
        {
            tab = next;
        }
        let close_rect = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
        close = crate::primitives::FloatingPanel::close_button(ui, id.with("close"), close_rect, |painter, rect| {
            icons::paint(painter, rect.shrink(6.0), Icon::Close, t.icon);
        });
        let body_height = if tab == 1 { max_height.min(TYPE_SETTINGS_BASIC_HEIGHT) } else { max_height };
        egui::ScrollArea::vertical()
            .id_salt(if tab == 1 { "ty_settings_details_scroll" } else { "ty_settings_scroll" })
            .max_height(body_height)
            .auto_shrink([false, true])
            .show(ui, |ui| add_body(ui, tab));
    });
    ctx.data_mut(|data| data.insert_temp(tab_id, tab));
    close
}

/// Controls that belong to a reusable text style. Node-only options from the layer panel are
/// omitted here because a style cannot store them.
pub(crate) fn text_style_settings(ui: &mut Ui, style: &TextStyle, tab: usize, budget: &mut usize) -> Option<(TextStyle, Option<bool>)> {
    use crate::primitives::{Button, ControlWidth, NumberField, Segment, SegmentedControl, Select, Slider, Switch};

    let t = Tokens::get(ui.ctx());
    let mut next = style.clone();
    let mut scrub = None;
    ui.set_width(240.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    match tab {
        0 => {
            let (section, _) = ui.allocate_exact_size(vec2(240.0, 144.0), Sense::hover());
            let preview = Rect::from_min_size(section.min + vec2(16.0, 16.0), vec2(208.0, 120.0));
            ui.painter().rect_filled(preview, CornerRadius::same(5), if t.dark { Color32::from_rgb(0x38, 0x38, 0x38) } else { t.hover });
            let sample = "Rag 123";
            if let Some(texture) = crate::font_preview::texture(ui.ctx(), style, sample, budget) {
                let width = (texture.size_vec2().x / ui.ctx().pixels_per_point()).min(preview.width() - 16.0);
                crate::font_preview::paint(
                    ui.painter(),
                    &texture,
                    pos2(preview.center().x - width / 2.0, preview.center().y),
                    preview.width() - 16.0,
                    preview.height(),
                    t.text,
                );
            } else {
                ui.painter().text(preview.center(), Align2::CENTER_CENTER, sample, regular(style.font_size.clamp(6.0, 60.0) as f32), t.text);
            }
            let (basic_rect, _) = ui.allocate_exact_size(vec2(240.0, 108.0), Sense::hover());
            ui.painter().hline(basic_rect.x_range(), basic_rect.bottom() - 0.5, Stroke::new(1.0, t.border));
            let mut basic = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(basic_rect.min + vec2(16.0, 0.0), vec2(208.0, 108.0))));
            basic.spacing_mut().item_spacing.y = 0.0;
            dialog_row(&mut basic, tr("Decoration"), 112.0, 112.0, |ui| {
                let items = [
                    Segment::icon(Icon::TypeDialogNone, tr("None")),
                    Segment::icon(Icon::TypeDialogUnderline, tr("Underline")),
                    Segment::icon(Icon::TypeDialogStrike, tr("Strikethrough")),
                ];
                let active = match style.decoration {
                    TextDecoration::None => 0,
                    TextDecoration::Underline => 1,
                    TextDecoration::Strikethrough => 2,
                };
                if let Some(index) = SegmentedControl::new(&items, Some(active)).width(ControlWidth::Fixed(96.0)).show(ui) {
                    next.decoration = [TextDecoration::None, TextDecoration::Underline, TextDecoration::Strikethrough][index];
                }
            });
            dialog_row(&mut basic, tr("Letter case|dialog"), 88.0, 88.0, |ui| {
                let items = [
                    Segment::icon(Icon::TypeDialogNone, tr("As typed")),
                    Segment::icon(Icon::TypeDialogUpper, tr("Uppercase")),
                    Segment::icon(Icon::TypeDialogLower, tr("Lowercase")),
                    Segment::icon(Icon::TypeDialogTitle, tr("Title case")),
                    Segment::icon(Icon::TypeDialogSmallCaps, tr("Small caps")),
                ];
                let active = match style.case {
                    TextCase::Original => 0,
                    TextCase::Upper => 1,
                    TextCase::Lower => 2,
                    TextCase::Title => 3,
                    TextCase::SmallCaps => 4,
                };
                if let Some(index) = SegmentedControl::new(&items, Some(active)).width(ControlWidth::Fixed(120.0)).show(ui) {
                    next.case = [TextCase::Original, TextCase::Upper, TextCase::Lower, TextCase::Title, TextCase::SmallCaps][index];
                }
            });
            let (advanced_rect, _) = ui.allocate_exact_size(vec2(240.0, 192.0), Sense::hover());
            let mut advanced =
                ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(advanced_rect.min + vec2(16.0, 8.0), vec2(208.0, 176.0))));
            advanced.spacing_mut().item_spacing.y = 0.0;
            dialog_row(&mut advanced, tr("Font size"), 112.0, 112.0, |ui| {
                let response = NumberField::new(Id::new("text_style_settings_size"), Lead::None, Some(style.font_size), "")
                    .options(&SIZES)
                    .show(ui, ControlWidth::Fixed(96.0));
                match response.edit.and_then(|edit| edit.settled()) {
                    Some(FieldEdit::Set(value)) => next.font_size = value.clamp(1.0, 5000.0),
                    Some(FieldEdit::Scrub(value, done)) => {
                        next.font_size = value.clamp(1.0, 5000.0);
                        scrub = Some(done);
                    }
                    None => {}
                }
                if let Some(value) = response.selected_option {
                    next.font_size = value;
                }
            });
            dialog_row(&mut advanced, tr("Line height"), 112.0, 112.0, |ui| {
                if let Some(change) = style_line_height_field(ui, Id::new("text_style_settings_line_height"), style, 96.0) {
                    let (value, done) = change.into_value_and_scrub();
                    if let Some(height) = line_height_of(&value) {
                        next.line_height = height;
                        scrub = done;
                    }
                }
            });
            dialog_row(&mut advanced, tr("Letter spacing"), 112.0, 112.0, |ui| {
                if let Some(change) = style_letter_spacing_field(ui, Id::new("text_style_settings_letter_spacing"), style, 96.0) {
                    let (value, done) = change.into_value_and_scrub();
                    if let Some(spacing) = letter_spacing_of(&value) {
                        next.letter_spacing = spacing;
                        scrub = done;
                    }
                }
            });
        }
        1 => {
            ui.set_min_height(TYPE_SETTINGS_BASIC_HEIGHT);
            details_section(ui, tr("Decoration"), |ui| {
                dialog_row(ui, tr("Decoration style"), 112.0, 112.0, |ui| {
                    let options = [tr("Solid"), tr("Dotted"), tr("Wavy")];
                    let selected = match style.look().style {
                        DecorationStyle::Solid => 0,
                        DecorationStyle::Dotted => 1,
                        DecorationStyle::Wavy => 2,
                    };
                    if let Some(index) = Select::new(&options, selected).width(ControlWidth::Fixed(96.0)).show(ui) {
                        next.set_look(|look| look.style = [DecorationStyle::Solid, DecorationStyle::Dotted, DecorationStyle::Wavy][index]);
                    }
                });
                dialog_row(ui, tr("Thickness"), 112.0, 112.0, |ui| {
                    let current = match style.look().thickness {
                        LineHeight::Pixels(value) => Some(value),
                        _ => None,
                    };
                    let field = NumberField::new(Id::new("text_style_settings_thickness"), Lead::None, current, "")
                        .empty(widgets::Empty::Auto)
                        .show(ui, ControlWidth::Fixed(96.0));
                    match field.edit.and_then(|edit| edit.settled()) {
                        Some(FieldEdit::Set(value)) => next.set_look(|look| look.thickness = LineHeight::Pixels(value.max(0.0))),
                        Some(FieldEdit::Scrub(value, done)) => {
                            next.set_look(|look| look.thickness = LineHeight::Pixels(value.max(0.0)));
                            scrub = Some(done);
                        }
                        None => {}
                    }
                });
                let (switch_rect, _) = ui.allocate_exact_size(vec2(208.0, 32.0), Sense::hover());
                if Switch::new(Id::new("text_style_settings_skip_ink"), style.look().skip_ink, tr("Skip ink")).show_at(ui, switch_rect).clicked() {
                    next.set_look(|look| look.skip_ink = !look.skip_ink);
                }
            });
            details_section(ui, tr("Letter case"), |ui| {
                dialog_row(ui, tr("Letter case|dialog"), 88.0, 88.0, |ui| {
                    let items = [
                        Segment::icon(Icon::TypeDialogNone, tr("As typed")),
                        Segment::icon(Icon::TypeDialogUpper, tr("Uppercase")),
                        Segment::icon(Icon::TypeDialogLower, tr("Lowercase")),
                        Segment::icon(Icon::TypeDialogTitle, tr("Title case")),
                        Segment::icon(Icon::TypeDialogSmallCaps, tr("Small caps")),
                    ];
                    let active = match style.case {
                        TextCase::Original => 0,
                        TextCase::Upper => 1,
                        TextCase::Lower => 2,
                        TextCase::Title => 3,
                        TextCase::SmallCaps => 4,
                    };
                    if let Some(index) = SegmentedControl::new(&items, Some(active)).width(ControlWidth::Fixed(120.0)).show(ui) {
                        next.case = [TextCase::Original, TextCase::Upper, TextCase::Lower, TextCase::Title, TextCase::SmallCaps][index];
                    }
                });
            });
        }
        2 => {
            ui.set_min_height(TYPE_SETTINGS_BASIC_HEIGHT);
            if let Some(face) = figcraft_text::find_face(&style.font_family, &style.font_style).filter(|face| face.is_variable()) {
                let values = face.axis_values(style);
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                    ui.set_width(208.0);
                    ui.label(egui::RichText::new(&face.family).color(t.text_secondary));
                    for (index, axis) in face.axes.iter().enumerate().filter(|(_, axis)| !axis.hidden) {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(&axis.name).color(t.text));
                        let value = values.get(index).copied().unwrap_or(axis.default);
                        let slider = Slider::new(Id::new(("text_style_settings_axis", &axis.tag)), &axis.name)
                            .range(axis.min, axis.max)
                            .reference(axis.default)
                            .value(Some(value))
                            .width(ControlWidth::Fixed(208.0));
                        if let Some((value, done)) = slider.show(ui).change {
                            next.variations.insert(axis.tag.clone(), axis.clamp(value));
                            scrub = Some(done);
                        }
                    }
                    if !style.variations.is_empty()
                        && Button::new(tr("Reset to style")).variant(crate::primitives::ButtonVariant::Ghost).show(ui).clicked()
                    {
                        next.variations.clear();
                    }
                });
            }
        }
        _ => {}
    }
    (next != *style || scrub == Some(true)).then_some((next, scrub))
}

/// The Type settings popover: Basics (decoration, case, list, indent, truncation), Details
/// (OpenType features) and, for variable fonts, Variable (axis sliders).
fn dialog_row<R>(ui: &mut Ui, label: &str, label_width: f32, text_width: f32, add_control: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(208.0, 32.0), Sense::hover());
    let mut row_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    row_ui.spacing_mut().item_spacing.x = 0.0;
    let (label_rect, _) = row_ui.allocate_exact_size(vec2(label_width, 32.0), Sense::hover());
    let text_rect = Rect::from_min_size(label_rect.min, vec2(text_width, 32.0));
    let mut label_ui = row_ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    label_ui.add(egui::Label::new(egui::RichText::new(label).font(regular(10.0)).color(t.text)).wrap().halign(egui::Align::Min));
    add_control(&mut row_ui)
}

fn type_settings_basics(
    app: &mut FigApp,
    ui: &mut Ui,
    doc: &Document,
    ids: &[NodeId],
    styles: &[TextStyle],
    apply_style: &dyn Fn(&mut FigApp, &str, Value),
) {
    use crate::primitives::{ControlWidth, NumberField, Segment, SegmentedControl, Select, SelectVariant};

    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0;
    let (preview_section, _) = ui.allocate_exact_size(vec2(240.0, 144.0), Sense::hover());
    let preview = Rect::from_min_size(preview_section.min + vec2(16.0, 16.0), vec2(208.0, 120.0));
    ui.painter().rect_filled(preview, CornerRadius::same(5), Color32::from_rgb(0x38, 0x38, 0x38));
    ui.painter().text(preview.left_center() + vec2(16.0, 0.0), Align2::LEFT_CENTER, tr("Preview|typography"), regular(11.0), t.text_secondary);

    let (basic_section, _) = ui.allocate_exact_size(vec2(240.0, 108.0), Sense::hover());
    ui.painter().hline(basic_section.x_range(), basic_section.bottom() - 0.5, Stroke::new(1.0, t.border));
    let mut basic = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(basic_section.min + vec2(16.0, 0.0), vec2(208.0, 96.0))));
    basic.spacing_mut().item_spacing.y = 0.0;
    let align = common(doc, ids, |n| n.kind.text().map(|text| text.align_h)).flatten();
    dialog_row(&mut basic, tr("Alignment"), 112.0, 112.0, |ui| {
        let items = [
            Segment::icon(Icon::TypeAlignLeft, tr("Align left")),
            Segment::icon(Icon::TypeAlignCenter, tr("Align center")),
            Segment::icon(Icon::TypeAlignRight, tr("Align right")),
            Segment::icon(Icon::TypeDialogJustify, tr("Justify")),
        ];
        let selected = align.map(|value| match value {
            TextAlignH::Left => 0,
            TextAlignH::Center => 1,
            TextAlignH::Right => 2,
            TextAlignH::Justified => 3,
        });
        if let Some(index) = SegmentedControl::new(&items, selected).width(ControlWidth::Fixed(96.0)).show(ui) {
            let value = ["left", "center", "right", "justified"].get(index).copied().unwrap_or("left");
            app.set_props(ids, json!({"textAlign": value}));
        }
    });
    let decoration = uniform(styles, |style| style.decoration);
    let mut show_decoration_details = ui.data(|data| data.get_temp::<bool>(Id::new("ty_deco_details"))).unwrap_or(false);
    dialog_row(&mut basic, tr("Decoration"), 112.0, 112.0, |ui| {
        let items = [
            Segment::icon(Icon::TypeDialogNone, tr("None")),
            Segment::icon(Icon::TypeDialogUnderline, tr("Underline")),
            Segment::icon(Icon::TypeDialogStrike, tr("Strikethrough")),
        ];
        let selected = decoration.map(|value| match value {
            TextDecoration::None => 0,
            TextDecoration::Underline => 1,
            TextDecoration::Strikethrough => 2,
        });
        if let Some(index) = SegmentedControl::new(&items, selected).width(ControlWidth::Fixed(72.0)).show(ui) {
            let value = ["none", "underline", "strikethrough"].get(index).copied().unwrap_or("none");
            apply_style(app, "textDecoration", json!(value));
        }
        ui.add_space(8.0);
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 24.0), Sense::hover());
        if crate::primitives::Button::icon(Icon::ChevronRight, tr("Decoration style"))
            .variant(crate::primitives::ButtonVariant::Secondary)
            .icon_size(10.0)
            .show_at(ui, Id::new("ty_deco_more"), rect)
            .clicked()
        {
            show_decoration_details = !show_decoration_details;
        }
    });
    ui.data_mut(|data| data.insert_temp(Id::new("ty_deco_details"), show_decoration_details));
    let text_case = uniform(styles, |style| style.case);
    dialog_row(&mut basic, tr("Letter case|dialog"), 88.0, 64.0, |ui| {
        let items = [
            Segment::icon(Icon::TypeDialogNone, tr("As typed")),
            Segment::icon(Icon::TypeDialogUpper, tr("Uppercase")),
            Segment::icon(Icon::TypeDialogLower, tr("Lowercase")),
            Segment::icon(Icon::TypeDialogTitle, tr("Title case")),
            Segment::icon(Icon::TypeDialogSmallCaps, tr("Small caps")),
        ];
        let selected = text_case.map(|value| match value {
            TextCase::Original => 0,
            TextCase::Upper => 1,
            TextCase::Lower => 2,
            TextCase::Title => 3,
            TextCase::SmallCaps => 4,
        });
        if let Some(index) = SegmentedControl::new(&items, selected).width(ControlWidth::Fixed(120.0)).show(ui) {
            let value = ["original", "upper", "lower", "title", "smallCaps"].get(index).copied().unwrap_or("original");
            apply_style(app, "textCase", json!(value));
        }
    });

    let (advanced_section, _) = ui.allocate_exact_size(vec2(240.0, 192.0), Sense::hover());
    let mut advanced = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(advanced_section.min + vec2(16.0, 8.0), vec2(208.0, 160.0))));
    advanced.spacing_mut().item_spacing.y = 0.0;
    let trim = common(doc, ids, |n| n.kind.text().map(|text| text.leading_trim)).flatten();
    dialog_row(&mut advanced, tr("Vertical trim"), 160.0, 160.0, |ui| {
        let items =
            [Segment::icon(Icon::TypeDialogTrimStandard, tr("Standard")), Segment::icon(Icon::TypeDialogTrimCap, tr("Cap height to baseline"))];
        let selected = trim.map(|value| usize::from(value == LeadingTrim::CapHeight));
        if let Some(index) = SegmentedControl::new(&items, selected).width(ControlWidth::Fixed(48.0)).show(ui) {
            app.set_props(ids, json!({"leadingTrim": if index == 1 { "capHeight" } else { "none" }}));
        }
    });
    let range = figcraft_engine::textedit::selected_range(&app.session);
    let list = common(doc, ids, |node| {
        node.kind.text().and_then(|text| {
            let (start, end) = range.filter(|range| range.0 == node.id).map_or((0, text.char_count()), |(_, start, end)| (start, end));
            text.common_para_list(start, end).map(|paragraph| paragraph.list)
        })
    })
    .flatten();
    dialog_row(&mut advanced, tr("List style"), 136.0, 112.0, |ui| {
        let items = [
            Segment::icon(Icon::TypeDialogNone, tr("No list")),
            Segment::icon(Icon::TypeDialogListBullets, tr("Bulleted list")),
            Segment::icon(Icon::TypeDialogListNumbered, tr("Numbered list")),
        ];
        let selected = list.map(|value| match value {
            ListStyle::None => 0,
            ListStyle::Bulleted => 1,
            ListStyle::Numbered => 2,
        });
        if let Some(index) = SegmentedControl::new(&items, selected).width(ControlWidth::Fixed(72.0)).show(ui) {
            let value = ["none", "bulleted", "numbered"].get(index).copied().unwrap_or("none");
            app.run("text.listStyle", json!({"ids": ids, "style": value}));
        }
    });
    let spacing = common(doc, ids, |node| node.kind.text().map(|text| text.paragraph_spacing.to_bits())).flatten().map(f64::from_bits);
    dialog_row(&mut advanced, tr("Paragraph spacing"), 136.0, 111.0, |ui| {
        let result = NumberField::new(Id::new("ty_dialog_paragraph_spacing"), Lead::None, spacing, "").show(ui, ControlWidth::Fixed(72.0));
        match result.edit.and_then(|edit| edit.settled()) {
            Some(FieldEdit::Set(value)) => app.set_props(ids, json!({"paragraphSpacing": value.clamp(0.0, 10_000.0)})),
            Some(FieldEdit::Scrub(value, done)) => {
                live(app, "Paragraph spacing", "node.set", json!({"ids": ids, "props": {"paragraphSpacing": value.clamp(0.0, 10_000.0)}}), done);
            }
            None => {}
        }
    });
    let resize = common(doc, ids, |node| node.kind.text().map(|text| text.auto_resize)).flatten();
    dialog_row(&mut advanced, tr("Truncate text"), 160.0, 160.0, |ui| {
        let items =
            [Segment::icon(Icon::TypeDialogNone, tr("Don't truncate")), Segment::icon(Icon::TypeDialogTruncate, tr("Truncate with an ellipsis"))];
        let selected = resize.map(|value| usize::from(value == TextAutoResize::Truncate));
        if let Some(index) = SegmentedControl::new(&items, selected).width(ControlWidth::Fixed(48.0)).show(ui) {
            if index == 1 {
                app.set_props(ids, json!({"textAutoResize": "truncate"}));
            } else if !matches!(resize, Some(TextAutoResize::Height | TextAutoResize::WidthAndHeight)) {
                app.set_props(ids, json!({"textAutoResize": "none"}));
            }
        }
    });
    dialog_row(&mut advanced, tr("Line break style"), 112.0, 96.0, |ui| {
        let options = [tr("Automatic")];
        let _ = Select::new(&options, 0).width(ControlWidth::Fixed(96.0)).variant(SelectVariant::Outlined).show(ui);
    });
    if show_decoration_details && matches!(decoration, Some(TextDecoration::Underline | TextDecoration::Strikethrough)) {
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
            decoration_details(app, ui, styles, apply_style, decoration == Some(TextDecoration::Underline), ui.available_width());
        });
    }
}

fn type_settings(
    app: &mut FigApp,
    ui: &mut Ui,
    doc: &Document,
    ids: &[NodeId],
    styles: &[TextStyle],
    apply_style: &dyn Fn(&mut FigApp, &str, Value),
    tab: usize,
) {
    match tab {
        0 => type_settings_basics(app, ui, doc, ids, styles, apply_style),
        1 => type_settings_details(app, ui, doc, ids, styles, apply_style),
        2 => {
            if let Some(face) = variable_face(styles) {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    variable_axes(app, ui, ids, styles, &face);
                });
            }
        }
        _ => {}
    }
}

/// The Details body is one scrollable stack of the groups in the reference design. Every
/// interactive field is a primitive; unsupported OpenType features stay visible but disabled.
fn type_settings_details(
    app: &mut FigApp,
    ui: &mut Ui,
    doc: &Document,
    ids: &[NodeId],
    styles: &[TextStyle],
    apply_style: &dyn Fn(&mut FigApp, &str, Value),
) {
    use crate::primitives::{ControlWidth, NumberField, Segment, SegmentedControl};

    ui.set_width(240.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    let face = uniform(styles, |style| (style.font_family.clone(), style.font_style.clone()))
        .and_then(|(family, style)| figcraft_text::find_face(&family, &style));
    let face = face.as_deref();

    details_section(ui, tr("Indentation"), |ui| {
        for (key, label, value) in [
            (
                "hangingPunctuation",
                tr("Hanging punctuation"),
                common(doc, ids, |node| node.kind.text().map(|text| text.hanging_punctuation)).flatten(),
            ),
            ("hangingList", tr("Hanging list"), common(doc, ids, |node| node.kind.text().map(|text| text.hanging_list)).flatten()),
        ] {
            if let Some(enabled) = details_toggle(ui, label, value, true) {
                app.set_props(ids, json!({key: enabled}));
            }
        }
        let indent = common(doc, ids, |node| node.kind.text().map(|text| text.paragraph_indent.to_bits())).flatten().map(f64::from_bits);
        let start_aligned =
            common(doc, ids, |node| node.kind.text().map(|text| text.align_h != TextAlignH::Center && text.align_h != TextAlignH::Justified))
                .flatten();
        let edit = dialog_row(ui, tr("Paragraph indent"), 136.0, 136.0, |ui| {
            ui.add_enabled_ui(start_aligned != Some(false), |ui| {
                NumberField::new(Id::new("ty_details_indent"), Lead::None, indent, "")
                    .show(ui, ControlWidth::Fixed(72.0))
                    .edit
                    .and_then(|edit| edit.settled())
            })
            .inner
        });
        match edit {
            Some(FieldEdit::Set(value)) => app.set_props(ids, json!({"paragraphIndent": value.clamp(0.0, 10_000.0)})),
            Some(FieldEdit::Scrub(value, done)) => {
                live(app, "Paragraph indent", "node.set", json!({"ids": ids, "props": {"paragraphIndent": value.clamp(0.0, 10_000.0)}}), done);
            }
            None => {}
        }
    });

    details_section(ui, tr("Letter case"), |ui| {
        let current = uniform(styles, |style| style.case);
        let selected = current.map(|value| match value {
            TextCase::Original => 0,
            TextCase::Upper => 1,
            TextCase::Lower => 2,
            TextCase::Title => 3,
            TextCase::SmallCaps => 4,
        });
        let changed = dialog_row(ui, tr("Letter case|dialog"), 88.0, 88.0, |ui| {
            let items = [
                Segment::icon(Icon::TypeDialogNone, tr("As typed")),
                Segment::icon(Icon::TypeDialogUpper, tr("Uppercase")),
                Segment::icon(Icon::TypeDialogLower, tr("Lowercase")),
                Segment::icon(Icon::TypeDialogTitle, tr("Title case")),
                Segment::icon(Icon::TypeDialogSmallCaps, tr("Small caps")),
            ];
            SegmentedControl::new(&items, selected).width(ControlWidth::Fixed(120.0)).show(ui)
        });
        if let Some(index) = changed {
            let value = ["original", "upper", "lower", "title", "smallCaps"].get(index).copied().unwrap_or("original");
            apply_style(app, "textCase", json!(value));
        }
        details_feature(app, ui, doc, ids, face, "case", tr("Case-sensitive forms"), false);
        details_feature(app, ui, doc, ids, face, "cpsp", tr("Capital spacing"), false);
    });

    details_section(ui, tr("Numbers"), |ui| {
        details_feature(app, ui, doc, ids, face, "tnum", tr("Tabular numbers"), false);
        details_feature(app, ui, doc, ids, face, "frac", tr("Fractions"), false);
        details_feature(app, ui, doc, ids, face, "zero", tr("Slashed zero"), false);
    });

    details_section(ui, tr("Letter forms"), |ui| {
        details_feature(app, ui, doc, ids, face, "liga", tr("Standard ligatures"), true);
        details_feature(app, ui, doc, ids, face, "dlig", tr("Discretionary ligatures"), false);
        details_feature(app, ui, doc, ids, face, "calt", tr("Contextual alternates"), true);
        details_feature(app, ui, doc, ids, face, "ordn", tr("Ordinals"), false);
        details_feature(app, ui, doc, ids, face, "smcp", tr("Small capitals"), false);
    });

    details_section(ui, tr("Stylistic sets"), |ui| {
        for (tag, label) in [
            ("ss01", tr("Stylistic set 1")),
            ("ss02", tr("Stylistic set 2")),
            ("ss03", tr("Stylistic set 3")),
            ("ss04", tr("Stylistic set 4")),
            ("ss05", tr("Stylistic set 5")),
        ] {
            details_feature(app, ui, doc, ids, face, tag, label, false);
        }
    });

    details_section(ui, tr("Horizontal spacing"), |ui| {
        details_feature(app, ui, doc, ids, face, "kern", tr("Kerning pairs"), true);
    });
}

fn details_section(ui: &mut Ui, title: &str, add_rows: impl FnOnce(&mut Ui)) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
        ui.set_width(208.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_sized(vec2(208.0, 32.0), egui::Label::new(egui::RichText::new(title).font(semibold(10.5)).color(t.text)).halign(egui::Align::Min));
        add_rows(ui);
        ui.add_space(8.0);
    });
    let (separator, _) = ui.allocate_exact_size(vec2(240.0, 1.0), Sense::hover());
    ui.painter().hline(separator.x_range(), separator.center().y, Stroke::new(1.0, t.border));
}

fn details_toggle(ui: &mut Ui, label: &str, value: Option<bool>, supported: bool) -> Option<bool> {
    use crate::primitives::{ControlWidth, Segment, SegmentedControl};
    dialog_row(ui, label, 160.0, 160.0, |ui| {
        let items = [Segment::icon(Icon::TypeDialogNone, tr("Off")), Segment::icon(Icon::Check, tr("On"))];
        let result =
            ui.add_enabled_ui(supported, |ui| SegmentedControl::new(&items, value.map(usize::from)).width(ControlWidth::Fixed(48.0)).show(ui));
        result.response.on_disabled_hover_text(tr("Not in this font"));
        result.inner.map(|index| index == 1)
    })
}

fn details_feature(
    app: &mut FigApp,
    ui: &mut Ui,
    doc: &Document,
    ids: &[NodeId],
    face: Option<&figcraft_text::Face>,
    tag: &str,
    label: &str,
    default_on: bool,
) {
    let current = common(doc, ids, |node| node.kind.text().map(|text| text.features.get(tag).copied())).flatten();
    let shown = current.map(|override_value| override_value.unwrap_or(default_on));
    // `has_feature` lists GSUB tags only; spacing features can live in GPOS.
    let supported =
        matches!(tag, "kern" | "cpsp") || face.is_none_or(|face| <[u8; 4]>::try_from(tag.as_bytes()).ok().is_some_and(|tag| face.has_feature(&tag)));
    if let Some(enabled) = details_toggle(ui, label, shown, supported) {
        let value = if enabled == default_on { Value::Null } else { json!(enabled) };
        app.run("text.setFeatures", json!({"ids": ids, "features": {tag: value}}));
    }
}

/// A Type settings row: a label, then `add` with the rest of the width.
fn field_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.add_sized(vec2(110.0, FIELD_H), egui::Label::new(egui::RichText::new(label).font(regular(11.0)).color(t.text)));
        add(ui, w - 118.0)
    })
    .inner
}

/// How the decoration looks: style, thickness, (underlines) offset and skip ink, and colour.
fn decoration_details(app: &mut FigApp, ui: &mut Ui, styles: &[TextStyle], apply_style: &dyn Fn(&mut FigApp, &str, Value), underline: bool, w: f32) {
    caption(ui, tr("Decoration style"));
    let active = uniform(styles, |s| s.look().style as usize);
    let items = [(tr("Solid"), tr_noop("Solid")), (tr("Dotted"), tr_noop("Dotted")), (tr("Wavy"), tr_noop("Wavy"))];
    if let Some(i) = text_seg(ui, "deco_style", &items, active, w) {
        apply_style(app, "textDecorationStyle", json!((["solid", "dotted", "wavy"].get(i).copied().unwrap_or("solid"))));
    }
    ui.add_space(4.0);
    let mut lengths = vec![("textDecorationThickness", tr("Thickness"), uniform(styles, |s| s.look().thickness))];
    if underline {
        lengths.push(("textDecorationOffset", tr("Offset"), uniform(styles, |s| s.look().offset)));
    }
    for (key, label, value) in lengths {
        field_row(ui, label, |ui, fw| {
            let (shown, empty) = match value {
                Some(DecorationLength::Pixels(v)) => (Some(v), String::new()),
                Some(DecorationLength::Percent(p)) => (None, format!("{}%", trim_num(p))),
                Some(DecorationLength::Auto) => (None, tr("Auto").to_string()),
                None => (None, tr(MIXED).to_string()),
            };
            let auto = value == Some(DecorationLength::Auto);
            let field_w = if auto { fw } else { fw - 36.0 };
            if let Some(FieldEdit::Set(v) | FieldEdit::Scrub(v, true)) =
                widgets::number_field_or(ui, Id::new(("ty_deco", key)), shown, &empty, field_w, 0.5)
            {
                // A thickness of 0 or less goes back to the font's.
                let v = if key == "textDecorationThickness" && v <= 0.0 { json!("auto") } else { json!(v.clamp(-1000.0, 1000.0)) };
                apply_style(app, key, v);
            }
            let tip = tr("Back to the font's value");
            if !auto
                && ui
                    .add(
                        egui::Label::new(egui::RichText::new(tr("Auto")).font(regular(11.0)).color(Tokens::get(ui.ctx()).accent_text))
                            .sense(Sense::click()),
                    )
                    .on_hover_text(tip)
                    .clicked()
            {
                apply_style(app, key, json!("auto"));
            }
        });
    }
    if underline {
        let skip = uniform(styles, |s| s.look().skip_ink);
        if widgets::checkbox(ui, skip == Some(true), tr("Skip ink")) {
            apply_style(app, "textDecorationSkipInk", json!(skip != Some(true)));
        }
    }
    let color = uniform(styles, |s| s.look().color.map(|c| format!("#{}", c.to_hex())));
    field_row(ui, tr("Colour"), |ui, fw| {
        let (value, hint) = match &color {
            Some(c) => (c.clone().unwrap_or_default(), tr("Auto")),
            None => (String::new(), tr(MIXED)),
        };
        if let Some(s) = widgets::text_field(ui, Id::new("ty_deco_color"), &value, fw, hint) {
            let s = s.trim();
            if s.is_empty() || s.eq_ignore_ascii_case("auto") {
                apply_style(app, "textDecorationColor", json!("auto"));
            } else if let Some(c) = figcraft_color::Color::parse_css(s).or_else(|| figcraft_color::Color::from_hex(s)) {
                apply_style(app, "textDecorationColor", json!(format!("#{}", c.to_hex())));
            } else {
                app.toast(trf("{value} isn't a colour", &[("value", &s)]));
            }
        }
    });
}

/// Type settings ▸ Variable: the font's named instances, then a slider and a field per visible
/// axis (values the style name implies show until changed; "Mixed" when the shown styles differ).
/// The canvas previews a hovered instance and a dragged slider or field live; letting go commits
/// one undo step, Escape cancels.
fn variable_axes(app: &mut FigApp, ui: &mut Ui, ids: &[NodeId], styles: &[TextStyle], face: &figcraft_text::Face) {
    let t = Tokens::get(ui.ctx());
    let range = figcraft_engine::textedit::selected_range(&app.session);
    widgets::label(ui, &face.family, t.text_secondary);
    named_instances(app, ui, ids, range, styles, face);
    // Each style's axis values; a style drawn by another face (e.g. a separate italic file) is mixed.
    let per_style: Vec<Option<Vec<f64>>> = styles
        .iter()
        .map(|s| figcraft_text::find_face(&s.font_family, &s.font_style).filter(|f| f.id == face.id).map(|_| face.axis_values(s)))
        .collect();
    let w = ui.available_width();
    let field_w = 52.0;
    let label_w = 84.0;
    let slider_w = (w - label_w - field_w - 12.0).max(40.0);
    let cancel_id = Id::new("ty_axis_cancelled");
    // A cancelled gesture ends when the pointer is let go.
    if !ui.input(|i| i.pointer.any_down()) {
        ui.data_mut(|d| d.remove::<bool>(cancel_id));
    }
    for (i, axis) in face.axes.iter().enumerate().filter(|(_, a)| !a.hidden) {
        let first = per_style.first().cloned().flatten().and_then(|v| v.get(i).copied());
        let value = first.filter(|v0| per_style.iter().all(|vs| vs.as_ref().and_then(|v| v.get(i)).is_some_and(|v| (v - v0).abs() < 1e-9)));
        // While a drag previews, the controls show the dragged value (the panel shows the document).
        let live_id = Id::new(("ty_axis_live", &axis.tag));
        if app.ui.text_preview.is_none() {
            ui.data_mut(|d| d.remove::<f64>(live_id));
        }
        let shown = ui.data(|d| d.get_temp::<f64>(live_id)).or(value).map(|v| round_axis(axis, v));
        // (value, gesture ended)
        let mut set: Option<(Value, bool)> = None;
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (lr, _) = ui.allocate_exact_size(vec2(label_w, FIELD_H), Sense::hover());
            let name = ui.painter().layout(axis.name.clone(), regular(11.0), t.text, label_w);
            let tip = trf(
                "{name} ({tag}): {min} – {max}, default {default}. Double-click the slider to go back to the style's value; Escape while dragging cancels.",
                &[
                    ("name", &axis.name),
                    ("tag", &axis.tag),
                    ("min", &trim_num(axis.min)),
                    ("max", &trim_num(axis.max)),
                    ("default", &trim_num(axis.default)),
                ],
            );
            ui.interact(lr, Id::new(("ty_axis_label", &axis.tag)), Sense::hover()).on_hover_text(tip);
            ui.painter().galley(pos2(lr.min.x, lr.center().y - name.size().y / 2.0), name, t.text);
            match axis_slider(ui, Id::new(("ty_axis_slider", &axis.tag)), axis, shown, slider_w) {
                Some((Some(v), done)) => set = Some((json!(round_axis(axis, v)), done)),
                Some((None, _)) => set = Some((Value::Null, true)),
                None => {}
            }
            let step = if axis.max - axis.min > 20.0 { 1.0 } else { 0.1 };
            match widgets::number_field(ui, Id::new(("ty_axis_field", &axis.tag)), Lead::None, shown, "", field_w, step) {
                Some(FieldEdit::Set(v)) => set = Some((json!(axis.clamp(round_axis(axis, v))), true)),
                Some(FieldEdit::Scrub(v, done)) => set = Some((json!(axis.clamp(round_axis(axis, v))), done)),
                None => {}
            }
        });
        let Some((v, done)) = set else { continue };
        // Escape cancels a previewing gesture (egui may end the drag on it too): the preview ends
        // and the rest of the gesture is ignored until the pointer is let go.
        let previewing = ui.data(|d| d.get_temp::<f64>(live_id)).is_some();
        if previewing && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            ui.data_mut(|d| {
                d.insert_temp(cancel_id, true);
                d.remove::<f64>(live_id);
            });
            app.end_text_preview(ui.ctx());
            continue;
        }
        if ui.data(|d| d.get_temp::<bool>(cancel_id)).unwrap_or(false) {
            continue;
        }
        let params = json!({"ids": ids, "variations": {axis.tag.clone(): v}});
        if done {
            ui.data_mut(|d| d.remove::<f64>(live_id));
            app.run("text.setVariations", params);
        } else {
            if let Some(x) = v.as_f64() {
                ui.data_mut(|d| d.insert_temp(live_id, x));
            }
            app.preview_text(ui.ctx(), "text.setVariations", params);
        }
    }
    if styles.iter().any(|s| !s.variations.is_empty()) {
        ui.add_space(8.0);
        let r = ui.add(egui::Label::new(egui::RichText::new(tr("Reset to style")).font(regular(11.0)).color(t.accent_text)).sense(Sense::click()));
        if r.on_hover_text(tr("Back to the values the font style implies")).clicked() {
            app.run("text.setVariations", json!({"ids": ids, "variations": {}, "replace": true}));
        }
    }
}

/// The variable font's named instances as a menu: the one the text shows ("Custom" once an axis
/// moved), hovering one previews it on the canvas, clicking applies it (its custom axis values go).
fn named_instances(
    app: &mut FigApp,
    ui: &mut Ui,
    ids: &[NodeId],
    range: Option<(NodeId, usize, usize)>,
    styles: &[TextStyle],
    face: &figcraft_text::Face,
) {
    if face.instances.is_empty() {
        return;
    }
    let current = uniform(styles, |s| (s.variations.is_empty(), s.font_style.clone()));
    let label = match &current {
        Some((true, st)) if face.instance(st).is_some() => st.clone(),
        Some(_) => tr("Custom").to_string(),
        None => tr(MIXED).to_string(),
    };
    ui.add_space(4.0);
    // An inline list (a menu would close the Type settings popover it opens from).
    let open_id = Id::new("ty_instances_open");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    if crate::primitives::Select::trigger(ui, &label, ui.available_width(), None).on_hover_text(tr("Named instance")).clicked() {
        open = !open;
    }
    if open {
        ui.add_space(2.0);
        egui::ScrollArea::vertical().id_salt("ty_instances").max_height(220.0).show(ui, |ui| {
            for inst in &face.instances {
                let mark = if label == inst.name { "✓ " } else { "   " };
                let (clicked, hovered) = menu_item_hover(ui, &format!("{mark}{}", inst.name));
                let props = json!({"fontStyle": inst.name, "fontVariations": null});
                if clicked {
                    commit_text(app, ids, range, props);
                    open = false;
                } else if hovered {
                    app.preview_text(ui.ctx(), "text.style", text_style_params(ids, range, props));
                }
            }
        });
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
    ui.add_space(2.0);
}

/// Axis values snap to whole numbers on wide axes (weight, width…) and tenths on narrow ones.
fn round_axis(axis: &figcraft_text::Axis, v: f64) -> f64 {
    if axis.max - axis.min > 20.0 { v.round() } else { (v * 10.0).round() / 10.0 }
}

/// A horizontal slider over an axis's range with a tick at its default. `value` None = mixed.
/// Returns the new value (None = double-click: back to the style's value) and whether the
/// gesture ended.
fn axis_slider(ui: &mut Ui, id: Id, axis: &figcraft_text::Axis, value: Option<f64>, width: f32) -> Option<(Option<f64>, bool)> {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, FIELD_H), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(width - 12.0, 4.0));
    let span = (axis.max - axis.min).max(1e-9);
    let x_of = |v: f64| track.min.x + ((v - axis.min) / span).clamp(0.0, 1.0) as f32 * track.width();
    let p = ui.painter();
    p.rect_filled(track, CornerRadius::same(2), t.border);
    let dx = x_of(axis.default);
    p.line_segment([pos2(dx, track.min.y - 3.0), pos2(dx, track.max.y + 3.0)], Stroke::new(1.0, t.border_strong));
    if let Some(v) = value {
        let x = x_of(v);
        p.rect_filled(Rect::from_min_max(track.min, pos2(x, track.max.y)), CornerRadius::same(2), t.accent);
        p.circle_filled(pos2(x, track.center().y), 6.0, Color32::WHITE);
        let ring = if resp.hovered() || resp.dragged() { crate::theme::ACCENT } else { t.border_strong };
        p.circle_stroke(pos2(x, track.center().y), 6.0, Stroke::new(1.0, ring));
    }
    if resp.double_clicked() {
        return Some((None, true));
    }
    if (resp.dragged() || resp.clicked() || resp.drag_started())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let f = f64::from(((pos.x - track.min.x) / track.width().max(1.0)).clamp(0.0, 1.0));
        return Some((Some(axis.min + f * span), resp.drag_stopped() || resp.clicked()));
    }
    if resp.drag_stopped() {
        return value.map(|v| (Some(v), true));
    }
    None
}

fn caption(ui: &mut Ui, s: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    ui.label(egui::RichText::new(s).font(regular(10.5)).color(t.text_secondary));
}

/// A segmented control with short text labels (decoration, case, list…); `tip`s are English keys
/// (matched below), translated where shown. Returns the clicked index.
fn text_seg(ui: &mut Ui, salt: &str, items: &[(&str, &str)], active: Option<usize>, width: f32) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, FIELD_H), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
    let cell = width / items.len().max(1) as f32;
    let mut out = None;
    for (i, (label, tip)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.min.x + i as f32 * cell, rect.min.y), vec2(cell, FIELD_H));
        let resp = ui.interact(r, Id::new(("ty_seg", salt, i)), Sense::click()).on_hover_text(tr(tip));
        crate::a11y::toggle(&resp, tr(tip), active == Some(i));
        if active == Some(i) {
            ui.painter().rect_filled(r.shrink(2.0), CornerRadius::same(4), t.panel);
            ui.painter().rect_stroke(r.shrink(2.0), CornerRadius::same(4), Stroke::new(1.0, t.border), StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(r.shrink(2.0), CornerRadius::same(4), t.hover);
        }
        let col = if active == Some(i) { t.text } else { t.text_secondary };
        let galley = if *tip == "Small caps" {
            small_caps_sample(ui, label, col)
        } else {
            ui.painter().layout_no_wrap((*label).to_string(), regular(11.5), col)
        };
        let gr = Rect::from_center_size(r.center(), galley.size());
        let size = galley.size();
        ui.painter().galley(gr.min, galley, col);
        // Draw the decoration on its own sample letter.
        match *tip {
            "Underline" => {
                let y = gr.max.y - 1.5;
                ui.painter().line_segment([pos2(gr.min.x, y), pos2(gr.min.x + size.x, y)], Stroke::new(1.0, col));
            }
            "Strikethrough" => {
                let y = gr.center().y + 0.5;
                ui.painter().line_segment([pos2(gr.min.x - 1.0, y), pos2(gr.min.x + size.x + 1.0, y)], Stroke::new(1.0, col));
            }
            _ => {}
        }
        if resp.clicked() {
            out = Some(i);
        }
    }
    out
}

/// The Small caps sample: a capital, then its other letters as smaller capitals. Drawn from
/// ordinary capitals because the small-capital letters (`ɢ`, U+0262) are missing from many UI
/// fonts, egui's defaults among them, where they showed as a box.
fn small_caps_sample(ui: &Ui, label: &str, col: Color32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    let mut chars = label.chars();
    let first: String = chars.next().into_iter().collect();
    let rest: String = chars.map(small_capital_to_capital).collect::<String>().to_uppercase();
    job.append(&first, 0.0, egui::TextFormat { font_id: regular(11.5), color: col, valign: egui::Align::BOTTOM, ..Default::default() });
    job.append(&rest, 0.0, egui::TextFormat { font_id: regular(8.5), color: col, valign: egui::Align::BOTTOM, ..Default::default() });
    ui.painter().layout_job(job)
}

/// The capital a small-capital letter stands for (the ones a sample uses), else the letter.
fn small_capital_to_capital(c: char) -> char {
    match c {
        'ɢ' => 'G',
        'ʜ' => 'H',
        'ɪ' => 'I',
        'ʟ' => 'L',
        'ɴ' => 'N',
        'ʀ' => 'R',
        'ʏ' => 'Y',
        c => c,
    }
}

#[derive(Clone, Copy)]
enum LeadGlyph {
    LineHeight,
    LetterSpacing,
}

/// Paint a small original glyph for a typography field's lead.
fn paint_lead(p: &egui::Painter, r: Rect, g: LeadGlyph, col: Color32) {
    match g {
        LeadGlyph::LineHeight => icons::paint(p, r, Icon::TypeLineHeight, col),
        LeadGlyph::LetterSpacing => icons::paint(p, r, Icon::TypeLetterSpacing, col),
    }
}

/// Line height, letter spacing or paragraph spacing: property, field id, glyph and tooltip.
struct SpacingField {
    field: &'static str,
    id: Id,
    glyph: LeadGlyph,
    tip: &'static str,
}

enum SpacingEdit {
    Typed(String),
    Scrub(f64, bool),
}

impl SpacingEdit {
    fn text(&self) -> String {
        match self {
            Self::Typed(text) => text.clone(),
            Self::Scrub(value, _) => trim_num(*value),
        }
    }
}

/// Keep typed unit parsing and character-range targeting; only a scrub outside a text-editing
/// session needs its own interaction. The editing session already owns its undo step.
fn commit_spacing(app: &mut FigApp, ids: &[NodeId], range: Option<(NodeId, usize, usize)>, field: &SpacingField, edit: &SpacingEdit, value: Value) {
    let props = json!({field.field: value});
    match edit {
        SpacingEdit::Scrub(_, done) if figcraft_engine::textedit::selected_range(&app.session).is_none() => {
            live(app, field.tip, "node.set", json!({"ids": ids, "props": props}), *done);
        }
        _ => commit_text(app, ids, range, props),
    }
}

/// The original text editor plus the same leading-icon scrub gesture as ordinary number fields.
/// The optional start and step let line height scrub in percent even when displayed as px or Auto.
fn spacing_input(ui: &mut Ui, field: &SpacingField, value: &str, width: f32, scrub: Option<(f64, f64)>) -> Option<SpacingEdit> {
    let rect = Rect::from_min_size(ui.cursor().min, vec2(24.0, FIELD_H));
    let typed = lead_field(ui, field.id, field.glyph, value, width, field.tip);
    if ui.data(|d| d.get_temp::<String>(field.id.with("editing")).is_some()) {
        return typed.map(SpacingEdit::Typed);
    }
    let response = ui.interact(rect, field.id.with("lead"), Sense::drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let (numeric, step) = scrub.map_or_else(|| (value.trim_end_matches('%').parse::<f64>().ok(), 1.0), |(start, step)| (Some(start), step));
    match widgets::scrub(ui, field.id, &response, numeric, step) {
        Some(FieldEdit::Scrub(v, done)) => Some(SpacingEdit::Scrub(v, done)),
        _ => typed.map(SpacingEdit::Typed),
    }
}

/// The same typography fields used by the properties panel, exposed to text-style editors.
pub(crate) enum StyleSpacingEdit {
    Typed(Value),
    Scrub(Value, bool),
}

impl StyleSpacingEdit {
    pub(crate) fn into_value_and_scrub(self) -> (Value, Option<bool>) {
        match self {
            Self::Typed(value) => (value, None),
            Self::Scrub(value, done) => (value, Some(done)),
        }
    }
}

pub(crate) fn style_line_height_field(ui: &mut Ui, id: Id, style: &TextStyle, width: f32) -> Option<StyleSpacingEdit> {
    let field = SpacingField { field: "lineHeight", id, glyph: LeadGlyph::LineHeight, tip: tr("Line height") };
    let scrub = line_height_scrub_start(Some(style.line_height), Some(style)).map(|start| (start, 0.1));
    match spacing_input(ui, &field, &style.line_height.label(), width, scrub)? {
        SpacingEdit::Typed(value) => parse_line_height(&value, Some(style.line_height)).map(StyleSpacingEdit::Typed),
        SpacingEdit::Scrub(value, done) => Some(StyleSpacingEdit::Scrub(json!(format!("{:.1}%", (value.max(0.0) * 10.0).round() / 10.0)), done)),
    }
}

pub(crate) fn style_letter_spacing_field(ui: &mut Ui, id: Id, style: &TextStyle, width: f32) -> Option<StyleSpacingEdit> {
    let field = SpacingField { field: "letterSpacing", id, glyph: LeadGlyph::LetterSpacing, tip: tr("Letter spacing") };
    let edit = spacing_input(ui, &field, &style.letter_spacing.label(), width, None)?;
    let value = parse_letter_spacing(&edit.text(), Some(style.letter_spacing))?;
    match edit {
        SpacingEdit::Typed(_) => Some(StyleSpacingEdit::Typed(value)),
        SpacingEdit::Scrub(_, done) => Some(StyleSpacingEdit::Scrub(value, done)),
    }
}

/// A spacing field that whole layers can bind to a number variable (in pixels): the variable's
/// chip when bound, else the editable field with a variable button on hover. While a text range is
/// selected it only takes values. Returns a typed commit or a live scrub.
fn spacing_field(
    app: &mut FigApp,
    ui: &mut Ui,
    ids: &[NodeId],
    f: &SpacingField,
    value: &str,
    width: f32,
    range: bool,
    scrub: Option<(f64, f64)>,
) -> Option<SpacingEdit> {
    let rect = Rect::from_min_size(ui.cursor().min, vec2(width, FIELD_H));
    if !range && crate::variable_ui::bound_chip(app, ui, ids, f.field, VariableType::Number, rect) {
        ui.allocate_rect(rect, Sense::hover());
        return None;
    }
    let out = spacing_input(ui, f, value, width, scrub);
    if !range && ui.data(|d| d.get_temp::<String>(f.id.with("editing")).is_none()) {
        crate::variable_ui::bind_button(ui, rect, ids, f.field, VariableType::Number);
    }
    out
}

fn lead_field(ui: &mut Ui, id: Id, glyph: LeadGlyph, value: &str, width: f32, tip: &str) -> Option<String> {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, FIELD_H), Sense::hover());
    let lead = Rect::from_min_size(rect.min, vec2(24.0, FIELD_H));
    let text_rect = Rect::from_min_max(pos2(lead.max.x, rect.min.y), rect.max);
    let editing_id = id.with("editing");
    let mut editing: Option<String> = ui.data(|d| d.get_temp(editing_id));
    let resp = ui.interact(rect, id.with("click"), Sense::click()).on_hover_text(tip);
    crate::a11y::text(&resp, tip, value);
    ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
    if editing.is_some() {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, crate::theme::ACCENT), StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.field_hover_border), StrokeKind::Inside);
    }
    paint_lead(ui.painter(), lead, glyph, t.icon_secondary);
    let started = editing.is_none() && resp.clicked();
    if started {
        editing = Some(if value == tr(MIXED) { String::new() } else { value.to_string() });
        ui.memory_mut(|m| m.request_focus(id.with("te")));
    }
    let mut out = None;
    match editing {
        Some(mut text) => {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect.shrink2(vec2(2.0, 4.0))));
            let r = child.add(
                egui::TextEdit::singleline(&mut text)
                    .id(id.with("te"))
                    .frame(egui::Frame::NONE)
                    .font(regular(11.0))
                    .desired_width(text_rect.width() - 6.0),
            );
            crate::a11y::own_focus(&r);
            crate::widgets::select_all_on_focus(ui, &r, &text);
            // The Enter that started the edit (from the keyboard) doesn't end it.
            let enter = !started && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if esc {
                ui.data_mut(|d| d.remove::<String>(editing_id));
            } else if r.lost_focus() || enter {
                out = Some(text);
                ui.data_mut(|d| d.remove::<String>(editing_id));
            } else {
                ui.data_mut(|d| d.insert_temp(editing_id, text));
            }
        }
        None => {
            let col = if value == tr(MIXED) { t.text_tertiary } else { t.text };
            ui.painter().text(pos2(text_rect.min.x + 2.0, rect.center().y), Align2::LEFT_CENTER, value, regular(11.0), col);
        }
    }
    out
}

/// The percent equivalent of the current line height for an icon drag. Auto uses the same font
/// metrics as text layout, falling back to 120% when the face is unavailable.
fn line_height_scrub_start(line_height: Option<LineHeight>, style: Option<&TextStyle>) -> Option<f64> {
    let line_height = line_height?;
    let style = style?;
    match line_height {
        LineHeight::Percent(percent) => Some(percent),
        LineHeight::Pixels(px) => (style.font_size.is_finite() && style.font_size > 0.0).then_some(px / style.font_size * 100.0),
        LineHeight::Auto => {
            let ratio = figcraft_text::find_face(&style.font_family, &style.font_style)
                .map(|face| ((face.ascent + face.descent + face.line_gap.max(0.0)) / face.upem).max(0.5))
                .unwrap_or(1.2);
            Some(ratio * 100.0)
        }
    }
}

/// "Auto", "24", "150%" → the `lineHeight` value. A bare number means pixels regardless of the
/// current unit. Empty text means Auto, except on a Mixed field (`cur` is `None`), where nothing
/// typed is an untouched field and commits nothing.
pub(crate) fn parse_line_height(s: &str, cur: Option<LineHeight>) -> Option<Value> {
    let s = s.trim();
    if s.is_empty() && cur.is_none() {
        return None;
    }
    if s.is_empty() || s.eq_ignore_ascii_case("auto") {
        return Some(json!("auto"));
    }
    if let Some(p) = s.strip_suffix('%') {
        let cur_pct = match cur {
            Some(LineHeight::Percent(p)) => Some(p),
            _ => None,
        };
        return widgets::eval(p.trim(), cur_pct).filter(|v| v.is_finite()).map(|v| json!(format!("{}%", trim_num(v.max(0.0)))));
    }
    if let Some(px) = s.strip_suffix("px") {
        return widgets::eval(px.trim(), None).filter(|v| v.is_finite()).map(|v| json!(v.max(0.0)));
    }
    let current_px = match cur {
        Some(LineHeight::Pixels(px)) => Some(px),
        _ => None,
    };
    widgets::eval(s, current_px).filter(|v| v.is_finite()).map(|v| json!(v.max(0.0)))
}

/// "2%", "1.5px", "-0.5" → the `letterSpacing` value. A bare number keeps the current unit
/// (percent by default).
pub(crate) fn parse_letter_spacing(s: &str, cur: Option<LetterSpacing>) -> Option<Value> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%') {
        return widgets::eval(p.trim(), None).filter(|v| v.is_finite()).map(|v| json!(format!("{}%", trim_num(v))));
    }
    if let Some(p) = s.strip_suffix("px") {
        return widgets::eval(p.trim(), None).filter(|v| v.is_finite()).map(|v| json!(v));
    }
    match cur {
        Some(LetterSpacing::Pixels(px)) => widgets::eval(s, Some(px)).filter(|v| v.is_finite()).map(|v| json!(v)),
        Some(LetterSpacing::Percent(p)) => widgets::eval(s, Some(p)).filter(|v| v.is_finite()).map(|v| json!(format!("{}%", trim_num(v)))),
        None => widgets::eval(s, None).filter(|v| v.is_finite()).map(|v| json!(format!("{}%", trim_num(v)))),
    }
}

/// A [`parse_line_height`] value as a [`LineHeight`]: "auto", "150%" or a number of pixels.
pub(crate) fn line_height_of(v: &Value) -> Option<LineHeight> {
    match v {
        Value::String(s) if s.eq_ignore_ascii_case("auto") => Some(LineHeight::Auto),
        Value::String(s) => s.strip_suffix('%')?.trim().parse().ok().filter(|p: &f64| p.is_finite()).map(LineHeight::Percent),
        other => other.as_f64().filter(|v| v.is_finite()).map(LineHeight::Pixels),
    }
}

/// A [`parse_letter_spacing`] value as a [`LetterSpacing`]: "2%" or a number of pixels.
pub(crate) fn letter_spacing_of(v: &Value) -> Option<LetterSpacing> {
    match v {
        Value::String(s) => s.strip_suffix('%')?.trim().parse().ok().filter(|p: &f64| p.is_finite()).map(LetterSpacing::Percent),
        other => other.as_f64().filter(|v| v.is_finite()).map(LetterSpacing::Pixels),
    }
}

// ---- Font picker ----

const RECENT_KEY: &str = "figcraft_recent_fonts";
const ROW: f32 = 26.0;
/// The font list's tallest (it scrolls past that).
const LIST_HEIGHT: f32 = 380.0;

fn recent_families(ctx: &egui::Context) -> Vec<String> {
    ctx.data_mut(|d| d.get_persisted::<Vec<String>>(Id::new(RECENT_KEY))).unwrap_or_default()
}

pub(crate) fn remember_family(ctx: &egui::Context, family: &str) {
    ctx.data_mut(|d| {
        let v = d.get_persisted_mut_or_default::<Vec<String>>(Id::new(RECENT_KEY));
        v.retain(|f| !f.eq_ignore_ascii_case(family));
        v.insert(0, family.to_string());
        v.truncate(8);
    });
}

enum PickRow {
    Header(&'static str),
    Family(String),
}

/// What the family list did this frame.
#[derive(Default)]
pub(crate) struct FontPick {
    /// The family clicked, or the one Enter picked.
    pub picked: Option<String>,
    /// The family Up and Down are on (Enter picks it).
    pub highlighted: Option<String>,
    /// The family the pointer is over (the caller previews it on the canvas).
    pub hovered: Option<String>,
}

/// The searchable family list: click a family, or step through it with Up and Down and press Enter.
/// Hovering a family previews it on the canvas (closing without picking restores the original).
pub(crate) fn font_picker(ui: &mut Ui, current: Option<&str>, just_opened: bool) -> FontPick {
    font_picker_impl(ui, current, just_opened, false)
}

fn font_panel_picker(ui: &mut Ui, current: Option<&str>, just_opened: bool) -> FontPick {
    font_picker_impl(ui, current, just_opened, true)
}

/// Shared floating font chooser used by the Typography section and text-style editor.
pub(crate) fn font_family_panel(ctx: &egui::Context, id: Id, initial: egui::Pos2, current: Option<&str>, just_opened: bool) -> (FontPick, bool) {
    let mut close = false;
    let mut picked = FontPick::default();
    crate::primitives::FloatingPanel::new(id, initial).width(crate::primitives::FloatingPanelWidth::Compact).show_with_header(ctx, |ui, header| {
        close = crate::primitives::FloatingPanel::title_header(ui, header, tr("Fonts"), id.with("close"));
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
            picked = font_panel_picker(ui, current, just_opened);
        });
    });
    (picked, close)
}

fn font_picker_impl(ui: &mut Ui, current: Option<&str>, just_opened: bool, panel: bool) -> FontPick {
    let t = Tokens::get(ui.ctx());
    let q_id = Id::new("font_query");
    // The row Up and Down are on, and the search it is a row of.
    let hi_id = Id::new("font_highlight");
    let active_id = Id::new("font_query_active");
    let filter_id = Id::new("font_filter");
    let mut q: String = ui.data(|d| d.get_temp(q_id)).unwrap_or_default();
    if just_opened {
        q = if panel { current.unwrap_or_default().to_string() } else { String::new() };
        ui.data_mut(|d| d.remove::<(String, usize)>(hi_id));
        ui.data_mut(|d| d.insert_temp(active_id, !panel));
        if panel {
            ui.data_mut(|d| d.insert_temp(filter_id, 0_usize));
        }
    }
    // Taken before the search field sees them (it would move its cursor).
    let step = ui.input_mut(|i| {
        i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) as i64
            - i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) as i64
    });
    let search = if panel {
        crate::primitives::SearchField::new(q_id.with("input"), tr("Search fonts")).clearable().show(ui, &mut q)
    } else {
        ui.add(egui::TextEdit::singleline(&mut q).hint_text(tr("Search fonts")).desired_width(ui.available_width()).font(regular(11.5)))
    };
    crate::widgets::select_all_on_focus(ui, &search, &q);
    if just_opened {
        search.request_focus();
    }
    if search.changed() {
        ui.data_mut(|d| d.insert_temp(active_id, true));
    }
    ui.data_mut(|d| d.insert_temp(q_id, q.clone()));
    let filter = if panel {
        ui.add_space(8.0);
        let options = [tr("All fonts"), tr("Recent")];
        let selected = ui.data(|d| d.get_temp::<usize>(filter_id)).unwrap_or(0);
        if let Some(next) = crate::primitives::Select::new(&options, selected)
            .width(crate::primitives::ControlWidth::Fill)
            .variant(crate::primitives::SelectVariant::Outlined)
            .show(ui)
        {
            ui.data_mut(|d| d.insert_temp(filter_id, next));
        }
        ui.add_space(8.0);
        ui.data(|d| d.get_temp::<usize>(filter_id)).unwrap_or(0)
    } else {
        0
    };
    let query_active = ui.data(|d| d.get_temp::<bool>(active_id)).unwrap_or(true);
    let ql = if query_active { q.trim().to_lowercase() } else { String::new() };
    let all = figcraft_text::family_names();
    let recent = recent_families(ui.ctx());
    let matches: Vec<String> = all
        .iter()
        .filter(|f| (filter == 0 || recent.iter().any(|r| r.eq_ignore_ascii_case(f))) && (ql.is_empty() || f.to_lowercase().contains(&ql)))
        .cloned()
        .collect();
    let mut rows: Vec<PickRow> = Vec::new();
    if ql.is_empty() && !panel {
        let recent: Vec<String> = recent.into_iter().filter(|f| all.iter().any(|a| a.eq_ignore_ascii_case(f))).collect();
        if !recent.is_empty() {
            rows.push(PickRow::Header(tr("Recent")));
            rows.extend(recent.into_iter().map(PickRow::Family));
            rows.push(PickRow::Header(tr("All fonts")));
        }
    }
    if matches.is_empty() {
        widgets::label(ui, tr("No fonts match"), t.text_tertiary);
        return FontPick::default();
    }
    rows.extend(matches.into_iter().map(PickRow::Family));
    let family = |i: usize| match rows.get(i) {
        Some(PickRow::Family(f)) => Some(f.clone()),
        _ => None,
    };
    // A new search starts again; the first step starts from the current family.
    let mut hi = ui.data(|d| d.get_temp::<(String, usize)>(hi_id)).filter(|(hq, _)| *hq == q).map(|(_, i)| i);
    let stride = ROW + ui.spacing().item_spacing.y;
    let mut offset = None;
    if step != 0 {
        let next = |i: usize| if step > 0 { i.checked_add(1) } else { i.checked_sub(1) };
        let mut from =
            hi.or_else(|| rows.iter().rposition(|r| matches!(r, PickRow::Family(f) if current.is_some_and(|c| c.eq_ignore_ascii_case(f)))));
        for _ in 0..step.unsigned_abs() {
            let mut at = match from {
                Some(i) => next(i),
                None if step > 0 => Some(0),
                None => rows.len().checked_sub(1),
            };
            // Past the headers; at either end it stays where it was.
            while let Some(i) = at.filter(|i| *i < rows.len() && family(*i).is_none()) {
                at = next(i);
            }
            if let Some(i) = at.filter(|i| family(*i).is_some()) {
                from = Some(i);
                hi = Some(i);
            }
        }
        if let Some(i) = hi {
            // Scrolled just enough to show it (the list is shorter than its tallest where the
            // window has less room).
            let (seen, height): (f32, f32) = ui.data(|d| d.get_temp(hi_id.with("view"))).unwrap_or((0.0, LIST_HEIGHT));
            let top = i as f32 * stride;
            offset = Some(seen.max(top + stride - height).min(top));
        }
    }
    match hi {
        Some(i) => ui.data_mut(|d| {
            d.insert_temp(hi_id, (q.clone(), i));
        }),
        None => ui.data_mut(|d| d.remove::<(String, usize)>(hi_id)),
    }
    if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let first = rows.iter().find_map(|r| match r {
            PickRow::Family(f) => Some(f.clone()),
            PickRow::Header(_) => None,
        });
        return FontPick { picked: hi.and_then(family).or(first), highlighted: None, hovered: None };
    }
    let mut budget = 4;
    let mut picked = None;
    let mut hovered_family = None;
    let mut area = egui::ScrollArea::vertical().max_height(LIST_HEIGHT).auto_shrink([false, true]);
    if let Some(y) = offset {
        area = area.vertical_scroll_offset(y);
    }
    let shown = area.show_rows(ui, ROW, rows.len(), |ui, range| {
        for (i, r) in rows.iter().enumerate().skip(range.start).take(range.len()) {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW), if matches!(r, PickRow::Family(_)) { Sense::click() } else { Sense::hover() });
            match r {
                PickRow::Header(h) => {
                    ui.painter().text(pos2(rect.min.x + 8.0, rect.center().y), Align2::LEFT_CENTER, *h, semibold(10.5), t.text_secondary);
                }
                PickRow::Family(f) => {
                    let hovered = resp.hovered() || hi == Some(i);
                    if hovered {
                        ui.painter().rect_filled(rect, CornerRadius::same(5), t.accent);
                    }
                    let col = if hovered { t.accent_on } else { t.text };
                    if current.is_some_and(|c| c.eq_ignore_ascii_case(f)) {
                        icons::paint(
                            ui.painter(),
                            Rect::from_center_size(pos2(rect.min.x + 10.0, rect.center().y), vec2(12.0, 12.0)),
                            Icon::Check,
                            col,
                        );
                    }
                    let text_min = pos2(rect.min.x + 22.0, rect.min.y);
                    match crate::font_preview::family(ui.ctx(), f, 13.0, &mut budget) {
                        Some(tex) => {
                            crate::font_preview::paint(
                                ui.painter(),
                                &tex,
                                pos2(text_min.x, rect.center().y),
                                rect.max.x - text_min.x - 6.0,
                                ROW,
                                col,
                            );
                        }
                        None => {
                            ui.painter().text(pos2(text_min.x, rect.center().y), Align2::LEFT_CENTER, f, regular(12.0), col);
                        }
                    }
                    if resp.clicked() {
                        picked = Some(f.clone());
                    }
                    if resp.hovered() {
                        hovered_family = Some(f.clone());
                    }
                }
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(hi_id.with("view"), (shown.state.offset.y, shown.inner_rect.height())));
    FontPick { highlighted: hi.and_then(family), picked, hovered: hovered_family }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_caps_sample_draws_only_letters_the_ui_font_has() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx, false, 0);
        let mut shown = None;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            shown = Some(small_caps_sample(ui, "Aɢ", Color32::WHITE));
        });
        out.textures_delta.clear();
        let g = shown.expect("drawn");
        assert_eq!(g.job.text, "AG");
        let [a, rest] = g.job.sections.as_slice() else { panic!("two sections: {:?}", g.job.sections) };
        assert!(rest.format.font_id.size < a.format.font_id.size, "the other letters are smaller capitals");
        ctx.fonts_mut(|f| {
            for sec in &g.job.sections {
                assert!(f.has_glyphs(&sec.format.font_id, &g.job.text), "UI font draws {:?}", g.job.text);
            }
        });
        assert_eq!(small_capital_to_capital('ʀ'), 'R');
        assert_eq!(small_capital_to_capital('x'), 'x');
    }

    #[test]
    fn parses_line_height_and_letter_spacing() {
        assert_eq!(parse_line_height("Auto", None), Some(json!("auto")));
        assert_eq!(parse_line_height("", Some(LineHeight::Pixels(20.0))), Some(json!("auto")));
        assert_eq!(parse_line_height("150%", Some(LineHeight::Auto)), Some(json!("150%")));
        assert_eq!(parse_line_height("24", Some(LineHeight::Auto)), Some(json!(24.0)));
        assert_eq!(parse_line_height("24px", Some(LineHeight::Percent(120.0))), Some(json!(24.0)));
        assert_eq!(parse_line_height("130", Some(LineHeight::Percent(120.0))), Some(json!(130.0)));
        assert_eq!(parse_line_height("110%", Some(LineHeight::Pixels(24.0))), Some(json!("110%")));
        assert_eq!(parse_line_height("auto", Some(LineHeight::Percent(120.0))), Some(json!("auto")));
        assert_eq!(parse_line_height("-5", Some(LineHeight::Pixels(10.0))), Some(json!(0.0)));
        assert_eq!(parse_line_height("abc", None), None);
        // Enter on an untouched Mixed field commits nothing; typing Auto still does.
        assert_eq!(parse_line_height("", None), None);
        assert_eq!(parse_line_height("  ", None), None);
        assert_eq!(parse_letter_spacing("2%", None), Some(json!("2%")));
        assert_eq!(parse_letter_spacing("1.5px", None), Some(json!(1.5)));
        assert_eq!(parse_letter_spacing("-1", None), Some(json!("-1%")));
        assert_eq!(parse_letter_spacing("3", Some(LetterSpacing::Pixels(1.0))), Some(json!(3.0)));
        assert_eq!(parse_letter_spacing("x", Some(LetterSpacing::Percent(0.0))), None);
    }

    /// One frame of the font picker with `events`.
    fn pick(ctx: &egui::Context, current: &str, opened: bool, events: Vec<egui::Event>) -> FontPick {
        let mut out = FontPick::default();
        let mut shown = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| out = font_picker(ui, Some(current), opened));
        shown.textures_delta.clear();
        out
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn arrow_keys_step_through_the_families_and_enter_picks() {
        // #345
        figcraft_text::scan_system_fonts();
        let all = figcraft_text::family_names();
        if all.len() < 3 {
            eprintln!("fewer than three font families; skipping");
            return;
        }
        let ctx = egui::Context::default();
        let current = all[1].as_str();
        assert!(pick(&ctx, current, true, vec![]).highlighted.is_none());
        // The first step starts from the current family; each press is one step.
        let down = pick(&ctx, current, false, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(down.highlighted.as_deref(), Some(all[2].as_str()));
        let up = pick(&ctx, current, false, vec![key(egui::Key::ArrowUp), key(egui::Key::ArrowUp)]);
        assert_eq!(up.highlighted.as_deref(), Some(all[0].as_str()));
        // At the top it stays; Enter picks the family it is on, not the first.
        assert_eq!(pick(&ctx, current, false, vec![key(egui::Key::ArrowUp)]).highlighted.as_deref(), Some(all[0].as_str()));
        pick(&ctx, current, false, vec![key(egui::Key::ArrowDown)]);
        let enter = pick(&ctx, current, false, vec![key(egui::Key::Enter)]);
        assert_eq!(enter.picked.as_deref(), Some(all[1].as_str()));
    }

    #[test]
    fn axis_values_round_by_axis_width() {
        let axis = |min: f64, max: f64| figcraft_text::Axis { tag: "test".into(), name: "Test".into(), min, default: min, max, hidden: false };
        assert_eq!(round_axis(&axis(100.0, 900.0), 456.6), 457.0);
        assert_eq!(round_axis(&axis(-10.0, 0.0), -4.26), -4.3);
        assert_eq!(round_axis(&axis(0.0, 1.0), 0.04), 0.0);
    }

    #[test]
    fn uniform_reports_mixed() {
        let a = TextStyle::default();
        let b = TextStyle { font_size: 30.0, ..Default::default() };
        assert_eq!(uniform(&[a.clone(), b.clone()], |s| s.font_family.clone()), Some("Inter".to_string()));
        assert_eq!(uniform(&[a.clone(), b], |s| s.font_size), None);
        assert_eq!(uniform(&[], |s: &TextStyle| s.font_size), None);
        // Ranges pick only the spans they touch.
        let mut doc = Document::new("t");
        let page = doc.first_page().unwrap();
        let id = doc.alloc_id();
        let mut td = TextData::new("Hello world");
        td.restyle(6, 11, |s| s.font_size = 40.0);
        doc.insert(Node::new(id, NodeKind::Text(td), "t", 10.0, 10.0), page, None).unwrap();
        assert_eq!(target_styles(&doc, &[id], Some((id, 0, 5))).len(), 1);
        assert_eq!(target_styles(&doc, &[id], Some((id, 4, 8))).len(), 2);
        assert_eq!(target_styles(&doc, &[id], Some((id, 3, 3))).len(), 2);
        assert_eq!(target_styles(&doc, &[id], None).len(), 2);
    }
}
