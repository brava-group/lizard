//! The style browser: the popover a Fill, Stroke, Effects, Layout grid or Typography section's
//! Styles button opens, laid out like the variable picker. Search; this file's styles, then each
//! enabled library's published ones, grouped by the folders of their `/`-separated names (folders
//! collapse); a list or, for colour and text styles, a grid. Text styles show in their own font,
//! and while the pointer is over one the canvas shows it on the selected text (or the selected
//! chars of the text being edited). Click applies a style (a library's is copied in, linked); a
//! local style's edit button (or a right-click) edits its name, description and values; `+`
//! creates a style from the selection (for text, the selected chars); the detach button unlinks
//! it. Colour slots also list colour variables to bind (this file's, then each enabled library's).

use egui::{Align2, Color32, CornerRadius, Id, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use lizard_color::{Paint, PaintKind};
use lizard_doc::{
    Document, Effect, GridKind, LayoutGrid, Node, NodeId, NodeKind, PublishedLibrary, Style, StyleKind, TextStyle, VariableType, trim_num,
};
use lizard_engine::library;
use serde_json::{Value, json};

use crate::LizardApp;
use crate::a11y::Tip;
use crate::colorpicker::PickerTarget;
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon, LayoutIcon};
use crate::primitives::{Button, ButtonBehavior, ControlWidth, SearchField, Select, SelectPosition, Tab, Tabs};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets;

/// Content width of the popover.
const W: f32 = 240.0;
/// Height of a list row.
const ROW_H: f32 = 32.0;
/// Height of a text style's list row (its name in its own font).
const TEXT_ROW_H: f32 = 32.0;
/// Side of a swatch in the grid view.
const SWATCH: f32 = 24.0;
/// A text style's tile in the grid view.
const TILE: egui::Vec2 = egui::vec2(54.0, 44.0);
/// Largest size a text style's name previews at in the list (the grid's "Ag" goes to 24).
const LIST_PREVIEW_MAX: f64 = 16.0;

/// A style slot on a layer: what a style picker lists and what a style chip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleSlot {
    Fill,
    Stroke,
    Effect,
    Grid,
    Text,
}

impl StyleSlot {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fill" => Some(StyleSlot::Fill),
            "stroke" => Some(StyleSlot::Stroke),
            "effect" => Some(StyleSlot::Effect),
            "grid" => Some(StyleSlot::Grid),
            "text" => Some(StyleSlot::Text),
            _ => None,
        }
    }

    /// The slot's name in `style.apply {target}`, `style.detach {which}` and `variable.bind {field}`.
    pub fn key(self) -> &'static str {
        match self {
            StyleSlot::Fill => "fill",
            StyleSlot::Stroke => "stroke",
            StyleSlot::Effect => "effect",
            StyleSlot::Grid => "grid",
            StyleSlot::Text => "text",
        }
    }

    fn linked(self, n: &Node) -> Option<String> {
        match self {
            StyleSlot::Fill => n.styles.fill.clone(),
            StyleSlot::Stroke => n.styles.stroke.clone(),
            StyleSlot::Effect => n.styles.effect.clone(),
            StyleSlot::Grid => n.styles.grid.clone(),
            StyleSlot::Text => n.styles.text.clone(),
        }
    }

    fn paints(self) -> bool {
        matches!(self, StyleSlot::Fill | StyleSlot::Stroke)
    }

    /// Whether the slot's styles also show as a grid (swatches, "Ag" tiles).
    fn has_grid(self) -> bool {
        self.paints() || self == StyleSlot::Text
    }

    fn title(self) -> &'static str {
        match self {
            StyleSlot::Fill | StyleSlot::Stroke => tr("Color styles"),
            StyleSlot::Effect => tr("Effect styles"),
            StyleSlot::Grid => tr("Grid styles"),
            StyleSlot::Text => tr("Text styles"),
        }
    }

    /// `style.create {type}` for this slot (taken from the layer's fills, strokes, effects, grids
    /// or text).
    fn create_type(self) -> &'static str {
        match self {
            StyleSlot::Fill => "paint",
            StyleSlot::Stroke => "stroke",
            StyleSlot::Effect => "effect",
            StyleSlot::Grid => "grid",
            StyleSlot::Text => "text",
        }
    }

    /// The base of a new style's name ("Color 3").
    fn default_name(self) -> &'static str {
        match self {
            StyleSlot::Fill | StyleSlot::Stroke => "Color",
            StyleSlot::Effect => "Effect",
            StyleSlot::Grid => "Grid",
            StyleSlot::Text => "Text",
        }
    }

    fn holds(self, k: &StyleKind) -> bool {
        matches!(
            (k, self),
            (StyleKind::Paint { .. }, StyleSlot::Fill | StyleSlot::Stroke)
                | (StyleKind::Effect { .. }, StyleSlot::Effect)
                | (StyleKind::Grid { .. }, StyleSlot::Grid)
                | (StyleKind::Text { .. }, StyleSlot::Text)
        )
    }

    fn icon(self) -> Icon {
        match self {
            StyleSlot::Grid => Icon::Grid,
            StyleSlot::Text => Icon::Text,
            _ => Icon::Effects,
        }
    }
}

/// The style being edited, with its name and description as typed.
#[derive(Clone, Debug)]
struct Editing {
    id: String,
    name: String,
    description: String,
    /// Focus the name (and select it) on the next frame: a style that was just created.
    focus: bool,
    creating: bool,
}

/// The open picker.
#[derive(Clone, Debug)]
pub struct Picker {
    slot: StyleSlot,
    ids: Vec<NodeId>,
    /// Screen y of the button that opened it.
    y: f32,
    query: String,
    edit: Option<Editing>,
    /// Colour styles show local colours in Custom and all sources in Libraries.
    libraries_tab: bool,
    /// The library selected in the colour picker: None is all, `_local` is this file.
    library_filter: Option<String>,
    /// The book button narrows the text and effect pickers to enabled libraries.
    libraries_only: bool,
}

impl Picker {
    pub fn slot(&self) -> StyleSlot {
        self.slot
    }
}

/// Open the picker for `slot` of `ids`, level with the pointer.
pub fn open(app: &mut LizardApp, ui: &Ui, ids: &[NodeId], slot: StyleSlot) {
    let y = ui.input(|i| i.pointer.interact_pos()).map_or(140.0, |p| p.y - 12.0);
    ui.data_mut(|d| d.insert_temp(Id::new("style_picker_just_opened"), true));
    app.ui.style_picker = Some(Picker {
        slot,
        ids: ids.to_vec(),
        y,
        query: String::new(),
        edit: None,
        libraries_tab: true,
        library_filter: None,
        libraries_only: false,
    });
}

/// Open the picker for `slot` of the selection (the control channel), optionally editing `style`.
pub fn open_for_selection(app: &mut LizardApp, slot: StyleSlot, style: Option<&str>) {
    let ids = app.session.selection();
    let edit = style.and_then(|s| app.session.doc().ok()?.style(s).map(editing));
    app.ui.style_picker =
        Some(Picker { slot, ids, y: 140.0, query: String::new(), edit, libraries_tab: true, library_filter: None, libraries_only: false });
}

fn editing(s: &Style) -> Editing {
    Editing { id: s.id.clone(), name: s.name.clone(), description: s.description.clone(), focus: false, creating: false }
}

/// The style `slot` of `ids` links to: `Some(Some(id))` one style, `Some(None)` none, None when
/// they differ. Text goes by chars: the selected chars of the text being edited, else every char.
fn link_of(app: &LizardApp, doc: &Document, ids: &[NodeId], slot: StyleSlot) -> Option<Option<String>> {
    if slot != StyleSlot::Text {
        return crate::right::common(doc, ids, |n| slot.linked(n));
    }
    let range = lizard_engine::textedit::selected_range(&app.session);
    let styles = crate::typography::target_styles(doc, ids, range);
    let mut links = styles.iter().map(|s| s.style_id.clone());
    let Some(first) = links.next() else { return Some(None) };
    links.all(|l| l == first).then_some(first)
}

/// The text style the targeted chars of `ids` link to (see [`link_of`]).
pub fn text_link(app: &LizardApp, doc: &Document, ids: &[NodeId]) -> Option<Option<String>> {
    link_of(app, doc, ids, StyleSlot::Text)
}

/// The applied style's name (click opens the picker) with a detach button; for text, "Mixed"
/// when the chars link to different styles (or some to none).
pub fn chip(app: &mut LizardApp, ui: &mut Ui, doc: &Document, ids: &[NodeId], slot: StyleSlot) {
    let t = Tokens::get(ui.ctx());
    let (name, mixed) = match link_of(app, doc, ids, slot) {
        Some(Some(id)) => (doc.style(&id).map_or_else(|| tr("Missing style").to_string(), |st| st.name.clone()), false),
        None if slot == StyleSlot::Text => (tr("Mixed").to_string(), true),
        _ => return,
    };
    let mut open_it = false;
    crate::right::row(ui, 32.0, |ui| {
        let w = ui.available_width().max(40.0);
        let (r, resp) = ui.allocate_exact_size(vec2(w, widgets::FIELD_H), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(5), t.hover);
        }
        let icon = if slot == StyleSlot::Text { Icon::Text } else { Icon::Styles };
        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(14.0, 14.0)), icon, t.icon_secondary);
        let col = if mixed { t.text_secondary } else { t.text };
        crate::bidi::paint_name(ui.painter(), pos2(r.min.x + 26.0, r.center().y), &name, regular(11.0), col, r.width() - 32.0);
        let tip = if mixed { tr("The selection uses several text styles: click to apply one") } else { tr("Change style") };
        open_it = resp.on_hover_text(tip).clicked();
        let detach = if mixed { tr("Detach styles") } else { tr("Detach style") };
        if icons::button(ui, Icon::Detach, 24.0, false, detach).clicked() {
            app.run("style.detach", json!({"ids": ids, "which": slot.key()}));
        }
    });
    if open_it {
        open(app, ui, ids, slot);
    }
}

/// A listed style: by its library's id (None: this file's) and its id there.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    library: Option<String>,
    style: String,
}

impl Key {
    /// What `local` (a style in this document) is: a library style's copy, or this file's own.
    fn of_local(doc: &Document, local: &str) -> Key {
        match doc.libraries.style_import(local) {
            Some(r) => Key { library: Some(r.library.clone()), style: r.key.clone() },
            None => Key { library: None, style: local.to_string() },
        }
    }

    /// The command (and its params) applying this style to `ids`.
    fn apply(&self, ids: &[NodeId], slot: StyleSlot) -> (&'static str, Value) {
        let mut q = match &self.library {
            Some(lib) => json!({"library": lib, "style": self.style, "ids": ids}),
            None => json!({"ids": ids, "style": self.style}),
        };
        if slot.paints() {
            q["target"] = json!(slot.key());
        }
        (if self.library.is_some() { "library.applyStyle" } else { "style.apply" }, q)
    }
}

/// What the user asked the picker to do this frame.
enum Act {
    Apply(Key),
    UseCustomColor,
    Bind(crate::variable_ui::Pick),
    Create,
    Edit(String),
    Back,
    Close,
    EditPaint(String, usize),
    /// Unbind the variable on a field of a style.
    Unbind(String, String),
    /// A text style's new definition (from the editor's fields).
    SetText(String, TextStyle, Option<bool>),
    SetPaints(String, Vec<Paint>, Option<bool>),
    SetEffects(String, Vec<Effect>, Option<bool>),
    SetGrids(String, Vec<LayoutGrid>, Option<bool>),
    UpdateFromSelection(String),
    Delete(String),
    ToggleFolder(String),
    ToggleView,
}

/// What a listing reports besides acts: the style under the pointer, and the preview budget.
struct Frame {
    act: Option<Act>,
    hover: Option<Key>,
    /// Text previews still allowed to render this frame.
    budget: usize,
}

/// An icon button on the dark popover.
fn menu_icon(ui: &mut Ui, rect: Rect, icon: Icon, tip: &str, on: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(rect, ui.id().with(("style_picker_btn", tip)), Sense::click()).tip(tip);
    if on || resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.menu_hover);
    }
    icons::paint(ui.painter(), rect.shrink(5.0), icon, t.menu_text);
    resp
}

/// A text button on the dark popover.
fn menu_button(ui: &mut Ui, label: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let galley = ui.painter().layout_no_wrap(label.to_string(), regular(11.0), t.menu_text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 20.0, widgets::FIELD_H), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(5), if resp.hovered() { t.menu_hover } else { Color32::TRANSPARENT });
    ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.menu_dim), egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, regular(11.0), t.menu_text);
    resp
}

