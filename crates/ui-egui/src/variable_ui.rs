//! Binding variables to properties from the Design panel: number fields show a variable button on
//! hover and the bound variable's name once bound; colour fills bind through the style picker. The
//! picker popover lists the document's variables of the matching type (and scope), by collection,
//! then those of each enabled library (picking one copies it in, linked and read-only); the
//! Variables editor uses it to pick alias targets too, and the interaction editor the variables of
//! set variable actions and conditions.

use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use lizard_doc::{Collection, Document, ModeId, NodeId, PropDef, PublishedLibrary, Variable, VariableId, VariableType, VariableValue, Variables};
use lizard_engine::library;
use serde_json::json;

use crate::LizardApp;
use crate::a11y::Tip;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::primitives::{Button, ControlWidth, FloatingPanel, FloatingPanelWidth, SearchField, Select, SelectPosition, SelectVariant};
use crate::theme::{COMPONENT, Tokens, regular, semibold};
use crate::widgets::{self, BindEvent, Edit, Empty, FieldEdit, Lead};

/// What picking a variable does.
#[derive(Clone)]
enum Target {
    /// Bind it to `field` of `ids`.
    Bind { ids: Vec<NodeId>, field: String },
    /// Make `variable`'s value in `mode` an alias of it.
    Alias { variable: VariableId, mode: ModeId },
    /// Use it in a prototype action (a set variable's variable, a condition's operand).
    Proto(crate::proto_editor::VarSlot),
    /// Bind it to `field` of the shared style `style`.
    Style { style: String, field: String },
}

/// The open variable picker: what it does, the variable type (None: any), where it opens.
#[derive(Clone)]
struct Picker {
    target: Target,
    kind: Option<VariableType>,
    at: Pos2,
    opened_frame: u64,
    library_filter: usize,
}

fn picker_id() -> Id {
    Id::new("variable_picker")
}

/// The variable `field` is bound to on every one of `ids` (its name), if they agree.
pub fn bound_name(doc: &Document, ids: &[NodeId], field: &str) -> Option<String> {
    let mut it = ids.iter().map(|i| doc.node(*i).and_then(|n| n.bound.get(field)).cloned());
    let first = it.next().flatten()?;
    if !it.all(|v| v.as_ref() == Some(&first)) {
        return None;
    }
    doc.variables.get(&first).map(|v| v.name.clone())
}

/// A variable a picker offers: one of this file's, or a published one of an enabled library.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    /// The library's id; None for this file's variables.
    pub library: Option<String>,
    /// The variable's id (in the library for a library variable).
    pub id: VariableId,
}

/// Bind `field` of `ids` to `pick` (a library variable is copied in first).
pub fn bind(app: &mut LizardApp, ids: &[NodeId], field: &str, pick: &Pick) {
    match &pick.library {
        Some(lib) => app.run("library.bindVariable", json!({"library": lib, "variable": pick.id, "ids": ids, "field": field})),
        None => app.run("variable.bind", json!({"ids": ids, "field": field, "variable": pick.id})),
    };
}

/// The enabled libraries this session has read, for pickers and the Variables editor.
pub fn libraries(app: &LizardApp, doc: &Document) -> Vec<Arc<PublishedLibrary>> {
    library::read_libraries(&app.session.libraries, doc).map(|(_, lib)| lib.clone()).collect()
}

/// A variable group a picker lists: its library (None: this file), its collection and the
/// variables offered.
pub type Group<'a> = (Option<&'a PublishedLibrary>, &'a Collection, Vec<&'a Variable>);

/// Every group a picker lists, with the variables passing `keep`: this file's collections first
/// (never library copies), then each library's published ones. Empty groups are left out.
pub fn groups<'a>(
    doc: &'a Document,
    libs: &'a [Arc<PublishedLibrary>],
    keep: impl Fn(Option<&PublishedLibrary>, &Variable) -> bool,
) -> Vec<Group<'a>> {
    let mut out: Vec<Group<'a>> = Vec::new();
    let local: Vec<&Variable> = doc.variables.variables.iter().filter(|v| !library::is_remote_variable(doc, &v.id)).collect();
    for c in library::local_collections(doc) {
        out.push((None, c, local.iter().copied().filter(|v| v.collection == c.id && keep(None, v)).collect()));
    }
    for lib in libs {
        let offered: Vec<&Variable> = library::published_variables(lib).collect();
        for c in library::published_collections(lib) {
            out.push((Some(lib.as_ref()), c, offered.iter().copied().filter(|v| v.collection == c.id && keep(Some(lib), v)).collect()));
        }
    }
    out.retain(|g| !g.2.is_empty());
    out
}