/// A dim caption on the popover.
fn caption(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(text).font(regular(10.0)).color(t.menu_dim));
}

/// The folder part of a style name ("Brand/Blue/500" → "Brand/Blue"), empty at the top level.
fn folder(name: &str) -> &str {
    name.rsplit_once('/').map_or("", |(f, _)| f.trim())
}

/// The last part of a style name ("Brand/Blue/500" → "500").
fn leaf(name: &str) -> &str {
    name.rsplit_once('/').map_or(name, |(_, l)| l.trim())
}

/// `base N` with the smallest N no style of the document is named.
fn next_name(doc: &Document, base: &str) -> String {
    (1..).map(|n| format!("{base} {n}")).find(|n| doc.styles.iter().all(|s| &s.name != n)).unwrap_or_else(|| base.to_string())
}

/// Styles by folder, folders in order of first appearance, top-level styles first.
fn by_folder<'a>(styles: impl Iterator<Item = &'a Style>) -> Vec<(&'a str, Vec<&'a Style>)> {
    let mut groups: Vec<(&str, Vec<&Style>)> = Vec::new();
    for s in styles {
        let f = folder(&s.name);
        match groups.iter_mut().find(|(g, _)| *g == f) {
            Some((_, list)) => list.push(s),
            None => groups.push((f, vec![s])),
        }
    }
    groups.sort_by_key(|(f, _)| !f.is_empty());
    groups
}

/// A text style's definition, if `s` is one.
fn text_def(s: &Style) -> Option<&TextStyle> {
    match &s.kind {
        StyleKind::Text { style } => Some(style),
        _ => None,
    }
}

/// "32/Auto": a text style's size and line height.
fn text_meta(def: &TextStyle) -> String {
    format!("{}/{}", trim_num(def.font_size), def.line_height.label())
}

/// A text style's sample (`text` in its own font at most `max` big), painted from `left_center`;
/// falls back to plain UI text until the preview renders (or when the font is missing).
fn text_sample(ui: &Ui, def: &TextStyle, text: &str, max: f64, left_center: egui::Pos2, max_w: f32, max_h: f32, budget: &mut usize) {
    let t = Tokens::get(ui.ctx());
    let shown = TextStyle { font_size: def.font_size.clamp(6.0, max), ..def.clone() };
    match crate::font_preview::texture(ui.ctx(), &shown, text, budget) {
        Some(tex) => {
            crate::font_preview::paint(ui.painter(), &tex, left_center, max_w, max_h, t.menu_text);
        }
        None => {
            crate::bidi::paint_name(ui.painter(), left_center, text, regular(11.0), t.menu_text, max_w);
        }
    }
}

/// Draw a style's preview (its first paint, or the slot's icon) centred in `r`.
fn preview(ui: &Ui, r: Rect, s: &Style, slot: StyleSlot) {
    let t = Tokens::get(ui.ctx());
    match &s.kind {
        StyleKind::Paint { paints } => {
            let p = paints.last().cloned().unwrap_or_else(|| Paint::solid(lizard_color::Color::TRANSPARENT));
            widgets::paint_swatch(ui.painter(), r, &p);
        }
        _ => icons::paint(ui.painter(), r, slot.icon(), t.menu_text),
    }
}

/// Click, right-click and hover on a listed style become acts.
fn react(resp: &Response, key: Key, editable: bool, out: &mut Frame) {
    if resp.hovered() {
        out.hover = Some(key.clone());
    }
    if resp.secondary_clicked() && editable {
        out.act = Some(Act::Edit(key.style));
    } else if resp.clicked() {
        out.act = Some(Act::Apply(key));
    }
}

/// One style in the list: preview (a text style: its name in its own font and its size), name,
/// a check when applied and an edit button on hover.
fn style_row(ui: &mut Ui, s: &Style, key: Key, slot: StyleSlot, applied: bool, editable: bool, out: &mut Frame) {
    let t = Tokens::get(ui.ctx());
    let h = if slot == StyleSlot::Text { TEXT_ROW_H } else { ROW_H };
    let width = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    let edit_r = Rect::from_center_size(pos2(r.max.x - 20.0, r.center().y), vec2(24.0, 24.0));
    let hovered = resp.hovered() || ui.rect_contains_pointer(edit_r);
    if hovered || applied {
        ui.painter().rect_filled(r, CornerRadius::ZERO, t.menu_hover);
    }
    match text_def(s) {
        Some(def) => {
            let meta = format!("· {}", text_meta(def));
            let meta_w = ui.painter().layout_no_wrap(meta.clone(), regular(10.0), t.menu_dim).size().x;
            let name_x = r.min.x + 48.0;
            let name_w = (edit_r.min.x - name_x - meta_w - 8.0).max(12.0);
            text_sample(ui, def, "Ag", LIST_PREVIEW_MAX, pos2(r.min.x + 16.0, r.center().y), 27.0, h - 4.0, &mut out.budget);
            let name = widgets::elide_font(ui, leaf(&s.name), name_w, regular(11.0));
            let shown_w = ui.painter().layout_no_wrap(name.clone(), regular(11.0), t.menu_text).size().x;
            crate::bidi::paint_name(ui.painter(), pos2(name_x, r.center().y), &name, regular(11.0), t.menu_text, name_w);
            ui.painter().text(
                pos2(name_x + shown_w + 4.0, r.center().y),
                Align2::LEFT_CENTER,
                meta,
                regular(10.0),
                if hovered { t.menu_text } else { t.menu_dim },
            );
        }
        None => {
            let icon_rect = Rect::from_center_size(pos2(r.min.x + 24.0, r.center().y), vec2(16.0, 16.0));
            if slot == StyleSlot::Effect {
                icons::paint(ui.painter(), icon_rect, Icon::Rectangle, t.menu_text);
            } else if let StyleKind::Paint { paints } = &s.kind {
                if let Some(paint) = paints.last() {
                    if paint.solid_color().is_some() {
                        ui.painter().circle_filled(icon_rect.center(), 8.0, crate::theme::doc_c32(ui.ctx(), paint.preview_color()));
                    } else {
                        widgets::paint_swatch(ui.painter(), icon_rect, paint);
                    }
                }
            } else {
                preview(ui, icon_rect, s, slot);
            }
            crate::bidi::paint_name(ui.painter(), pos2(r.min.x + 48.0, r.center().y), leaf(&s.name), regular(11.0), t.menu_text, width - 88.0);
        }
    }
    // A style with variables bound inside it is marked (its users follow their own modes).
    if !s.bound.is_empty() {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(edit_r.min.x - 10.0, r.center().y), vec2(11.0, 11.0)), Icon::Variable, t.menu_dim);
    }
    if hovered && editable {
        if Button::icon(Icon::Adjust, tr("Edit style"))
            .icon_size(14.0)
            .show_at(ui, Id::new(("style_picker_edit", &key.library, &key.style)), edit_r)
            .clicked()
        {
            out.act = Some(Act::Edit(s.id.clone()));
            return;
        }
    } else if applied {
        icons::paint(ui.painter(), edit_r.shrink(5.0), Icon::Check, t.menu_text);
    }
    let tip = match (s.description.is_empty(), text_def(s)) {
        (true, None) if !s.bound.is_empty() => Some(format!("{}\n{}", s.name, tr("Uses variables: each layer shows them in its own mode"))),
        (true, None) => None,
        (true, Some(def)) => Some(format!("{}\n{} {} · {}", s.name, def.font_family, def.font_style, text_meta(def))),
        (false, _) => Some(s.description.clone()),
    };
    let resp = match tip {
        Some(tip) => resp.on_hover_text(tip),
        None => resp,
    };
    react(&resp, key, editable, out);
}

/// Colour styles as swatches and text styles as "Ag" tiles in their own font (names on hover).
fn style_grid(ui: &mut Ui, list: &[(&Style, Key)], applied: Option<&Key>, editable: impl Fn(&Key) -> bool, out: &mut Frame) {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    let text = list.first().is_some_and(|(s, _)| text_def(s).is_some());
    let cell = if text { TILE } else { vec2(SWATCH, SWATCH) };
    let per_row = ((width - 8.0 + 6.0) / (cell.x + 6.0)).floor().max(1.0) as usize;
    for chunk in list.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            for (s, key) in chunk {
                let (r, resp) = ui.allocate_exact_size(cell, Sense::click());
                match text_def(s) {
                    Some(def) => {
                        let fill = if resp.hovered() { t.menu_hover } else { Color32::from_white_alpha(14) };
                        ui.painter().rect_filled(r, CornerRadius::same(5), fill);
                        let sample_w = ui.painter().layout_no_wrap("Ag".into(), regular(18.0), t.menu_text).size().x;
                        text_sample(
                            ui,
                            def,
                            "Ag",
                            24.0,
                            pos2(r.center().x - sample_w / 2.0, r.center().y),
                            r.width() - 6.0,
                            r.height() - 4.0,
                            &mut out.budget,
                        );
                    }
                    None => preview(ui, r, s, StyleSlot::Fill),
                }
                if applied == Some(key) {
                    ui.painter().rect_stroke(r.expand(2.0), CornerRadius::same(5), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
                } else if resp.hovered() && !text {
                    ui.painter().rect_stroke(r.expand(2.0), CornerRadius::same(4), Stroke::new(1.0, t.menu_text), egui::StrokeKind::Inside);
                }
                let tip = match text_def(s) {
                    Some(def) => format!("{}\n{} {} · {}", s.name, def.font_family, def.font_style, text_meta(def)),
                    None => s.name.clone(),
                };
                let resp = resp.on_hover_text(tip);
                react(&resp, key.clone(), editable(key), out);
            }
        });
        ui.add_space(2.0);
    }
}

/// The styles a source offers (this file's, or a library's published ones) that the slot holds
/// and the search matches.
fn offered<'a>(doc: &'a Document, lib: Option<&'a PublishedLibrary>, slot: StyleSlot, q: &str) -> Vec<&'a Style> {
    let matches = |s: &&Style| slot.holds(&s.kind) && (q.is_empty() || s.name.to_lowercase().contains(q));
    match lib {
        Some(l) => l.doc.styles.iter().filter(|s| l.manifest.style(&s.id).is_some_and(|x| x.published)).filter(matches).collect(),
        None => library::local_styles(doc).filter(matches).collect(),
    }
}