/// A collection the Design panel's mode picker lists: this file's, or one an enabled library
/// offers (or offered: a copy whose library isn't read any more).
#[derive(Clone, Debug, PartialEq)]
pub struct ModeRow<'a> {
    /// The collection with its modes (a library's copy once it is used here).
    pub coll: &'a Collection,
    /// The library's (id, name); None for this file's collections.
    pub library: Option<(&'a str, &'a str)>,
    /// The collection's id in the library, when the library is read (modes are set through
    /// `library.setVariableMode`, which copies the collection in).
    pub key: Option<&'a str>,
    /// The id frames' modes are kept under: this file's collection or a library collection's copy
    /// (None until a library collection is used here).
    pub local: Option<&'a str>,
}

/// The collections with more than one mode the mode picker lists: this file's first, then each
/// enabled library's published ones (their copies once used), then copies of collections from
/// libraries that aren't read.
pub fn mode_rows<'a>(doc: &'a Document, libs: &'a [Arc<PublishedLibrary>]) -> Vec<ModeRow<'a>> {
    let several = |c: &Collection| c.modes.len() > 1;
    let mut out: Vec<ModeRow<'a>> =
        library::local_collections(doc).filter(|c| several(c)).map(|c| ModeRow { coll: c, library: None, key: None, local: Some(&c.id) }).collect();
    let copy = |lib: &str, key: &str| doc.libraries.collections.iter().find(|r| r.library == lib && r.key == key);
    for lib in libs {
        let (id, name) = (lib.manifest.id.as_str(), library::library_name(doc, &lib.manifest.id));
        for c in library::published_collections(lib) {
            let local = copy(id, &c.id).map(|r| r.local.as_str());
            let coll = local.and_then(|l| doc.variables.collection(l)).unwrap_or(c);
            if several(coll) {
                out.push(ModeRow { coll, library: Some((id, name)), key: Some(&c.id), local });
            }
        }
    }
    for r in &doc.libraries.collections {
        if out.iter().any(|x| x.local == Some(r.local.as_str())) {
            continue;
        }
        if let Some(c) = doc.variables.collection(&r.local).filter(|c| several(c)) {
            out.push(ModeRow { coll: c, library: Some((&r.library, library::library_name(doc, &r.library))), key: None, local: Some(&r.local) });
        }
    }
    out
}

/// A picker row: the variable's swatch (colours) or icon, its name and (other types) its value in
/// the default modes; true when clicked.
pub fn variable_row(ui: &mut Ui, vars: &Variables, v: &Variable, width: f32) -> bool {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(5), t.menu_hover);
    }
    let value = vars.resolve(&v.id, &std::collections::BTreeMap::new());
    let lead = Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(14.0, 14.0));
    match &value {
        Some(VariableValue::Color(c)) => widgets::paint_swatch(ui.painter(), lead, &lizard_color::Paint::solid(*c)),
        _ => icons::paint(ui.painter(), lead.shrink(1.0), Icon::Variable, t.menu_text),
    }
    let name_r = Rect::from_min_max(pos2(r.min.x + 26.0, r.min.y), pos2(r.max.x - 6.0, r.max.y));
    if let Some(val) = value.filter(|v| !matches!(v, VariableValue::Color(_))) {
        ui.painter().text(pos2(r.max.x - 6.0, r.center().y), Align2::RIGHT_CENTER, val.display(), regular(11.0), t.menu_dim);
    }
    ui.painter().with_clip_rect(name_r).text(name_r.left_center(), Align2::LEFT_CENTER, &v.name, regular(11.0), t.menu_text);
    resp.clicked()
}