/// The style list: header, search, this file's and the libraries' styles by folder, and (colour
/// slots) colour variables.
fn listing(app: &LizardApp, ui: &mut Ui, doc: &Document, p: &mut Picker, out: &mut Frame, header: Rect) {
    let t = Tokens::get(ui.ctx());
    let slot = p.slot;
    let color = slot.paints();
    let grid = slot.has_grid() && app.ui.style_grid;
    let applied = link_of(app, doc, &p.ids, slot).flatten().map(|id| Key::of_local(doc, &id));
    let border = Stroke::new(1.0, t.border);

    if color {
        let tabs = [Tab::new(tr("Custom")), Tab::new(tr("Libraries"))];
        let tabs_rect = Rect::from_min_size(pos2(header.left() + 8.0, header.top() + 4.0), vec2(168.0, 32.0));
        let mut tabs_ui = ui.new_child(egui::UiBuilder::new().max_rect(tabs_rect));
        if let Some(index) = Tabs::new(Id::new("color_style_picker_tabs"), &tabs, Some(usize::from(p.libraries_tab)))
            .width(ControlWidth::Fixed(168.0))
            .selected_fill(if t.dark { crate::primitives::FloatingPanel::DARK_FIELD } else { t.field })
            .show(&mut tabs_ui)
        {
            p.libraries_tab = index == 1;
        }
    } else {
        ui.painter().text(pos2(header.left() + 16.0, header.center().y), Align2::LEFT_CENTER, slot.title(), semibold(11.0), t.text);
        let book = Rect::from_center_size(pos2(header.right() - 88.0, header.center().y), vec2(24.0, 24.0));
        let book_button = Button::icon(Icon::Library, tr("Libraries"))
            .behavior(ButtonBehavior::Toggle { selected: p.libraries_only })
            .icon_size(16.0)
            .show_at(ui, Id::new(("style_picker_libraries", slot.key())), book);
        crate::right::menu_popup(&book_button).show(|ui| {
            if crate::right::menu_check(ui, !p.libraries_only, tr("Styles"), "") {
                p.libraries_only = false;
                ui.close();
            }
            if crate::right::menu_check(ui, p.libraries_only, tr("Libraries"), "") {
                p.libraries_only = true;
                ui.close();
            }
            if slot.has_grid() {
                crate::primitives::Menu::separator(ui, None);
                let label = if grid { tr("Show as list") } else { tr("Show as grid") };
                if crate::right::menu_item(ui, label, "") {
                    out.act = Some(Act::ToggleView);
                    ui.close();
                }
            }
        });
    }
    let add_rect = Rect::from_center_size(pos2(header.right() - 52.0, header.center().y), vec2(24.0, 24.0));
    let create_tip = if slot == StyleSlot::Text { tr("Create style from the selected text") } else { tr("Create style") };
    if Button::icon(Icon::Plus, create_tip).icon_size(16.0).show_at(ui, Id::new(("style_picker_add", slot.key())), add_rect).clicked() {
        out.act = Some(Act::Create);
    }
    let close_rect = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, Id::new(("style_picker_close", slot.key())), close_rect).clicked() {
        out.act = Some(Act::Close);
    }
    ui.painter().hline(header.x_range(), header.bottom() - 0.5, border);

    let (search_row, _) = ui.allocate_exact_size(vec2(W, 40.0), Sense::hover());
    let search_rect = Rect::from_min_size(pos2(search_row.left() + 16.0, search_row.top() + 8.0), vec2(W - 32.0, 24.0));
    let mut search_ui = ui.new_child(egui::UiBuilder::new().max_rect(search_rect));
    SearchField::new(Id::new(("style_picker_search", slot.key())), tr("Search"))
        .width(ControlWidth::Fill)
        .compact()
        .clearable()
        .ghost()
        .show(&mut search_ui, &mut p.query);
    ui.painter().hline(search_row.x_range(), search_row.bottom() - 0.5, border);

    let libs = crate::variable_ui::libraries(app, doc);
    if color && p.libraries_tab {
        let (filter_row, _) = ui.allocate_exact_size(vec2(W, 40.0), Sense::hover());
        let mut names = vec![tr("All libraries").to_string(), tr("This file").to_string()];
        names.extend(libs.iter().map(|library| library.manifest.name.clone()));
        let labels: Vec<&str> = names.iter().map(String::as_str).collect();
        let selected = match p.library_filter.as_deref() {
            Some("_local") => 1,
            Some(id) => libs.iter().position(|library| library.manifest.id == id).map_or(0, |index| index + 2),
            None => 0,
        };
        let select_rect = Rect::from_min_size(pos2(filter_row.left() + 16.0, filter_row.top() + 8.0), vec2(160.0, 24.0));
        let mut select_ui = ui.new_child(egui::UiBuilder::new().max_rect(select_rect));
        if let Some(index) =
            Select::new(&labels, selected).label(tr("Libraries")).width(ControlWidth::Fill).position(SelectPosition::BottomLeft).show(&mut select_ui)
        {
            p.library_filter = match index {
                0 => None,
                1 => Some("_local".to_string()),
                other => libs.get(other - 2).map(|library| library.manifest.id.clone()),
            };
        }
        let view_rect = Rect::from_center_size(pos2(filter_row.right() - 24.0, filter_row.center().y), vec2(24.0, 24.0));
        let (icon, hint) = if grid { (Icon::ViewList, tr("Show as list")) } else { (Icon::ViewGrid, tr("Show as grid")) };
        if Button::icon(icon, hint).icon_size(16.0).show_at(ui, Id::new("color_style_picker_view"), view_rect).clicked() {
            out.act = Some(Act::ToggleView);
        }
        ui.painter().hline(filter_row.x_range(), filter_row.bottom() - 0.5, border);
    }

    let q = p.query.trim().to_lowercase();
    let filter = if color && !p.libraries_tab { None } else { p.library_filter.as_deref() };
    let local = !p.libraries_only && (!color || !p.libraries_tab || filter.is_none() || filter == Some("_local"));
    let remote = !color || p.libraries_tab;
    let mut sources: Vec<(Option<&PublishedLibrary>, Vec<&Style>)> = Vec::new();
    if local {
        sources.push((None, offered(doc, None, slot, &q)));
    }
    if remote && filter != Some("_local") {
        for library in &libs {
            if color && filter.is_some_and(|id| id != library.manifest.id.as_str()) {
                continue;
            }
            let list = offered(doc, Some(library), slot, &q);
            if !list.is_empty() {
                sources.push((Some(library.as_ref()), list));
            }
        }
    }
    let colors = if color {
        crate::variable_ui::groups(doc, &libs, |_, variable| {
            variable.kind == VariableType::Color && (q.is_empty() || variable.name.to_lowercase().contains(&q))
        })
    } else {
        Vec::new()
    };
    let current_paint = if color && local {
        p.ids.first().and_then(|id| doc.node(*id)).and_then(|node| if slot == StyleSlot::Fill { node.fills.last() } else { node.strokes.last() })
    } else {
        None
    };
    let list_height = if color { if p.libraries_tab { 307.0 } else { 347.0 } } else { 300.0 };
    egui::ScrollArea::vertical().id_salt(("style_picker_scroll", slot.key())).max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(W - 12.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(8.0);
        if let Some(paint) = current_paint {
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            if response.hovered() || applied.is_none() {
                ui.painter().rect_filled(rect, CornerRadius::ZERO, t.menu_hover);
            }
            widgets::paint_swatch(ui.painter(), Rect::from_center_size(pos2(rect.left() + 28.0, rect.center().y), vec2(16.0, 16.0)), paint);
            ui.painter().text(pos2(rect.left() + 48.0, rect.center().y), Align2::LEFT_CENTER, tr("Color"), regular(11.0), t.text);
            if response.clicked() {
                out.act = Some(Act::UseCustomColor);
            }
        }
        if sources.iter().all(|(_, styles)| styles.is_empty()) && current_paint.is_none() {
            let any = library::local_styles(doc).any(|style| slot.holds(&style.kind));
            let hint = match (any, slot) {
                (true, _) => tr("No matching styles"),
                (false, StyleSlot::Text) => tr("No text styles yet. Click + to create one from the selected text."),
                (false, _) => tr("No styles yet. Click + to create one from the selection."),
            };
            ui.add_space(12.0);
            caption(ui, hint);
        }
        for (library, styles) in &sources {
            if styles.is_empty() {
                continue;
            }
            if let Some(library) = library {
                ui.add_space(12.0);
                let (title, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
                ui.painter().hline(title.left()..=title.right() + 12.0, title.top() - 0.5, border);
                ui.painter().text(pos2(title.left() + 16.0, title.center().y), Align2::LEFT_CENTER, &library.manifest.name, semibold(11.0), t.text);
            } else if color {
                let (title, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                ui.painter().text(pos2(title.left() + 16.0, title.center().y), Align2::LEFT_CENTER, tr("Styles"), semibold(11.0), t.text);
            }
            let library_id = library.map(|library| library.manifest.id.clone());
            for (folder, group) in by_folder(styles.iter().copied()) {
                let fold = library_id.as_ref().map_or_else(|| folder.to_string(), |id| format!("{id}/{folder}"));
                if !folder.is_empty() {
                    let closed = q.is_empty() && app.ui.style_folds.contains(&fold);
                    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.menu_hover);
                        let icon = if closed { Icon::ChevronRight } else { Icon::Chevron };
                        icons::paint(
                            ui.painter(),
                            Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(10.0, 10.0)),
                            icon,
                            t.icon_secondary,
                        );
                    }
                    crate::bidi::paint_name(
                        ui.painter(),
                        pos2(rect.left() + 16.0, rect.center().y),
                        folder,
                        regular(11.0),
                        t.text,
                        rect.width() - 48.0,
                    );
                    if response.clicked() && q.is_empty() {
                        out.act = Some(Act::ToggleFolder(fold));
                    }
                    if closed {
                        continue;
                    }
                }
                let keyed: Vec<(&Style, Key)> =
                    group.into_iter().map(|style| (style, Key { library: library_id.clone(), style: style.id.clone() })).collect();
                let editable = |key: &Key| key.library.is_none();
                if grid {
                    ui.add_space(4.0);
                    style_grid(ui, &keyed, applied.as_ref(), editable, out);
                } else {
                    for (style, key) in keyed {
                        let on = applied.as_ref() == Some(&key);
                        let can_edit = editable(&key);
                        style_row(ui, style, key, slot, on, can_edit, out);
                    }
                }
            }
        }
        for (library, collection, variables) in &colors {
            let library_id = library.map(|library| library.manifest.id.as_str());
            if (library_id.is_none() && !local)
                || (library_id.is_some() && (!remote || filter == Some("_local")))
                || (color && filter.is_some_and(|id| id != "_local" && Some(id) != library_id))
            {
                continue;
            }
            crate::variable_ui::group_caption(ui, *library, collection);
            let all = library.map_or(&doc.variables, |library| &library.doc.variables);
            for variable in variables {
                let width = ui.available_width();
                if crate::variable_ui::variable_row(ui, all, variable, width) {
                    out.act = Some(Act::Bind(crate::variable_ui::Pick {
                        library: library.map(|library| library.manifest.id.clone()),
                        id: variable.id.clone(),
                    }));
                }
            }
        }
    });
}

/// One property row in the editor (a paint, an effect, a grid); paints open the colour picker.
fn property_row(ui: &mut Ui, label: &str, paint: Option<&Paint>, slot: StyleSlot) -> bool {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(W, ROW_H), if paint.is_some() { Sense::click() } else { Sense::hover() });
    if paint.is_some() && resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(5), t.menu_hover);
    }
    let pr = Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(14.0, 14.0));
    match paint {
        Some(p) => widgets::paint_swatch(ui.painter(), pr, p),
        None => icons::paint(ui.painter(), pr, slot.icon(), t.menu_text),
    }
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, label, regular(11.0), t.menu_text);
    paint.is_some() && resp.clicked()
}

/// The variable on `field` of the style a [`property_row`] just drew: its name at the row's end
/// (a click unbinds it), or, with the pointer over the row, the button that opens the variable
/// picker for it. True when the pointer is on it (the row's own click isn't meant then).
fn binding_cell(ui: &mut Ui, doc: &Document, style: &Style, field: &str, kind: lizard_doc::VariableType, out: &mut Frame) -> bool {
    let t = Tokens::get(ui.ctx());
    let row = ui.min_rect();
    let row = Rect::from_min_max(pos2(row.min.x, row.max.y - ROW_H), row.max);
    let Some(name) = style.bound.get(field).map(|v| doc.variables.get(v).map_or(v.as_str(), |x| x.name.as_str())) else {
        let b = Rect::from_center_size(pos2(row.max.x - 13.0, row.center().y), vec2(20.0, 20.0));
        if ui.rect_contains_pointer(row) && menu_icon(ui, b, Icon::Variable, tr("Apply variable"), false).clicked() {
            crate::variable_ui::open_style(ui, &style.id, field, kind, row.left_bottom());
        }
        return ui.rect_contains_pointer(b);
    };
    let id = Id::new(("style_binding", &style.id, field));
    let shown = widgets::elide_font(ui, leaf(name), 90.0, regular(10.0));
    let w = ui.painter().layout_no_wrap(shown.clone(), regular(10.0), t.menu_dim).size().x + 22.0;
    let chip = Rect::from_min_max(pos2(row.max.x - w - 4.0, row.min.y + 4.0), pos2(row.max.x - 4.0, row.max.y - 4.0));
    let resp =
        ui.interact(chip, id, Sense::click()).on_hover_text(crate::i18n::trf("Bound to {name}. Click to detach the variable.", &[("name", &name)]));
    ui.painter().rect_filled(chip, CornerRadius::same(4), if resp.hovered() { t.menu_hover } else { t.menu });
    ui.painter().rect_stroke(chip, CornerRadius::same(4), Stroke::new(1.0, t.menu_dim.gamma_multiply(0.4)), egui::StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(chip.min.x + 9.0, chip.center().y), vec2(11.0, 11.0)), Icon::Variable, t.menu_dim);
    ui.painter().text(pos2(chip.min.x + 18.0, chip.center().y), Align2::LEFT_CENTER, shown, regular(10.0), t.menu_text);
    if resp.clicked() {
        out.act = Some(Act::Unbind(style.id.clone(), field.to_string()));
    }
    resp.hovered()
}

/// The field of effect `fx` (item `i` of an effect style) a variable binds from its row, and its
/// type: a shadow's colour, a blur's radius.
fn effect_binding(fx: &lizard_doc::Effect, i: usize) -> Option<(String, lizard_doc::VariableType)> {
    use lizard_doc::{EffectKind as K, VariableType as V};
    match fx.kind {
        K::DropShadow(_) | K::InnerShadow(_) => Some((format!("effects/{i}/color"), V::Color)),
        K::LayerBlur(_) | K::BackgroundBlur(_) => Some((format!("effects/{i}/radius"), V::Number)),
        _ => None,
    }
}

/// A layout grid's summary ("Columns 12", "Grid 8").
fn grid_label(g: &lizard_doc::LayoutGrid) -> String {
    match g.kind {
        GridKind::Grid => trf("Grid {size}", &[("size", &g.size)]),
        GridKind::Columns => trf("Columns {count}", &[("count", &g.count)]),
        GridKind::Rows => trf("Rows {count}", &[("count", &g.count)]),
    }
}

/// A text style's typography in the editor: a sample, then font, style, size, line height and
/// letter spacing; a change redefines the style (every linked char follows).
fn text_properties(ui: &mut Ui, id: &str, def: &TextStyle, out: &mut Frame) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(W, 40.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(5), Color32::from_white_alpha(14));
    text_sample(ui, def, "Ag", 28.0, pos2(r.min.x + 10.0, r.center().y), 60.0, r.height() - 4.0, &mut out.budget);
    let summary = format!("{} {}\n{}", def.font_family, def.font_style, text_meta(def));
    let g = ui.painter().layout(summary, regular(10.5), t.menu_dim, W - 80.0);
    ui.painter().galley(pos2(r.min.x + 72.0, r.center().y - g.size().y / 2.0), g, t.menu_dim);
    ui.add_space(6.0);
    let set = |out: &mut Frame, f: &dyn Fn(&mut TextStyle)| {
        let mut d = def.clone();
        f(&mut d);
        if d != *def {
            out.act = Some(Act::SetText(id.to_string(), d, None));
        }
    };
    let half = (W - 8.0) / 2.0;
    // The dropdowns' menus open outside the dark scope (they're regular popups).
    let (family_resp, style_resp) = crate::theme::on_dark(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        let family_resp = crate::primitives::Select::trigger(ui, &def.font_family, W, None).on_hover_text(tr("Font"));
        let mut style_resp = None;
        ui.horizontal(|ui| {
            style_resp = Some(crate::primitives::Select::trigger(ui, &def.font_style, half, None).on_hover_text(tr("Font style")));
            if let Some(widgets::FieldEdit::Set(v) | widgets::FieldEdit::Scrub(v, true)) =
                widgets::number_field(ui, Id::new(("ts_size", id)), widgets::Lead::Text(tr("Size")), Some(def.font_size), "", half, 1.0)
            {
                set(out, &|d| d.font_size = v.clamp(1.0, 5000.0));
            }
        });
        ui.horizontal(|ui| {
            if let Some(v) = widgets::text_field(ui, Id::new(("ts_lh", id)), &def.line_height.label(), half, tr("Line height"))
                && let Some(lh) = crate::typography::parse_line_height(&v, Some(def.line_height)).and_then(|v| crate::typography::line_height_of(&v))
            {
                set(out, &|d| d.line_height = lh);
            }
            if let Some(v) = widgets::text_field(ui, Id::new(("ts_ls", id)), &def.letter_spacing.label(), half, tr("Letter spacing"))
                && let Some(ls) =
                    crate::typography::parse_letter_spacing(&v, Some(def.letter_spacing)).and_then(|v| crate::typography::letter_spacing_of(&v))
            {
                set(out, &|d| d.letter_spacing = ls);
            }
        });
        (family_resp, style_resp)
    });
    crate::right::menu_popup(&family_resp).width(260.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        if let Some(f) = crate::typography::font_picker(ui, Some(&def.font_family), family_resp.clicked()).picked {
            set(out, &|d| {
                d.font_family = f.clone();
                let styles = lizard_text::family_styles(&f);
                if !styles.is_empty() && !styles.contains(&d.font_style) {
                    d.set_font_style(styles.iter().find(|s| s.eq_ignore_ascii_case("Regular")).or(styles.first()).cloned().unwrap_or_default());
                }
            });
            ui.close();
        }
    });
    if let Some(sr) = style_resp {
        crate::right::menu_popup(&sr).width(200.0).show(|ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for s in lizard_text::family_styles(&def.font_family) {
                    let mark = if def.font_style == s { "✓ " } else { "   " };
                    if crate::right::menu_item(ui, &format!("{mark}{s}"), "") {
                        set(out, &|d| d.set_font_style(s.clone()));
                        ui.close();
                    }
                }
            });
        });
    }
}

fn style_metadata(ui: &mut Ui, edit: &mut Editing, id_salt: &'static str, description_hint: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(16.0);
    let (name_row, _) = ui.allocate_exact_size(vec2(304.0, 24.0), Sense::hover());
    ui.painter().text(pos2(name_row.left() + 16.0, name_row.center().y), Align2::LEFT_CENTER, tr("Name"), regular(11.0), t.text_secondary);
    let mut name_ui =
        ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(name_row.left() + 112.0, name_row.top()), vec2(176.0, 24.0))));
    let name_response = widgets::text_input(&mut name_ui, Id::new((id_salt, "name", &edit.id)), &mut edit.name, 176.0, "");
    if edit.focus {
        name_response.request_focus();
        edit.focus = false;
    }
    ui.add_space(8.0);
    let (description_row, _) = ui.allocate_exact_size(vec2(304.0, 24.0), Sense::hover());
    ui.painter().text(
        pos2(description_row.left() + 16.0, description_row.center().y),
        Align2::LEFT_CENTER,
        tr("Description"),
        regular(11.0),
        t.text_secondary,
    );
    let mut description_ui = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(description_row.left() + 112.0, description_row.top()), vec2(176.0, 24.0))),
    );
    widgets::text_input(&mut description_ui, Id::new((id_salt, "description", &edit.id)), &mut edit.description, 176.0, description_hint);
}

/// The full-size text-style editor, shared by creation and editing.
fn text_style_editor(ui: &mut Ui, doc: &Document, p: &mut Picker, out: &mut Frame, header: Rect) {
    use crate::primitives::{ControlWidth, NumberField, Select, SelectVariant};

    let t = Tokens::get(ui.ctx());
    let Some(edit) = p.edit.as_mut() else { return };
    let Some(style) = doc.style(&edit.id) else {
        out.act = Some(Act::Back);
        return;
    };
    let StyleKind::Text { style: def } = &style.kind else {
        out.act = Some(Act::Back);
        return;
    };
    let title = if edit.creating { tr("Create text style") } else { tr("Edit text style") };
    ui.painter().text(pos2(header.left() + 16.0, header.center().y), Align2::LEFT_CENTER, title, semibold(11.0), t.text);
    let delete_rect = Rect::from_center_size(pos2(header.right() - 44.0, header.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Trash, tr("Delete style")).icon_size(14.0).show_at(ui, Id::new(("text_style_delete", &edit.id)), delete_rect).clicked() {
        out.act = Some(Act::Delete(edit.id.clone()));
    }
    let close_rect = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, Id::new("text_style_close"), close_rect).clicked() {
        out.act = Some(Act::Close);
    }
    ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));

    let (preview, _) = ui.allocate_exact_size(vec2(304.0, 120.0), Sense::hover());
    ui.painter().rect_filled(preview, CornerRadius::ZERO, if t.dark { Color32::from_rgb(47, 47, 47) } else { t.hover });
    let sample = "Rag 123";
    if let Some(tex) = crate::font_preview::texture(ui.ctx(), def, sample, &mut out.budget) {
        let width = (tex.size_vec2().x / ui.ctx().pixels_per_point()).min(preview.width() - 32.0);
        crate::font_preview::paint(
            ui.painter(),
            &tex,
            pos2(preview.center().x - width / 2.0, preview.center().y),
            preview.width() - 32.0,
            preview.height() - 12.0,
            t.text,
        );
    } else {
        ui.painter().text(preview.center(), Align2::CENTER_CENTER, sample, regular(def.font_size.clamp(6.0, 60.0) as f32), t.text);
    }
    ui.painter().hline(preview.x_range(), preview.bottom() - 0.5, Stroke::new(1.0, t.border));

    let set_with_scrub = |out: &mut Frame, f: &dyn Fn(&mut TextStyle), scrub: Option<bool>| {
        let mut updated = def.clone();
        f(&mut updated);
        if updated != *def || scrub == Some(true) {
            out.act = Some(Act::SetText(style.id.clone(), updated, scrub));
        }
    };
    let set = |out: &mut Frame, f: &dyn Fn(&mut TextStyle)| set_with_scrub(out, f, None);
    style_metadata(ui, edit, "text_style", "");
    ui.add_space(20.0);
    let (properties_row, _) = ui.allocate_exact_size(vec2(304.0, 24.0), Sense::hover());
    ui.painter().text(pos2(properties_row.left() + 16.0, properties_row.center().y), Align2::LEFT_CENTER, tr("Properties"), semibold(11.0), t.text);
    ui.add_space(10.0);
    let mut family_response = None;
    let mut style_response = None;
    let mut more_response = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            family_response = Some(Select::trigger_with_variant(ui, &def.font_family, 248.0, None, SelectVariant::Outlined));
        });
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            style_response = Some(Select::trigger_with_variant(ui, &def.font_style, 120.0, None, SelectVariant::Outlined));
            let size = NumberField::new(Id::new(("text_style_size", &edit.id)), widgets::Lead::None, Some(def.font_size), "")
                .options(&crate::typography::SIZES)
                .show(ui, ControlWidth::Fixed(120.0));
            match size.edit.and_then(|edit| edit.settled()) {
                Some(widgets::FieldEdit::Set(value)) => set(out, &|d| d.font_size = value.clamp(1.0, 5000.0)),
                Some(widgets::FieldEdit::Scrub(value, done)) => {
                    set_with_scrub(out, &|d| d.font_size = value.clamp(1.0, 5000.0), Some(done));
                }
                None => {}
            }
            if let Some(value) = size.selected_option {
                set(out, &|d| d.font_size = value);
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            if let Some(change) = crate::typography::style_line_height_field(ui, Id::new(("text_style_line_height", &edit.id)), def, 120.0) {
                let (value, scrub) = change.into_value_and_scrub();
                if let Some(line_height) = crate::typography::line_height_of(&value) {
                    set_with_scrub(out, &|d| d.line_height = line_height, scrub);
                }
            }
            if let Some(change) = crate::typography::style_letter_spacing_field(ui, Id::new(("text_style_letter_spacing", &edit.id)), def, 120.0) {
                let (value, scrub) = change.into_value_and_scrub();
                if let Some(letter_spacing) = crate::typography::letter_spacing_of(&value) {
                    set_with_scrub(out, &|d| d.letter_spacing = letter_spacing, scrub);
                }
            }
            more_response = Some(Button::icon(Icon::Dots, tr("More text options")).icon_size(14.0).show(ui));
        });
    });
    ui.add_space(16.0);
    if let Some(response) = family_response {
        let panel_id = Id::new("text_style_font_panel");
        if response.clicked() {
            crate::primitives::FloatingPanel::dismiss(ui.ctx(), Id::new("text_style_settings_panel"));
        }
        if crate::primitives::FloatingPanel::toggle(ui.ctx(), panel_id, response.clicked()) {
            let ctx = ui.ctx().clone();
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                crate::primitives::FloatingPanel::dismiss(&ctx, panel_id);
                ctx.data_mut(|data| data.insert_temp(Id::new("text_style_font_panel_interacted"), true));
                return;
            }
            let initial = pos2(header.left() - 248.0, header.top());
            let (picked, mut close) = crate::typography::font_family_panel(&ctx, panel_id, initial, Some(&def.font_family), response.clicked());
            if let Some(family) = picked.picked {
                set(out, &|d| {
                    d.font_family = family.clone();
                    let styles = lizard_text::family_styles(&family);
                    if !styles.is_empty() && !styles.contains(&d.font_style) {
                        d.set_font_style(styles.iter().find(|s| s.eq_ignore_ascii_case("Regular")).or(styles.first()).cloned().unwrap_or_default());
                    }
                });
                crate::typography::remember_family(&ctx, &family);
                close = true;
            }
            if close {
                crate::primitives::FloatingPanel::dismiss(&ctx, panel_id);
                ctx.data_mut(|data| data.insert_temp(Id::new("text_style_font_panel_interacted"), true));
            }
        }
    }
    if let Some(response) = more_response {
        let panel_id = Id::new("text_style_settings_panel");
        if response.clicked() {
            crate::primitives::FloatingPanel::dismiss(ui.ctx(), Id::new("text_style_font_panel"));
        }
        if crate::primitives::FloatingPanel::toggle(ui.ctx(), panel_id, response.clicked()) {
            let ctx = ui.ctx().clone();
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                crate::primitives::FloatingPanel::dismiss(&ctx, panel_id);
                ctx.data_mut(|data| data.insert_temp(Id::new("text_style_subpanel_interacted"), true));
                return;
            }
            let initial = pos2(header.left() - 248.0, header.top());
            let variable = lizard_text::find_face(&def.font_family, &def.font_style).is_some_and(|face| face.is_variable());
            let close = crate::typography::type_settings_panel(&ctx, panel_id, Id::new("text_style_settings_tab"), initial, variable, |ui, tab| {
                if let Some((updated, scrub)) = crate::typography::text_style_settings(ui, def, tab, &mut out.budget) {
                    out.act = Some(Act::SetText(style.id.clone(), updated, scrub));
                }
            });
            if close {
                crate::primitives::FloatingPanel::dismiss(&ctx, panel_id);
                ctx.data_mut(|data| data.insert_temp(Id::new("text_style_subpanel_interacted"), true));
            }
        }
    }
    if let Some(response) = style_response {
        crate::right::menu_popup(&response).width(200.0).show(|ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for font_style in lizard_text::family_styles(&def.font_family) {
                    let mark = if def.font_style == font_style { "✓ " } else { "   " };
                    if crate::right::menu_item(ui, &format!("{mark}{font_style}"), "") {
                        set(out, &|d| d.set_font_style(font_style.clone()));
                        ui.close();
                    }
                }
            });
        });
    }
}