/// A group's caption: the collection's name, after its library's name for library variables.
pub fn group_caption(ui: &mut Ui, lib: Option<&PublishedLibrary>, coll: &Collection) {
    let t = Tokens::get(ui.ctx());
    let text = match lib {
        Some(l) => format!("{} · {}", l.manifest.name, coll.name),
        None => coll.name.clone(),
    };
    ui.label(egui::RichText::new(text).font(regular(10.0)).color(t.menu_dim));
}

/// Open the picker for `field` of `ids` at `at`.
pub fn open(ui: &Ui, ids: &[NodeId], field: &str, kind: VariableType, at: Pos2) {
    let target = Target::Bind { ids: ids.to_vec(), field: field.to_string() };
    let opened_frame = ui.ctx().cumulative_frame_nr();
    ui.data_mut(|d| d.insert_temp(picker_id(), Picker { target, kind: Some(kind), at, opened_frame, library_filter: 0 }));
}

/// Open the picker at `at` to pick a variable (of `kind`, or any) for a prototype action.
pub fn open_proto(ui: &Ui, slot: crate::proto_editor::VarSlot, kind: Option<VariableType>, at: Pos2) {
    let opened_frame = ui.ctx().cumulative_frame_nr();
    ui.data_mut(|d| d.insert_temp(picker_id(), Picker { target: Target::Proto(slot), kind, at, opened_frame, library_filter: 0 }));
}

/// Open the picker at `at` to bind a variable of `kind` to `field` of the shared style `style`.
pub fn open_style(ui: &Ui, style: &str, field: &str, kind: VariableType, at: Pos2) {
    let target = Target::Style { style: style.to_string(), field: field.to_string() };
    let opened_frame = ui.ctx().cumulative_frame_nr();
    ui.data_mut(|d| d.insert_temp(picker_id(), Picker { target, kind: Some(kind), at, opened_frame, library_filter: 0 }));
}

/// Where the open picker is on screen (clicks in it don't close what opened it).
pub fn picker_rect(ctx: &egui::Context) -> Option<Rect> {
    picker_open(ctx).then(|| ctx.memory(|m| m.area_rect(Id::new("variable_picker_area")))).flatten()
}

/// Whether the picker is open (Escape closes it before what opened it).
pub fn picker_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Picker>(picker_id()).is_some())
}

/// Open the picker at `at` to alias `variable`'s value in `mode` to another variable.
pub fn open_alias(ui: &Ui, variable: &str, mode: &str, kind: VariableType, at: Pos2) {
    let target = Target::Alias { variable: variable.to_string(), mode: mode.to_string() };
    let opened_frame = ui.ctx().cumulative_frame_nr();
    ui.data_mut(|d| d.insert_temp(picker_id(), Picker { target, kind: Some(kind), at, opened_frame, library_filter: 0 }));
}

/// Open the picker for `field` of `ids` under the pointer (from a section header button).
pub fn open_at_pointer(ui: &Ui, ids: &[NodeId], field: &str, kind: VariableType) {
    let at = ui.input(|i| i.pointer.interact_pos()).unwrap_or_default();
    open(ui, ids, field, kind, at);
}

/// Act on a bound field's event: change the variable, or detach it.
fn handle(app: &mut LizardApp, ui: &Ui, ids: &[NodeId], field: &str, kind: VariableType, ev: Option<BindEvent>) {
    match ev {
        Some(BindEvent::Open(at)) => open(ui, ids, field, kind, at),
        Some(BindEvent::Detach) => {
            app.run("variable.bind", json!({"ids": ids, "field": field, "variable": null}));
        }
        None => {}
    }
}

/// A number field whose value can be bound to a number variable. `suffix` follows the number
/// ("%" for opacity, whose variables hold percentages).
pub fn number_field(
    app: &mut LizardApp,
    ui: &mut Ui,
    ids: &[NodeId],
    field: &str,
    id: Id,
    lead: Lead,
    value: Option<f64>,
    suffix: &str,
    width: f32,
) -> Option<FieldEdit> {
    number_field_or(app, ui, ids, field, id, value, width, tr("Mixed"), lead, suffix)
}