pub(crate) fn style_paint_preview(ui: &Ui, rect: Rect, paints: &[Paint]) {
    let t = Tokens::get(ui.ctx());
    widgets::checker(ui.painter(), rect, 12.0);
    for paint in paints.iter().filter(|paint| paint.visible) {
        match &paint.kind {
            PaintKind::Solid { color } => {
                let color = color.with_alpha(color.a * paint.opacity);
                ui.painter().rect_filled(rect, CornerRadius::ZERO, crate::theme::doc_c32(ui.ctx(), color));
            }
            PaintKind::Gradient(gradient) => {
                for column in 0..64 {
                    let left = rect.left() + rect.width() * column as f32 / 64.0;
                    let right = rect.left() + rect.width() * (column + 1) as f32 / 64.0;
                    let color = gradient.sample((column as f32 + 0.5) / 64.0);
                    ui.painter().rect_filled(
                        Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
                        CornerRadius::ZERO,
                        crate::theme::doc_c32(ui.ctx(), color.with_alpha(color.a * paint.opacity)),
                    );
                }
            }
            PaintKind::Image(_) | PaintKind::Pattern(_) | PaintKind::Shader(_) => widgets::paint_swatch(ui.painter(), rect, paint),
        }
    }
    ui.painter().rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
}

fn update_style_paints(out: &mut Frame, id: &str, paints: &[Paint], change: impl FnOnce(&mut Vec<Paint>), scrub: Option<bool>) {
    let mut updated = paints.to_vec();
    change(&mut updated);
    if updated != paints || scrub == Some(true) {
        out.act = Some(Act::SetPaints(id.to_string(), updated, scrub));
    }
}

/// A colour style uses the same colour, opacity, visibility and removal controls as Fill.
fn color_style_editor(ui: &mut Ui, doc: &Document, p: &mut Picker, out: &mut Frame, header: Rect) {
    use crate::primitives::{ColorControl, ColorValue};

    let t = Tokens::get(ui.ctx());
    let Some(edit) = p.edit.as_mut() else { return };
    let Some(style) = doc.style(&edit.id) else {
        out.act = Some(Act::Back);
        return;
    };
    let StyleKind::Paint { paints } = &style.kind else {
        out.act = Some(Act::Back);
        return;
    };
    let title = if edit.creating { tr("Create color style") } else { tr("Edit color style") };
    ui.painter().text(pos2(header.left() + 16.0, header.center().y), Align2::LEFT_CENTER, title, semibold(11.0), t.text);
    let close_rect = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, Id::new("color_style_close"), close_rect).clicked() {
        out.act = Some(Act::Close);
    }
    ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));

    let (preview_section, _) = ui.allocate_exact_size(vec2(304.0, 120.0), Sense::hover());
    let preview = Rect::from_min_size(preview_section.min + vec2(16.0, 16.0), vec2(272.0, 104.0));
    style_paint_preview(ui, preview, paints);
    style_metadata(ui, edit, "color_style", tr("What is it for?"));
    ui.add_space(24.0);

    let (properties, _) = ui.allocate_exact_size(vec2(304.0, 32.0), Sense::hover());
    ui.painter().text(pos2(properties.left() + 16.0, properties.center().y), Align2::LEFT_CENTER, tr("Properties"), semibold(11.0), t.text);
    let add_rect = Rect::from_center_size(pos2(properties.right() - 20.0, properties.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Plus, tr("Add fill")).icon_size(14.0).show_at(ui, Id::new(("color_style_add", &style.id)), add_rect).clicked() {
        update_style_paints(
            out,
            &style.id,
            paints,
            |updated| {
                updated.push(Paint::solid(lizard_color::Color::from_rgb8_const(0xD9, 0xD9, 0xD9)));
            },
            None,
        );
    }
    ui.add_space(8.0);
    egui::ScrollArea::vertical().id_salt(("color_style_paints", &style.id)).max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.set_width(304.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        for (index, paint) in paints.iter().enumerate().rev() {
            let (row, _) = ui.allocate_exact_size(vec2(304.0, 32.0), Sense::hover());
            let field = Rect::from_min_size(pos2(row.left() + 16.0, row.top() + 4.0), vec2(220.0, 24.0));
            let hex = match &paint.kind {
                PaintKind::Solid { color } => color.to_hex(),
                _ => String::new(),
            };
            let label = paint.label();
            let binding =
                style.bound.get(&format!("fills/{index}/color")).and_then(|id| doc.variables.get(id)).map(|variable| variable.name.as_str());
            let value = match (&paint.kind, binding) {
                (PaintKind::Solid { .. }, Some(name)) => ColorValue::BoundVariable(name),
                (PaintKind::Solid { .. }, None) => ColorValue::EditableHex(&hex),
                _ => ColorValue::Label(&label),
            };
            let change = ColorControl::new(Id::new(("color_style_paint", &style.id, index)), paint, value, f64::from(paint.opacity) * 100.0)
                .show_at(ui, field);
            if change.open_picker {
                out.act = Some(Act::EditPaint(style.id.clone(), index));
            }
            if change.detach_variable {
                out.act = Some(Act::Unbind(style.id.clone(), format!("fills/{index}/color")));
            }
            if let Some(hex) = change.edited_hex
                && let Some(color) = lizard_color::Color::from_hex_or_name(&hex)
            {
                update_style_paints(
                    out,
                    &style.id,
                    paints,
                    |updated| {
                        if let Some(paint) = updated.get_mut(index) {
                            paint.kind = PaintKind::Solid { color: color.with_alpha(1.0) };
                        }
                    },
                    None,
                );
            }
            match change.opacity_edit {
                Some(widgets::FieldEdit::Set(value)) => update_style_paints(
                    out,
                    &style.id,
                    paints,
                    |updated| {
                        if let Some(paint) = updated.get_mut(index) {
                            paint.opacity = (value / 100.0).clamp(0.0, 1.0) as f32;
                        }
                    },
                    None,
                ),
                Some(widgets::FieldEdit::Scrub(value, done)) => update_style_paints(
                    out,
                    &style.id,
                    paints,
                    |updated| {
                        if let Some(paint) = updated.get_mut(index) {
                            paint.opacity = (value / 100.0).clamp(0.0, 1.0) as f32;
                        }
                    },
                    Some(done),
                ),
                None => {}
            }
            let eye = Rect::from_min_size(pos2(field.right() + 8.0, field.top()), vec2(24.0, 24.0));
            let eye_icon = if paint.visible { Icon::Eye } else { Icon::EyeOff };
            if Button::icon(eye_icon, tr("Toggle visibility"))
                .icon_size(14.0)
                .show_at(ui, Id::new(("color_style_visible", &style.id, index)), eye)
                .clicked()
            {
                update_style_paints(
                    out,
                    &style.id,
                    paints,
                    |updated| {
                        if let Some(paint) = updated.get_mut(index) {
                            paint.visible = !paint.visible;
                        }
                    },
                    None,
                );
            }
            let remove = Rect::from_min_size(pos2(row.right() - 32.0, field.top()), vec2(24.0, 24.0));
            if Button::icon(Icon::Minus, tr("Remove"))
                .icon_size(14.0)
                .show_at(ui, Id::new(("color_style_remove", &style.id, index)), remove)
                .clicked()
            {
                update_style_paints(
                    out,
                    &style.id,
                    paints,
                    |updated| {
                        if index < updated.len() {
                            updated.remove(index);
                        }
                    },
                    None,
                );
            }
        }
    });
    ui.add_space(8.0);
}

fn update_style_effects(out: &mut Frame, id: &str, effects: &[Effect], change: impl FnOnce(&mut Vec<Effect>), scrub: Option<bool>) {
    let mut updated = effects.to_vec();
    change(&mut updated);
    if updated != effects || scrub == Some(true) {
        out.act = Some(Act::SetEffects(id.to_string(), updated, scrub));
    }
}

/// Render the sample through the same renderer used by the canvas, including stacked effects.
fn effect_style_sample(effects: &[Effect]) -> Option<lizard_render::Pixmap> {
    let mut sample = Document::new("Effect preview");
    let page = sample.current_page?;
    let image =
        sample.create(page, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Preview", 96.0, 20.0, 112.0, 112.0).ok()?;
    let node = sample.node_mut(image)?;
    node.fills = vec![Paint::solid(lizard_color::Color::WHITE)];
    node.effects = effects.to_vec();
    lizard_engine::cmd::file::render_export_area(&sample, &[image], lizard_geom::Rect::new(0.0, 0.0, 304.0, 152.0), 2.0)
}

fn effect_style_preview(ui: &mut Ui, rect: Rect, effects: &[Effect]) {
    widgets::checker(ui.painter(), rect, 8.0);
    let cache_id = Id::new("effect_style_preview_texture");
    let cached = ui.ctx().data(|data| data.get_temp::<(Vec<Effect>, egui::TextureHandle)>(cache_id));
    let texture = if let Some((_, texture)) = cached.filter(|(previous, _)| previous == effects) {
        Some(texture)
    } else {
        effect_style_sample(effects).map(|pixmap| {
            let image = crate::render_worker::screen_image(&pixmap, lizard_color::ColorProfile::Srgb);
            let texture = ui.ctx().load_texture("effect-style-preview", image, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|data| data.insert_temp(cache_id, (effects.to_vec(), texture.clone())));
            texture
        })
    };
    if let Some(texture) = texture {
        ui.painter().image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

fn effect_style_editor(ui: &mut Ui, doc: &Document, p: &mut Picker, out: &mut Frame, header: Rect) {
    use crate::primitives::{Menu, MenuItem};

    let t = Tokens::get(ui.ctx());
    let Some(edit) = p.edit.as_mut() else { return };
    let Some(style) = doc.style(&edit.id) else {
        out.act = Some(Act::Back);
        return;
    };
    let StyleKind::Effect { effects } = &style.kind else {
        out.act = Some(Act::Back);
        return;
    };
    let title = if edit.creating { tr("Create effect style") } else { tr("Edit effect style") };
    ui.painter().text(pos2(header.left() + 16.0, header.center().y), Align2::LEFT_CENTER, title, semibold(11.0), t.text);
    let close_rect = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, Id::new("effect_style_close"), close_rect).clicked() {
        out.act = Some(Act::Close);
    }
    ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));
    let (preview, _) = ui.allocate_exact_size(vec2(304.0, 152.0), Sense::hover());
    effect_style_preview(ui, preview, effects);
    ui.painter().hline(preview.x_range(), preview.bottom() - 0.5, Stroke::new(1.0, t.border));
    style_metadata(ui, edit, "effect_style", tr("What is it for?"));
    ui.add_space(24.0);

    let (properties, _) = ui.allocate_exact_size(vec2(304.0, 32.0), Sense::hover());
    ui.painter().text(pos2(properties.left() + 16.0, properties.center().y), Align2::LEFT_CENTER, tr("Properties"), semibold(11.0), t.text);
    let add_rect = Rect::from_center_size(pos2(properties.right() - 20.0, properties.center().y), vec2(24.0, 24.0));
    let add = Button::icon(Icon::Plus, tr("Add effect")).icon_size(14.0).show_at(ui, Id::new(("effect_style_add", &style.id)), add_rect);
    Menu::popup(&add).show(|ui| {
        for (label, kind) in Effect::TYPES {
            if MenuItem::new(tr(label)).show(ui).clicked() {
                if let Some(effect) = Effect::of_type(kind) {
                    update_style_effects(out, &style.id, effects, |updated| updated.push(effect), None);
                }
                ui.close();
            }
        }
    });
    ui.add_space(8.0);
    let open_id = Id::new(("effect_style_open", &style.id));
    let mut open = ui.ctx().data(|data| data.get_temp::<usize>(open_id));
    egui::ScrollArea::vertical().id_salt(("effect_style_effects", &style.id)).max_height(192.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.set_width(304.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut drag = crate::list_drag::ListDrag::new(ui, ("effect_style", &style.id));
        for (index, effect) in effects.iter().enumerate().rev() {
            let row = crate::right::effect_row(ui, Id::new(("effect_style_row", &style.id, index)), effect, open == Some(index));
            if row.settings {
                open = if open == Some(index) { None } else { Some(index) };
            }
            if row.visibility {
                update_style_effects(
                    out,
                    &style.id,
                    effects,
                    |updated| {
                        if let Some(effect) = updated.get_mut(index) {
                            effect.visible = !effect.visible;
                        }
                    },
                    None,
                );
            }
            if row.remove {
                update_style_effects(
                    out,
                    &style.id,
                    effects,
                    |updated| {
                        if index < updated.len() {
                            updated.remove(index);
                        }
                    },
                    None,
                );
                open = open.and_then(|current| lizard_engine::cmd::lists::removed_index(current, index));
            }
            drag.row(ui, index, row.rect, row.rect, || tr(effect.label()).to_string());
        }
        if let Some((from, to)) = drag.finish(ui) {
            update_style_effects(
                out,
                &style.id,
                effects,
                |updated| {
                    if from < updated.len() {
                        let effect = updated.remove(from);
                        updated.insert(to.min(updated.len()), effect);
                    }
                },
                None,
            );
            open = open.map(|index| lizard_engine::cmd::lists::moved_index(index, from, to));
        }
    });
    ui.add_space(8.0);
    ui.ctx().data_mut(|data| {
        if let Some(index) = open {
            data.insert_temp(open_id, index);
        } else {
            data.remove::<usize>(open_id);
        }
    });
}

fn update_style_grids(out: &mut Frame, id: &str, grids: &[LayoutGrid], change: impl FnOnce(&mut Vec<LayoutGrid>), scrub: Option<bool>) {
    let mut updated = grids.to_vec();
    change(&mut updated);
    if updated != grids || scrub == Some(true) {
        out.act = Some(Act::SetGrids(id.to_string(), updated, scrub));
    }
}

/// A miniature guide on transparency. The type and density follow the saved grid.
fn grid_style_preview(ui: &mut Ui, rect: Rect, grids: &[LayoutGrid]) {
    widgets::checker(ui.painter(), rect, 8.0);
    if let Some(grid) = grids.iter().find(|grid| grid.visible) {
        let tile = Rect::from_center_size(rect.center(), vec2(46.0, 46.0));
        ui.painter().rect_stroke(tile.translate(vec2(4.0, 4.0)), CornerRadius::ZERO, Stroke::new(1.5, Color32::BLACK), egui::StrokeKind::Inside);
        ui.painter().rect_filled(tile, CornerRadius::ZERO, Color32::WHITE);
        ui.painter().rect_stroke(tile, CornerRadius::ZERO, Stroke::new(1.5, Color32::BLACK), egui::StrokeKind::Inside);
        let inner = tile.shrink(7.0);
        match grid.kind {
            GridKind::Grid => {
                let count = (36.0 / grid.size.max(1.0)).round().clamp(2.0, 6.0) as usize;
                let pitch = inner.width() / count as f32;
                for y in 0..count {
                    for x in 0..count {
                        let center = pos2(inner.left() + pitch * (x as f32 + 0.5), inner.top() + pitch * (y as f32 + 0.5));
                        ui.painter().rect_filled(
                            Rect::from_center_size(center, vec2((pitch * 0.5).max(2.0), (pitch * 0.5).max(2.0))),
                            CornerRadius::ZERO,
                            Color32::BLACK,
                        );
                    }
                }
            }
            GridKind::Columns | GridKind::Rows => {
                let count = grid.count.clamp(2, 6) as usize;
                let horizontal = grid.kind == GridKind::Rows;
                for index in 0..count {
                    let pitch = if horizontal { inner.height() } else { inner.width() } / count as f32;
                    let band = if horizontal {
                        Rect::from_min_size(pos2(inner.left(), inner.top() + pitch * index as f32), vec2(inner.width(), (pitch * 0.5).max(2.0)))
                    } else {
                        Rect::from_min_size(pos2(inner.left() + pitch * index as f32, inner.top()), vec2((pitch * 0.5).max(2.0), inner.height()))
                    };
                    ui.painter().rect_filled(band, CornerRadius::ZERO, Color32::BLACK);
                }
            }
        }
    }
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, Tokens::get(ui.ctx()).border));
}

fn grid_style_editor(ui: &mut Ui, doc: &Document, p: &mut Picker, out: &mut Frame, header: Rect) {
    use crate::primitives::{Menu, MenuItem};

    let t = Tokens::get(ui.ctx());
    let Some(edit) = p.edit.as_mut() else { return };
    let Some(style) = doc.style(&edit.id) else {
        out.act = Some(Act::Back);
        return;
    };
    let StyleKind::Grid { grids } = &style.kind else {
        out.act = Some(Act::Back);
        return;
    };
    let title = if edit.creating { tr("Create layout grid style") } else { tr("Edit layout grid style") };
    ui.painter().text(pos2(header.left() + 16.0, header.center().y), Align2::LEFT_CENTER, title, semibold(11.0), t.text);
    let close = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
    if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, Id::new("grid_style_close"), close).clicked() {
        out.act = Some(Act::Close);
    }
    ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));
    let (preview, _) = ui.allocate_exact_size(vec2(304.0, 152.0), Sense::hover());
    grid_style_preview(ui, preview, grids);
    style_metadata(ui, edit, "grid_style", tr("What is it for?"));
    ui.add_space(24.0);
    let (properties, _) = ui.allocate_exact_size(vec2(304.0, 32.0), Sense::hover());
    ui.painter().text(pos2(properties.left() + 16.0, properties.center().y), Align2::LEFT_CENTER, tr("Properties"), semibold(11.0), t.text);
    let add_rect = Rect::from_center_size(pos2(properties.right() - 20.0, properties.center().y), vec2(24.0, 24.0));
    let add = Button::icon(Icon::Plus, tr("Add layout grid")).icon_size(14.0).show_at(ui, Id::new(("grid_style_add", &style.id)), add_rect);
    Menu::popup(&add).show(|ui| {
        for (label, grid) in [(tr("Grid"), LayoutGrid::default()), (tr("Columns"), LayoutGrid::columns()), (tr("Rows"), LayoutGrid::rows())] {
            if MenuItem::new(label).show(ui).clicked() {
                update_style_grids(out, &style.id, grids, |updated| updated.push(grid), None);
                ui.close();
            }
        }
    });
    ui.add_space(8.0);
    let open_id = Id::new(("grid_style_open", &style.id));
    let mut open = ui.ctx().data(|data| data.get_temp::<usize>(open_id));
    egui::ScrollArea::vertical().id_salt(("grid_style_grids", &style.id)).max_height(192.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.set_width(304.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut drag = crate::list_drag::ListDrag::new(ui, ("grid_style", &style.id));
        for (index, grid) in grids.iter().enumerate().rev() {
            let row = crate::right::grid_row(ui, grid, open == Some(index));
            if row.settings {
                open = if open == Some(index) { None } else { Some(index) };
            }
            if let Some(kind) = row.kind {
                let replacement = match kind {
                    GridKind::Grid => LayoutGrid::default(),
                    GridKind::Columns => LayoutGrid::columns(),
                    GridKind::Rows => LayoutGrid::rows(),
                };
                update_style_grids(
                    out,
                    &style.id,
                    grids,
                    |updated| {
                        if let Some(grid) = updated.get_mut(index) {
                            *grid = replacement;
                        }
                    },
                    None,
                );
            }
            if row.visibility {
                update_style_grids(
                    out,
                    &style.id,
                    grids,
                    |updated| {
                        if let Some(grid) = updated.get_mut(index) {
                            grid.visible = !grid.visible;
                        }
                    },
                    None,
                );
            }
            if row.remove {
                update_style_grids(
                    out,
                    &style.id,
                    grids,
                    |updated| {
                        if index < updated.len() {
                            updated.remove(index);
                        }
                    },
                    None,
                );
                open = open.and_then(|current| lizard_engine::cmd::lists::removed_index(current, index));
            }
            drag.row(ui, index, row.rect, row.rect, || crate::right::grid_label(grid));
        }
        if let Some((from, to)) = drag.finish(ui) {
            update_style_grids(
                out,
                &style.id,
                grids,
                |updated| {
                    if from < updated.len() {
                        let grid = updated.remove(from);
                        updated.insert(to.min(updated.len()), grid);
                    }
                },
                None,
            );
            open = open.map(|index| lizard_engine::cmd::lists::moved_index(index, from, to));
        }
    });
    ui.add_space(8.0);
    ui.ctx().data_mut(|data| {
        if let Some(index) = open {
            data.insert_temp(open_id, index);
        } else {
            data.remove::<usize>(open_id);
        }
    });
}

/// Editing one style: name, description, its values (paints open the colour picker; text styles
/// have their typography), update from the selection, delete.
fn editor(ui: &mut Ui, doc: &Document, p: &mut Picker, out: &mut Frame) {
    let t = Tokens::get(ui.ctx());
    let slot = p.slot;
    let has_selection = !p.ids.is_empty();
    let Some(e) = p.edit.as_mut() else { return };
    let Some(style) = doc.style(&e.id).cloned() else {
        out.act = Some(Act::Back);
        return;
    };
    let (hr, _) = ui.allocate_exact_size(vec2(W, 24.0), Sense::hover());
    if menu_icon(ui, Rect::from_min_size(hr.min, vec2(24.0, 24.0)), Icon::ChevronLeft, tr("Back to styles"), false).clicked() {
        out.act = Some(Act::Back);
    }
    ui.painter().text(pos2(hr.min.x + 30.0, hr.center().y), Align2::LEFT_CENTER, tr("Edit style"), semibold(11.0), t.menu_text);
    ui.add_space(6.0);
    caption(ui, tr("Name"));
    let r = ui.add(egui::TextEdit::singleline(&mut e.name).font(regular(11.0)).desired_width(W));
    if e.focus {
        r.request_focus();
        let mut state = egui::TextEdit::load_state(ui.ctx(), r.id).unwrap_or_default();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(e.name.chars().count()));
        state.cursor.set_char_range(Some(all));
        egui::TextEdit::store_state(ui.ctx(), r.id, state);
        e.focus = false;
    }
    crate::widgets::select_all_on_focus(ui, &r, &e.name);
    ui.add_space(6.0);
    caption(ui, tr("Description"));
    ui.add(egui::TextEdit::multiline(&mut e.description).font(regular(11.0)).desired_width(W).desired_rows(2));
    ui.add_space(8.0);
    caption(ui, tr("Properties"));
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        match &style.kind {
            StyleKind::Paint { paints } if paints.is_empty() => caption(ui, tr("No paints")),
            StyleKind::Paint { paints } => {
                // Top of the list paints on top: show in reverse like the Fill section.
                for (i, paint) in paints.iter().enumerate().rev() {
                    let label = format!("{}  {:.0}%", tr(&paint.label()), paint.opacity * 100.0);
                    let clicked = property_row(ui, &label, Some(paint), slot);
                    // A solid paint's colour takes a variable.
                    let on_binding = matches!(paint.kind, PaintKind::Solid { .. })
                        && binding_cell(ui, doc, &style, &format!("fills/{i}/color"), lizard_doc::VariableType::Color, out);
                    if clicked && !on_binding {
                        out.act = Some(Act::EditPaint(style.id.clone(), i));
                    }
                }
            }
            StyleKind::Effect { effects } => {
                for (i, fx) in effects.iter().enumerate().rev() {
                    property_row(ui, tr(fx.label()), None, slot);
                    if let Some((field, kind)) = effect_binding(fx, i) {
                        binding_cell(ui, doc, &style, &field, kind, out);
                    }
                }
            }
            StyleKind::Grid { grids } => {
                for (i, g) in grids.iter().enumerate() {
                    property_row(ui, &grid_label(g), None, slot);
                    binding_cell(ui, doc, &style, &format!("layoutGrids/{i}/size"), lizard_doc::VariableType::Number, out);
                }
            }
            StyleKind::Text { style: def } => text_properties(ui, &style.id, def, out),
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if has_selection && menu_button(ui, tr("Update from selection")).on_hover_text(tr("Take the values from the first selected layer")).clicked()
        {
            out.act = Some(Act::UpdateFromSelection(style.id.clone()));
        }
        if menu_button(ui, tr("Delete")).on_hover_text(tr("Delete the style (layers keep their values)")).clicked() {
            out.act = Some(Act::Delete(style.id.clone()));
        }
    });
}