/// A bindable number field with a joined preset menu.
pub fn number_field_with_options(
    app: &mut LizardApp,
    ui: &mut Ui,
    ids: &[NodeId],
    field: &str,
    id: Id,
    value: Option<f64>,
    width: f32,
    options: &[f64],
) -> crate::primitives::NumberFieldResponse {
    let name = app.session.doc().ok().and_then(|d| bound_name(d, ids, field));
    let mut result = crate::primitives::NumberField::new(id, Lead::Named(tr("Font size")), value, "")
        .binding(Some(name.as_deref()))
        .options(options)
        .show(ui, crate::primitives::ControlWidth::Fixed(width));
    handle(app, ui, ids, field, VariableType::Number, result.binding.take());
    result
}

/// [`number_field`] showing `empty` (e.g. "Auto") for no value.
pub fn number_field_or(
    app: &mut LizardApp,
    ui: &mut Ui,
    ids: &[NodeId],
    field: &str,
    id: Id,
    value: Option<f64>,
    width: f32,
    empty: &str,
    lead: Lead,
    suffix: &str,
) -> Option<FieldEdit> {
    number_field_edit(app, ui, ids, field, id, value, width, Empty::Text(empty), lead, suffix)?.settled()
}

/// [`number_field_or`] for a field where 0 is its automatic value (a layout grid's width or
/// count): typing the word for Auto sets 0 ([`Empty::Auto`]).
pub fn number_field_auto(
    app: &mut LizardApp,
    ui: &mut Ui,
    ids: &[NodeId],
    field: &str,
    id: Id,
    value: Option<f64>,
    width: f32,
    lead: Lead,
    suffix: &str,
) -> Option<FieldEdit> {
    number_field_edit(app, ui, ids, field, id, value, width, Empty::Auto, lead, suffix)?.settled()
}

/// [`number_field`] over several layers: a relative edit of mixed values comes back as one for
/// each layer ([`Edit::Each`]; see `LizardApp::field_of`).
pub fn number_field_mixed(
    app: &mut LizardApp,
    ui: &mut Ui,
    ids: &[NodeId],
    field: &str,
    id: Id,
    lead: Lead,
    value: Option<f64>,
    suffix: &str,
    width: f32,
) -> Option<Edit> {
    number_field_edit(app, ui, ids, field, id, value, width, Empty::Text(tr("Mixed")), lead, suffix)
}

/// A bindable dimension value with a sizing mode selector sharing its 24 pt field.
pub fn dimension_field(
    app: &mut LizardApp,
    ui: &mut Ui,
    ids: &[NodeId],
    field: &str,
    id: Id,
    lead: Lead,
    value: Option<f64>,
    width: f32,
    sizing_label: &str,
) -> (Option<Edit>, egui::Response) {
    let name = app.session.doc().ok().and_then(|d| bound_name(d, ids, field));
    let (edit, ev, selector) = widgets::number_field_with_sizing(ui, id, lead, value, width, Some(name.as_deref()), sizing_label);
    handle(app, ui, ids, field, VariableType::Number, ev);
    (edit, selector)
}

fn number_field_edit(
    app: &mut LizardApp,
    ui: &mut Ui,
    ids: &[NodeId],
    field: &str,
    id: Id,
    value: Option<f64>,
    width: f32,
    empty: Empty<'_>,
    lead: Lead,
    suffix: &str,
) -> Option<Edit> {
    let name = app.session.doc().ok().and_then(|d| bound_name(d, ids, field));
    let result = crate::primitives::NumberField::new(id, lead, value, suffix)
        .binding(Some(name.as_deref()))
        .empty(empty)
        .show(ui, crate::primitives::ControlWidth::Fixed(width));
    handle(app, ui, ids, field, VariableType::Number, result.binding);
    result.edit
}

/// When `field` of `ids` is bound: the variable's chip in `rect` (change or detach it) and true;
/// otherwise nothing and false.
pub fn bound_chip(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], field: &str, kind: VariableType, rect: Rect) -> bool {
    let Some(name) = app.session.doc().ok().and_then(|d| bound_name(d, ids, field)) else { return false };
    let ev = widgets::bound_field(ui, Id::new(("bound_chip", field, ids)), Lead::None, rect, &name);
    handle(app, ui, ids, field, kind, ev);
    true
}