/// Write the typed name and description of the style being edited, when they changed.
fn commit_edit(app: &mut LizardApp, doc: &Document, e: &Editing) {
    let Some(s) = doc.style(&e.id) else { return };
    let name = e.name.trim();
    let mut q = json!({"style": e.id});
    if !name.is_empty() && name != s.name {
        q["name"] = json!(name);
    }
    if e.description != s.description {
        q["description"] = json!(e.description);
    }
    if q.as_object().is_some_and(|o| o.len() > 1) {
        app.run("style.update", q);
    }
}

/// The picker popover (call once per frame).
pub fn show(app: &mut LizardApp, ctx: &egui::Context) {
    let Some(mut p) = app.ui.style_picker.take() else { return };
    let Ok(doc) = app.session.ds().map(|d| d.text_preview.clone().unwrap_or_else(|| d.doc.clone())) else { return };
    // A new selection closes the picker (it acts on the layers it was opened for).
    if app.session.selection() != p.ids {
        if let Some(e) = &p.edit {
            commit_edit(app, &doc, e);
        }
        return;
    }
    let screen = ctx.content_rect();
    // Left of the Design panel. The style editors stay put while their colour picker opens beside them.
    let text_editor = p.slot == StyleSlot::Text && p.edit.is_some();
    let color_editor = p.slot.paints() && p.edit.is_some();
    let effect_editor = p.slot == StyleSlot::Effect && p.edit.is_some();
    let grid_editor = p.slot == StyleSlot::Grid && p.edit.is_some();
    let panel_width = if text_editor || color_editor || effect_editor || grid_editor {
        304.0
    } else if p.edit.is_none() {
        W
    } else {
        W + 16.0
    };
    let mut x = screen.max.x - app.ui.right_width - panel_width - 16.0;
    if app.ui.picker.is_some() && !text_editor && !color_editor && !effect_editor && !grid_editor {
        x -= 260.0;
    }
    let y = p.y.clamp(screen.min.y + 48.0, (screen.max.y - 460.0).max(screen.min.y + 48.0));
    let mut out = Frame { act: None, hover: None, budget: 4 };
    let area = if text_editor {
        crate::primitives::FloatingPanel::new(Id::new("text_style_editor_area"), pos2(x, y))
            .width(crate::primitives::FloatingPanelWidth::Regular)
            .height(crate::primitives::FloatingPanelHeight::Fixed(390.0))
            .corner_radius(12)
            .header_height(40.0)
            .show_with_header(ctx, |ui, header| {
                ui.spacing_mut().item_spacing.y = 0.0;
                text_style_editor(ui, &doc, &mut p, &mut out, header);
            })
    } else if color_editor {
        crate::primitives::FloatingPanel::new(Id::new("color_style_editor_area"), pos2(x, y))
            .width(crate::primitives::FloatingPanelWidth::Regular)
            .corner_radius(12)
            .header_height(40.0)
            .show_with_header(ctx, |ui, header| {
                ui.spacing_mut().item_spacing.y = 0.0;
                color_style_editor(ui, &doc, &mut p, &mut out, header);
            })
    } else if effect_editor {
        crate::primitives::FloatingPanel::new(Id::new("effect_style_editor_area"), pos2(x, y))
            .width(crate::primitives::FloatingPanelWidth::Regular)
            .corner_radius(12)
            .header_height(40.0)
            .show_with_header(ctx, |ui, header| {
                ui.spacing_mut().item_spacing.y = 0.0;
                effect_style_editor(ui, &doc, &mut p, &mut out, header);
            })
    } else if grid_editor {
        crate::primitives::FloatingPanel::new(Id::new("grid_style_editor_area"), pos2(x, y))
            .width(crate::primitives::FloatingPanelWidth::Regular)
            .corner_radius(12)
            .header_height(40.0)
            .show_with_header(ctx, |ui, header| {
                ui.spacing_mut().item_spacing.y = 0.0;
                grid_style_editor(ui, &doc, &mut p, &mut out, header);
            })
    } else if p.edit.is_none() {
        let height = if p.slot.paints() { 427.0 } else { 380.0 };
        crate::primitives::FloatingPanel::new(Id::new("style_picker_area"), pos2(x, y))
            .width(crate::primitives::FloatingPanelWidth::Compact)
            .height(crate::primitives::FloatingPanelHeight::Fixed(height))
            .corner_radius(12)
            .header_height(40.0)
            .show_with_header(ctx, |ui, header| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                listing(app, ui, &doc, &mut p, &mut out, header);
            })
    } else {
        crate::primitives::FloatingPanel::new(Id::new("style_picker_area"), pos2(x, y))
            .width(crate::primitives::FloatingPanelWidth::Fixed(W + 16.0))
            .corner_radius(10)
            .header_height(32.0)
            .show(ctx, |ui| {
                egui::Frame::NONE.inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_width(W);
                    editor(ui, &doc, &mut p, &mut out);
                });
            })
    };
    let rect = area.as_ref().map_or(Rect::NOTHING, |result| result.response.rect);
    if effect_editor
        && let Some(edit) = &p.edit
        && let Some(style) = doc.style(&edit.id)
        && let StyleKind::Effect { effects } = &style.kind
    {
        let open_id = Id::new(("effect_style_open", &style.id));
        if let Some(index) = ctx.data(|data| data.get_temp::<usize>(open_id))
            && let Some(effect) = effects.get(index)
        {
            crate::right::effect_style_popup(app, ctx, &style.id, effects, index, effect, pos2(rect.left() - 248.0, rect.top()));
        }
    }
    if grid_editor
        && let Some(edit) = &p.edit
        && let Some(style) = doc.style(&edit.id)
        && let StyleKind::Grid { grids } = &style.kind
    {
        let open_id = Id::new(("grid_style_open", &style.id));
        if let Some(index) = ctx.data(|data| data.get_temp::<usize>(open_id))
            && let Some(grid) = grids.get(index)
        {
            crate::right::grid_style_popup(app, ctx, &style.id, grids, index, grid, pos2(rect.left() - 248.0, rect.top()));
        }
    }
    // Clicks in the colour picker editing this picker's style, or in a menu the editor opened,
    // keep it open.
    let picker_rect = app
        .ui
        .picker
        .as_ref()
        .filter(|t| matches!(t, PickerTarget::Style { .. } | PickerTarget::StyleEffect { .. }))
        .and(ctx.memory(|m| m.area_rect(Id::new("color_picker"))));
    let effect_settings_rect =
        ctx.memory(|memory| memory.area_rect(Id::new(("effect_style_settings", p.edit.as_ref().map(|edit| edit.id.as_str()).unwrap_or("")))));
    let grid_settings_rect =
        ctx.memory(|memory| memory.area_rect(Id::new(("grid_style_settings", p.edit.as_ref().map(|edit| edit.id.as_str()).unwrap_or("")))));
    // So do clicks in the variable picker binding one of its fields (Escape closes that first).
    let variables = crate::variable_ui::picker_rect(ctx);
    let font_panel_id = Id::new("text_style_font_panel");
    let font_panel_open = crate::primitives::FloatingPanel::toggle(ctx, font_panel_id, false);
    let font_panel_rect = font_panel_open.then(|| ctx.memory(|m| m.area_rect(font_panel_id))).flatten();
    let font_panel_interacted = ctx.data_mut(|data| data.remove_temp::<bool>(Id::new("text_style_font_panel_interacted"))).unwrap_or(false);
    let settings_id = Id::new("text_style_settings_panel");
    let settings_open = crate::primitives::FloatingPanel::toggle(ctx, settings_id, false);
    let settings_rect = settings_open.then(|| ctx.memory(|m| m.area_rect(settings_id))).flatten();
    let settings_interacted = ctx.data_mut(|data| data.remove_temp::<bool>(Id::new("text_style_subpanel_interacted"))).unwrap_or(false);
    let in_popup =
        egui::Popup::is_any_open(ctx) || variables.is_some() || font_panel_open || font_panel_interacted || settings_open || settings_interacted;
    let just_opened = ctx.data_mut(|d| d.remove_temp::<bool>(Id::new("style_picker_just_opened"))).unwrap_or(false);
    let outside = !just_opened
        && ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|pos| {
                    !rect.contains(pos)
                        && !picker_rect.is_some_and(|r| r.contains(pos))
                        && !font_panel_rect.is_some_and(|r| r.contains(pos))
                        && !settings_rect.is_some_and(|r| r.contains(pos))
                        && !effect_settings_rect.is_some_and(|r| r.contains(pos))
                        && !grid_settings_rect.is_some_and(|r| r.contains(pos))
                })
        })
        && !in_popup;
    let mut close = outside || (ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !in_popup);
    let ids = p.ids.clone();
    let slot = p.slot;
    // The canvas shows the text style under the pointer on the selected text.
    if slot == StyleSlot::Text
        && !close
        && out.act.is_none()
        && let Some(key) = &out.hover
    {
        let (cmd, params) = key.apply(&ids, slot);
        app.preview_text(ctx, cmd, params);
    }
    match out.act {
        Some(Act::Apply(key)) => {
            let (cmd, params) = key.apply(&ids, slot);
            app.run(cmd, params);
            close = true;
        }
        Some(Act::UseCustomColor) => {
            app.run("style.detach", json!({"ids": ids, "which": slot.key()}));
            close = true;
        }
        Some(Act::Bind(pick)) => {
            crate::variable_ui::bind(app, &ids, slot.key(), &pick);
            close = true;
        }
        Some(Act::Create) => {
            let name = next_name(&doc, slot.default_name());
            let made = app.run("style.create", json!({"type": slot.create_type(), "name": name, "from": ids.first()}));
            if let Some(id) = made.as_ref().and_then(|v| v["id"].as_str()) {
                p.query.clear();
                p.edit = Some(Editing { id: id.to_string(), name, description: String::new(), focus: true, creating: true });
            }
        }
        Some(Act::Edit(id)) => p.edit = doc.style(&id).map(editing),
        Some(Act::Back) => {
            if let Some(e) = p.edit.take() {
                commit_edit(app, &doc, &e);
            }
        }
        Some(Act::Close) => close = true,
        Some(Act::EditPaint(id, index)) => {
            if let Some(e) = &p.edit {
                commit_edit(app, &doc, e);
            }
            app.ui.picker = Some(PickerTarget::Style { id, index });
        }
        Some(Act::Unbind(id, field)) => {
            app.run("style.bind", json!({"style": id, "field": field}));
        }
        Some(Act::SetText(id, def, scrub)) => {
            let params = json!({"style": id, "value": def});
            if let Some(done) = scrub {
                crate::typography::live(app, tr("Edit text style"), "style.update", params, done);
            } else {
                app.run("style.update", params);
            }
        }
        Some(Act::SetPaints(id, paints, scrub)) => {
            let params = json!({"style": id, "value": paints});
            if let Some(done) = scrub {
                crate::typography::live(app, tr("Edit color style"), "style.update", params, done);
            } else {
                app.run("style.update", params);
            }
        }
        Some(Act::SetEffects(id, effects, scrub)) => {
            let params = json!({"style": id, "value": effects});
            if let Some(done) = scrub {
                crate::typography::live(app, tr("Edit effect style"), "style.update", params, done);
            } else {
                app.run("style.update", params);
            }
        }
        Some(Act::SetGrids(id, grids, scrub)) => {
            let params = json!({"style": id, "value": grids});
            if let Some(done) = scrub {
                crate::typography::live(app, tr("Edit layout grid style"), "style.update", params, done);
            } else {
                app.run("style.update", params);
            }
        }
        Some(Act::UpdateFromSelection(id)) => {
            app.run("style.update", json!({"style": id, "from": ids.first()}));
        }
        Some(Act::Delete(id)) => {
            app.run("style.delete", json!({"style": id}));
            p.edit = None;
            if text_editor {
                close = true;
            }
        }
        Some(Act::ToggleFolder(f)) if !app.ui.style_folds.remove(&f) => {
            app.ui.style_folds.insert(f);
        }
        Some(Act::ToggleFolder(_)) => {}
        Some(Act::ToggleView) => app.ui.style_grid = !app.ui.style_grid,
        None => {}
    }
    if close {
        crate::primitives::FloatingPanel::dismiss(ctx, font_panel_id);
        crate::primitives::FloatingPanel::dismiss(ctx, settings_id);
        if let Some(edit) = &p.edit {
            ctx.data_mut(|data| data.remove::<usize>(Id::new(("effect_style_open", &edit.id))));
        }
        if let Some(edit) = &p.edit {
            ctx.data_mut(|data| data.remove::<usize>(Id::new(("grid_style_open", &edit.id))));
        }
        if let Some(e) = &p.edit {
            commit_edit(app, &doc, e);
        }
        if matches!(app.ui.picker, Some(PickerTarget::Style { .. } | PickerTarget::StyleEffect { .. })) {
            app.ui.picker = None;
        }
    } else {
        app.ui.style_picker = Some(p);
    }
}

/// Open the style editor on this file's style `id` (from the Local styles list).
fn edit_local(app: &mut LizardApp, ui: &Ui, slot: StyleSlot, id: &str) {
    let y = ui.input(|i| i.pointer.interact_pos()).map_or(140.0, |p| p.y - 12.0);
    let edit = app.session.doc().ok().and_then(|d| d.style(id).map(editing));
    ui.data_mut(|d| d.insert_temp(Id::new("style_picker_just_opened"), true));
    app.ui.style_picker = Some(Picker {
        slot,
        ids: app.session.selection(),
        y,
        query: String::new(),
        edit,
        libraries_tab: true,
        library_filter: None,
        libraries_only: false,
    });
}

/// `style.move` for the shown style `from` dropped at shown position `to` (`shown` in display
/// order): before the style that ends up under it, into the folder of its new neighbour above
/// (below at the top).
fn drop_move(shown: &[&Style], from: usize, to: usize) -> Option<Value> {
    let moved = shown.get(from)?;
    let rest: Vec<&Style> = shown.iter().enumerate().filter(|(i, _)| *i != from).map(|(_, s)| *s).collect();
    let below = rest.get(to);
    let near = to.checked_sub(1).and_then(|i| rest.get(i)).or(below)?;
    let mut q = json!({"styles": [moved.id], "folder": folder(&near.name)});
    if let Some(b) = below {
        q["before"] = json!(b.id);
    }
    Some(q)
}

/// The id of the folder caption being renamed in the Local styles list (temp data).
fn folder_rename_id() -> Id {
    Id::new("local_style_folder_rename")
}

fn create_local_style(app: &mut LizardApp, ui: &Ui, doc: &Document, slot: StyleSlot) {
    let name = next_name(doc, slot.default_name());
    let value = match slot {
        StyleSlot::Text => json!({"fontSize": 14}),
        StyleSlot::Fill | StyleSlot::Stroke => json!("#000000"),
        StyleSlot::Effect => json!([lizard_doc::Effect::drop_shadow()]),
        StyleSlot::Grid => json!([lizard_doc::LayoutGrid::default()]),
    };
    if let Some(id) = app
        .run("style.create", json!({"type": slot.create_type(), "name": name, "value": value}))
        .and_then(|result| result["id"].as_str().map(str::to_string))
    {
        edit_local(app, ui, slot, &id);
        if let Some(edit) = app.ui.style_picker.as_mut().and_then(|picker| picker.edit.as_mut()) {
            edit.creating = true;
            edit.focus = true;
        }
    }
}

/// The Design panel's Styles (shown with nothing selected): this file's saved styles by type
/// (text, colour, effect, grid), then by folder. The header creates a default style when there
/// is no selected layer. A click edits a style; right-click offers edit,
/// duplicate and delete; a row dragged by its grip moves (into the folder it lands in, renamed to
/// match); right-click on a folder renames it (and the styles in it).
pub fn local_styles(app: &mut LizardApp, ui: &mut Ui, doc: &Document) {
    let t = Tokens::get(ui.ctx());
    let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(header.min.x + widgets::PAD, header.center().y), Align2::LEFT_CENTER, tr("Styles"), semibold(11.0), t.text);
    let add_rect = Rect::from_min_size(pos2(header.max.x - 32.0, header.center().y - 12.0), vec2(24.0, 24.0));
    let add = Button::icon(Icon::Plus, tr("Create style")).icon_size(14.0).show_at(ui, Id::new("local_styles_add"), add_rect);
    let mut create = None;
    crate::right::menu_popup(&add).show(|ui| {
        for (slot, title) in [
            (StyleSlot::Text, tr("Text style")),
            (StyleSlot::Fill, tr("Color styles")),
            (StyleSlot::Effect, tr("Effect styles")),
            (StyleSlot::Grid, tr("Grid styles")),
        ] {
            if crate::right::menu_item(ui, title, "") {
                create = Some(slot);
                ui.close();
            }
        }
    });
    let renaming: Option<String> = ui.data(|d| d.get_temp(folder_rename_id()));
    let mut run: Option<(&'static str, Value)> = None;
    let mut edit: Option<(StyleSlot, String)> = None;
    let mut first_group = true;
    for slot in [StyleSlot::Text, StyleSlot::Fill, StyleSlot::Effect, StyleSlot::Grid] {
        let groups = by_folder(library::local_styles(doc).filter(|s| slot.holds(&s.kind)));
        if groups.is_empty() {
            continue;
        }
        if !first_group {
            ui.add_space(16.0);
        }
        first_group = false;
        let (heading, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        ui.painter().text(
            pos2(heading.min.x + widgets::PAD, heading.center().y),
            Align2::LEFT_CENTER,
            slot.title(),
            semibold(11.0),
            t.text_secondary,
        );
        if ui.rect_contains_pointer(heading) {
            let add_rect = Rect::from_min_size(pos2(heading.max.x - 32.0, heading.center().y - 12.0), vec2(24.0, 24.0));
            let id = Id::new(("local_style_add", slot.key()));
            if Button::icon(Icon::Plus, tr("Create style")).icon_size(14.0).show_at(ui, id, add_rect).clicked() {
                create = Some(slot);
            }
        }
        let mut drag = crate::list_drag::ListDrag::in_order(ui, ("local_styles", slot.key()));
        let mut shown: Vec<&Style> = Vec::new();
        for (f, list) in &groups {
            let indent = if f.is_empty() { 0.0 } else { 12.0 };
            if !f.is_empty() {
                let w = ui.available_width();
                let (r, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
                let text_r = Rect::from_min_max(pos2(r.min.x + widgets::PAD, r.min.y + 2.0), pos2(r.max.x - widgets::PAD, r.max.y - 2.0));
                if renaming.as_deref() == Some(*f) {
                    // Focused as it is first drawn: a focus asked for before its field is drawn
                    // isn't in the screen reader tree (UI Automation aborts the app, #224).
                    let start = ui.data_mut(|d| d.remove_temp::<bool>(folder_rename_id().with("start"))).unwrap_or(false);
                    let (out, open) = widgets::rename_field(ui, folder_rename_id().with(*f), f, text_r, regular(11.0), start);
                    if let Some(name) = out {
                        run = Some(("style.renameFolder", json!({"folder": f, "name": name})));
                    }
                    if !open {
                        ui.data_mut(|d| d.remove::<String>(folder_rename_id()));
                    }
                } else {
                    let resp = resp.tip(*f);
                    crate::bidi::paint_name(ui.painter(), text_r.left_center(), f, semibold(11.0), t.text, text_r.width());
                    crate::right::context_popup(&resp).show(|ui| {
                        if crate::right::menu_item(ui, tr("Rename folder"), "") {
                            ui.data_mut(|d| d.insert_temp(folder_rename_id(), f.to_string()));
                            ui.data_mut(|d| d.insert_temp(folder_rename_id().with("start"), true));
                            ui.close();
                        }
                    });
                }
            }
            for s in list {
                let index = shown.len();
                shown.push(s);
                let w = ui.available_width();
                let (r, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
                let hovered = ui.rect_contains_pointer(r);
                if hovered {
                    ui.painter().rect_filled(r.shrink2(vec2(8.0, 2.0)), CornerRadius::same(5), t.hover);
                }
                let x = r.min.x + widgets::PAD + indent + 1.0;
                let icon_r = Rect::from_center_size(pos2(x + 8.0, r.center().y), vec2(16.0, 16.0));
                match &s.kind {
                    StyleKind::Paint { paints } => {
                        let p = paints.last().cloned().unwrap_or_else(|| Paint::solid(lizard_color::Color::TRANSPARENT));
                        widgets::paint_swatch(ui.painter(), icon_r, &p);
                    }
                    StyleKind::Text { .. } => {
                        ui.painter().text(icon_r.center(), Align2::CENTER_CENTER, "Aa", regular(9.0), t.text);
                    }
                    StyleKind::Effect { .. } => icons::paint(ui.painter(), icon_r, Icon::Rectangle, t.icon_secondary),
                    _ => icons::paint(ui.painter(), icon_r, slot.icon(), t.icon_secondary),
                }
                let name_x = x + 24.0;
                let name_w = r.max.x - widgets::PAD - 32.0 - name_x;
                if let Some(def) = text_def(s) {
                    let meta = format!("· {}", text_meta(def));
                    let meta_w = ui.painter().layout_no_wrap(meta.clone(), regular(10.0), t.text_secondary).size().x;
                    let name_w = (name_w - meta_w - 6.0).max(0.0);
                    let name = widgets::elide_font(ui, leaf(&s.name), name_w, regular(11.0));
                    let shown_w = ui.painter().layout_no_wrap(name.clone(), regular(11.0), t.text).size().x;
                    crate::bidi::paint_name(ui.painter(), pos2(name_x, r.center().y), &name, regular(11.0), t.text, name_w);
                    ui.painter().text(pos2(name_x + shown_w + 5.0, r.center().y), Align2::LEFT_CENTER, meta, regular(10.0), t.text_secondary);
                } else {
                    crate::bidi::paint_name(ui.painter(), pos2(name_x, r.center().y), leaf(&s.name), regular(11.0), t.text, name_w);
                }
                let tip = if s.description.is_empty() { s.name.clone() } else { format!("{}\n{}", s.name, s.description) };
                let resp = resp.tip(tip);
                if resp.clicked() {
                    edit = Some((slot, s.id.clone()));
                }
                if hovered {
                    let settings_rect = Rect::from_min_size(pos2(r.max.x - 32.0, r.center().y - 12.0), vec2(24.0, 24.0));
                    let id = Id::new(("local_style_settings", &s.id));
                    if Button::layout_icon(LayoutIcon::Settings, tr("Edit style")).icon_size(14.0).show_at(ui, id, settings_rect).clicked() {
                        edit = Some((slot, s.id.clone()));
                    }
                }
                crate::right::context_popup(&resp).show(|ui| {
                    if crate::right::menu_item(ui, tr("Edit style"), "") {
                        edit = Some((slot, s.id.clone()));
                        ui.close();
                    }
                    if crate::right::menu_item(ui, tr("Duplicate"), "") {
                        run = Some(("style.duplicate", json!({"style": s.id})));
                        ui.close();
                    }
                    if crate::right::menu_item(ui, tr("Delete"), "") {
                        run = Some(("style.delete", json!({"style": s.id})));
                        ui.close();
                    }
                });
                drag.row(ui, index, r, r, || leaf(&s.name).to_string());
            }
        }
        if let Some(q) = drag.finish(ui).and_then(|(from, to)| drop_move(&shown, from, to)) {
            run = Some(("style.move", q));
        }
    }
    if let Some((cmd, q)) = run {
        app.run(cmd, q);
    }
    if let Some((slot, id)) = edit {
        edit_local(app, ui, slot, &id);
    }
    if let Some(slot) = create {
        create_local_style(app, ui, doc, slot);
    }
}

#[cfg(test)]
#[path = "style_picker_tests.rs"]
mod tests;