/// A control whose value can come from a variable: while `field` of `ids` is bound, the
/// variable's chip (`width` wide) stands in for it; otherwise `control` lays it out in the current
/// row and a variable button shows over that area on hover.
pub fn bindable(
    app: &mut LizardApp,
    ui: &mut Ui,
    ids: &[NodeId],
    field: &str,
    kind: VariableType,
    width: f32,
    control: impl FnOnce(&mut LizardApp, &mut Ui),
) {
    let area = Rect::from_min_size(ui.cursor().min, vec2(width, widgets::FIELD_H));
    if app.session.doc().ok().and_then(|d| bound_name(d, ids, field)).is_some() {
        let (r, _) = ui.allocate_exact_size(area.size(), Sense::hover());
        bound_chip(app, ui, ids, field, kind, r);
        return;
    }
    control(app, ui);
    bind_button(ui, area, ids, field, kind);
}

/// A variable button at the right end of `rect` while the pointer is over it: opens the picker
/// to bind `field` of `ids`.
pub fn bind_button(ui: &mut Ui, rect: Rect, ids: &[NodeId], field: &str, kind: VariableType) {
    if variable_button(ui, rect, Id::new(("bind_button", field, ids))) {
        open(ui, ids, field, kind, rect.left_bottom());
    }
}

/// The "Apply variable" button at the right end of `rect` while the pointer is over it; true
/// when clicked.
pub fn variable_button(ui: &mut Ui, rect: Rect, id: Id) -> bool {
    if !ui.rect_contains_pointer(rect) {
        return false;
    }
    let t = Tokens::get(ui.ctx());
    let b = Rect::from_center_size(pos2(rect.max.x - 11.0, rect.center().y), vec2(18.0, 18.0));
    let resp = ui.interact(b, id, Sense::click()).tip(tr("Apply variable"));
    if resp.hovered() {
        ui.painter().rect_filled(b, CornerRadius::same(4), t.hover);
    }
    icons::paint(ui.painter(), b.shrink(3.0), Icon::Variable, t.icon_secondary);
    resp.clicked()
}

/// A field showing the variable `field` of `ids` is bound to, for bindings without a value field
/// of their own (text content, visibility): `width` wide, at most the row. Nothing when unbound.
pub fn bound_row(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], field: &str, kind: VariableType, lead: Lead, width: f32) {
    let Some(name) = app.session.doc().ok().and_then(|d| bound_name(d, ids, field)) else { return };
    crate::right::row(ui, 32.0, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(width.min(ui.available_width()), widgets::FIELD_H), Sense::hover());
        let ev = widgets::bound_field(ui, Id::new(("bound_row", field)), lead, rect, &name);
        handle(app, ui, ids, field, kind, ev);
    });
}

fn binding_pick_row(ui: &mut Ui, label: &str, value: &str, icon: Icon, icon_color: Color32) -> bool {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.hover);
    }
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(16.0, 16.0)), icon, icon_color);
    let name_rect = Rect::from_min_max(pos2(rect.left() + 40.0, rect.top()), pos2((rect.right() - 92.0).max(rect.left() + 40.0), rect.bottom()));
    let name = crate::widgets::elide_font(ui, label, name_rect.width(), regular(11.0));
    ui.painter().text(pos2(name_rect.left(), rect.center().y), Align2::LEFT_CENTER, name, regular(11.0), t.menu_text);
    let value_rect = Rect::from_min_max(pos2(rect.right() - 88.0, rect.top()), pos2(rect.right() - 12.0, rect.bottom()));
    let value = crate::widgets::elide_font(ui, value, value_rect.width(), regular(11.0));
    ui.painter().with_clip_rect(value_rect).text(
        pos2(value_rect.right(), rect.center().y), Align2::RIGHT_CENTER, value, regular(11.0), t.menu_dim,
    );
    response.clicked()
}

fn picker_heading(ui: &mut Ui, title: &str) {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    ui.painter().text(pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, title, regular(10.0), t.text_secondary);
}

/// Whether a variable of type `have` belongs in a picker for `want` on `field`: its own type, and
/// numbers for text content too (shown as text).
fn kind_fits(want: Option<VariableType>, field: Option<&str>, have: VariableType) -> bool {
    want.is_none_or(|k| have == k || (field == Some("characters") && have == VariableType::Number))
}

/// The picker popover (call once per frame).
pub fn picker(app: &mut LizardApp, ctx: &egui::Context) {
    let Some(mut p) = ctx.data(|d| d.get_temp::<Picker>(picker_id())) else { return };
    if let Target::Bind { ids, .. } = &p.target
        && *ids != app.session.selection()
    {
        ctx.data_mut(|data| data.remove::<Picker>(picker_id()));
        return;
    }
    let Ok(doc) = app.session.ds().map(|d| d.doc.clone()) else { return };
    let libs = libraries(app, &doc);
    let t = Tokens::get(ctx);
    let query_id = Id::new("variable_picker_query");
    let mut query: String = ctx.data(|d| d.get_temp(query_id)).unwrap_or_default();
    let q = query.trim().to_lowercase();
    // Bindings offer variables scoped to the property; aliases, any that wouldn't loop (library
    // variables never lead back to this file's).
    let (scope, own) = match &p.target {
        Target::Bind { ids, field } => (ids.first().and_then(|id| lizard_engine::cmd::variables::field_scope(&doc, *id, field)), None),
        Target::Alias { variable, .. } => (None, doc.variables.get(variable)),
        Target::Proto(_) | Target::Style { .. } => (None, None),
    };
    // Prototypes and styles use this file's variables.
    let local_only = matches!(p.target, Target::Proto(_) | Target::Style { .. }) || p.library_filter == 1;
    let field = match &p.target {
        Target::Bind { field, .. } => Some(field.as_str()),
        _ => None,
    };
    let keep = |lib: Option<&PublishedLibrary>, v: &Variable| {
        kind_fits(p.kind, field, v.kind)
            && !(local_only && lib.is_some())
            && (q.is_empty() || v.name.to_lowercase().contains(&q))
            && scope.is_none_or(|s| v.in_scope(s))
            && (lib.is_some() || own.is_none_or(|o| lizard_engine::cmd::variables::alias_target(&doc.variables, o, &v.id).is_ok()))
    };
    let groups = groups(&doc, &libs, keep);
    let component = match &p.target {
        Target::Bind { ids, field } if ids.len() == 1 => ids.first().and_then(|id| {
            (!lizard_engine::components::inside_instance(&doc, *id))
                .then(|| lizard_engine::components::owning_component(&doc, *id))
                .flatten()
                .map(|owner| (*id, owner, field.as_str()))
        }),
        _ => None,
    };
    let component_properties: Vec<(String, String)> = component
        .and_then(|(_, owner, field)| doc.node(owner).map(|node| (node, field)))
        .and_then(|(node, field)| match &node.kind {
            lizard_doc::NodeKind::Component { props, .. } => Some((props, field)),
            _ => None,
        })
        .map(|(defs, field)| {
            defs.iter()
                .filter_map(|(name, def)| match (field, def) {
                    ("characters", PropDef::Text { default }) => Some((name.clone(), default.clone())),
                    ("visible", PropDef::Boolean { default }) => Some((name.clone(), default.to_string())),
                    _ => None,
                })
                .filter(|(name, _)| q.is_empty() || name.to_lowercase().contains(&q))
                .collect()
        })
        .unwrap_or_default();
    let component_name = component.and_then(|(_, owner, _)| {
        let display = lizard_engine::components::variant_set(&doc, owner).unwrap_or(owner);
        doc.node(display).map(|n| n.name.clone())
    });
    let mut chosen: Option<Pick> = None;
    let mut chosen_property: Option<(NodeId, String, String)> = None;
    let mut open_variables = false;
    let mut close = false;
    const WIDTH: f32 = 230.0;
    let border = Stroke::new(1.0, t.border);
    let panel = FloatingPanel::new(Id::new("variable_picker_area"), p.at + vec2(0.0, 4.0))
        .anchor_to_properties()
        .width(FloatingPanelWidth::Fixed(WIDTH))
        .corner_radius(12)
        .header_height(40.0);
    let Some(area) = panel.show_with_header(ctx, |ui, header| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.painter().text(pos2(header.left() + 16.0, header.center().y), Align2::LEFT_CENTER, tr("Apply variable"), semibold(11.0), t.text);
        let close_rect = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
        if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, Id::new("variable_picker_close"), close_rect).clicked() {
            close = true;
        }
        ui.painter().hline(header.x_range(), header.bottom() - 0.5, border);

        let (search_row, _) = ui.allocate_exact_size(vec2(WIDTH, 40.0), Sense::hover());
        let search_rect = Rect::from_min_size(pos2(search_row.left() + 16.0, search_row.top() + 8.0), vec2(WIDTH - 32.0, 24.0));
        let mut search_ui = ui.new_child(egui::UiBuilder::new().max_rect(search_rect));
        SearchField::new(query_id, tr("Search"))
            .width(ControlWidth::Fill)
            .compact()
            .clearable()
            .ghost()
            .show(&mut search_ui, &mut query);
        ui.painter().hline(search_row.x_range(), search_row.bottom() - 0.5, border);

        let (filter_row, _) = ui.allocate_exact_size(vec2(WIDTH, 40.0), Sense::hover());
        let options = [tr("All libraries"), tr("This file")];
        let select_rect = Rect::from_min_size(pos2(filter_row.left() + 16.0, filter_row.top() + 8.0), vec2(WIDTH - 80.0, 24.0));
        let mut select_ui = ui.new_child(egui::UiBuilder::new().max_rect(select_rect));
        if let Some(next) = Select::new(&options, p.library_filter)
            .label(tr("Libraries"))
            .width(ControlWidth::Fill)
            .variant(SelectVariant::Outlined)
            .position(SelectPosition::BottomLeft)
            .show(&mut select_ui)
        {
            p.library_filter = next;
        }
        let add_rect = Rect::from_center_size(pos2(filter_row.right() - 24.0, filter_row.center().y), vec2(24.0, 24.0));
        if Button::icon(Icon::Plus, tr("Create variable")).icon_size(16.0).show_at(ui, Id::new("variable_picker_add"), add_rect).clicked() {
            open_variables = true;
        }
        ui.painter().hline(filter_row.x_range(), filter_row.bottom() - 0.5, border);

        egui::ScrollArea::vertical().id_salt("variable_picker_scroll").max_height(300.0).show(ui, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(4.0);
            if let Some(name) = &component_name {
                picker_heading(ui, &format!("{} {name}", tr("Properties in")));
                if let Some((layer, _, field)) = component {
                    for (property, default) in &component_properties {
                        let icon = if field == "visible" { Icon::VariableBoolean } else { Icon::VariableString };
                        if binding_pick_row(ui, property, default, icon, COMPONENT) {
                            chosen_property = Some((layer, field.to_string(), property.clone()));
                        }
                    }
                }
                ui.add_space(8.0);
            }
            picker_heading(ui, tr("Variables in this file"));
            let mut library_heading = false;
            for (lib, c, vars) in &groups {
                if lib.is_some() && !library_heading {
                    ui.add_space(8.0);
                    picker_heading(ui, tr("Library variables"));
                    library_heading = true;
                }
                if lib.is_some() || component_name.is_none() {
                    group_caption(ui, *lib, c);
                }
                let all = lib.map_or(&doc.variables, |l| &l.doc.variables);
                for v in vars {
                    let value = all.resolve(&v.id, &std::collections::BTreeMap::new()).map(|value| value.display()).unwrap_or_default();
                    let icon = match v.kind {
                        VariableType::String => Icon::VariableString,
                        VariableType::Boolean => Icon::VariableBoolean,
                        VariableType::Number => Icon::VariableNumber,
                        VariableType::Color => Icon::VariableColor,
                    };
                    if binding_pick_row(ui, &v.name, &value, icon, t.icon) {
                        chosen = Some(Pick { library: lib.map(|l| l.manifest.id.clone()), id: v.id.clone() });
                    }
                }
            }
            if groups.is_empty() && component_properties.is_empty() {
                let (fit, none) = match p.kind {
                    Some(VariableType::Color) => {
                        (tr("No colour variables fit here."), tr("No colour variables yet. Create them in Local variables."))
                    }
                    Some(VariableType::Number) => {
                        (tr("No number variables fit here."), tr("No number variables yet. Create them in Local variables."))
                    }
                    Some(VariableType::String) => {
                        (tr("No string variables fit here."), tr("No string variables yet. Create them in Local variables."))
                    }
                    Some(VariableType::Boolean) => {
                        (tr("No boolean variables fit here."), tr("No boolean variables yet. Create them in Local variables."))
                    }
                    None => (tr("No variables fit here."), tr("No variables yet. Create them in Local variables.")),
                };
                let msg = if groups_any_kind(&doc, &libs, p.kind) { fit } else { none };
                ui.label(egui::RichText::new(msg).font(regular(11.0)).color(t.menu_dim));
            }
            ui.add_space(8.0);
        });
    }) else { return };
    let rect = area.response.rect;
    let clicked_outside = ctx.cumulative_frame_nr() > p.opened_frame
        && ctx.input(|i| {
            (i.pointer.button_pressed(egui::PointerButton::Primary) || i.pointer.button_pressed(egui::PointerButton::Secondary))
                && i.pointer.interact_pos().is_some_and(|pos| !rect.contains(pos))
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || clicked_outside {
        close = true;
    }
    if let Some(pick) = chosen {
        match (&p.target, &pick.library) {
            (Target::Bind { ids, field }, _) => bind(app, ids, field, &pick),
            (Target::Alias { variable, mode }, None) => {
                app.run("variable.set", json!({"variable": variable, "mode": mode, "value": {"alias": pick.id}}));
            }
            (Target::Alias { variable, mode }, Some(lib)) => {
                app.run("library.aliasVariable", json!({"library": lib, "alias": pick.id, "variable": variable, "mode": mode}));
            }
            (Target::Proto(slot), _) => crate::proto_editor::pick_variable(app, slot, &pick.id),
            (Target::Style { style, field }, _) => {
                app.run("style.bind", json!({"style": style, "field": field, "variable": pick.id}));
            }
        }
        close = true;
    }
    if let Some((layer, field, property)) = chosen_property {
        app.run("component.bindProperty", json!({"layer": layer, "field": field, "property": property}));
        close = true;
    }
    if open_variables {
        app.ui.variables.get_or_insert_with(Default::default);
        close = true;
    }
    if close {
        ctx.data_mut(|d| {
            d.remove::<Picker>(picker_id());
            d.remove::<String>(query_id);
        });
    } else {
        ctx.data_mut(|d| {
            d.insert_temp(query_id, query);
            d.insert_temp(picker_id(), p);
        });
    }
}

/// Whether this file or a library offers any variable of `kind` (or any variable).
fn groups_any_kind(doc: &Document, libs: &[Arc<PublishedLibrary>], kind: Option<VariableType>) -> bool {
    !groups(doc, libs, |_, v| kind.is_none_or(|k| v.kind == k)).is_empty()
}

#[cfg(test)]
mod kind_tests {
    use super::*;

    #[test]
    fn opening_value_picker_does_not_reenter_egui_data_lock() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            open(ui, &[NodeId(1)], "characters", VariableType::String, Pos2::ZERO);
            assert!(picker_open(ui.ctx()));
        });
        output.textures_delta.clear();
    }

    #[test]
    fn text_content_offers_numbers_too() {
        use VariableType::*;
        assert!(kind_fits(Some(String), Some("characters"), Number), "a number shows as text");
        assert!(kind_fits(Some(String), Some("characters"), String));
        assert!(!kind_fits(Some(String), Some("characters"), Color));
        assert!(!kind_fits(Some(String), Some("fontFamily"), Number), "only text content");
        assert!(!kind_fits(Some(Number), Some("width"), String));
        assert!(kind_fits(None, None, Boolean));
    }
}
