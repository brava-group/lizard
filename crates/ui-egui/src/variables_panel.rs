//! The Variables editor: collections on the left (this file's, then each enabled library's,
//! read-only under the library's name); the selected collection's variables on the right as a
//! table with one value column per mode. Double-click a name to rename it, right-click for more; a
//! value's variable button turns it into an alias of another variable (a library one too).
//! A library collection can be extended: a collection of this file with its modes and variables,
//! whose overridden values are marked (with a reset). Import and export read and write the
//! variables JSON (or CSS). Everything goes through `variable.*` / `library.*` commands, so it's
//! undoable and the same over MCP.

use egui::{Align2, Context, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use lizard_doc::{Collection, Document, PublishedLibrary, Variable, VariableType, VariableValue, Variables, scopes_for};
use lizard_engine::library;
use serde_json::{Value, json};

use crate::LizardApp;
use crate::a11y::Tip;
use crate::colorpicker::PickerTarget;
use crate::i18n::{tr, tr_noop, trf, trn};
use crate::icons::{self, Icon};
use crate::primitives::{
    AliasValue, Button, ButtonBehavior, ButtonVariant, ColorControl, ColorValue, ControlWidth, InlineField, SearchField, Surface, Switch,
};
use crate::right::{menu_check, menu_item};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets::{self, FieldEdit};

const SIDEBAR_W: f32 = 240.0;
const NAME_W: f32 = 200.0;
const MODE_W: f32 = 280.0;
const ROW_H: f32 = 40.0;
const TYPES: [VariableType; 4] = [VariableType::Color, VariableType::Number, VariableType::String, VariableType::Boolean];

/// What's being renamed inline.
#[derive(Clone, Debug, PartialEq, Hash)]
enum Rename {
    Collection(String),
    /// (collection, mode)
    Mode(String, String),
    Variable(String),
    /// (collection, group path)
    Group(String, String),
}

/// The open editor's state (`UiState::variables`; None = closed).
#[derive(Clone, Default)]
pub struct Editor {
    /// The library whose collection is shown (id or name); None for this file's.
    library: Option<String>,
    /// The selected collection's id.
    collection: Option<String>,
    query: String,
    rename: Option<Rename>,
    /// The rename just started: its field takes the focus in the frame it's first drawn.
    rename_start: bool,
    /// The variable whose details (name, description, scopes) are open.
    details: Option<String>,
    /// The group the table shows ("space" or "color/brand"); None = every variable.
    group: Option<String>,
    kind_filter: Option<VariableType>,
    expanded: bool,
}

impl Editor {
    /// An editor showing `collection` (id or name; the first one when None or unknown).
    pub fn on(collection: Option<String>) -> Self {
        Editor { collection, ..Default::default() }
    }

    /// An editor showing a collection of enabled library `library` (id or name).
    pub fn in_library(library: String, collection: Option<String>) -> Self {
        Editor { library: Some(library), collection, ..Default::default() }
    }
}

/// What the editor lists for one source: this file's variables (editable, never library copies)
/// or an enabled library's published ones (read-only).
struct View<'a> {
    vars: &'a Variables,
    /// The library's (id, name); None for this file.
    library: Option<(&'a str, &'a str)>,
    collections: Vec<&'a Collection>,
    variables: Vec<&'a Variable>,
}

impl<'a> View<'a> {
    fn local(doc: &'a Document) -> Self {
        View {
            vars: &doc.variables,
            library: None,
            collections: library::local_collections(doc).collect(),
            variables: doc.variables.variables.iter().filter(|v| !library::is_remote_variable(doc, &v.id)).collect(),
        }
    }

    fn of(lib: &'a PublishedLibrary) -> Self {
        View {
            vars: &lib.doc.variables,
            library: Some((&lib.manifest.id, &lib.manifest.name)),
            collections: library::published_collections(lib).collect(),
            variables: library::published_variables(lib).collect(),
        }
    }

    fn id(&self) -> Option<&'a str> {
        self.library.map(|l| l.0)
    }

    fn editable(&self) -> bool {
        self.library.is_none()
    }

    /// Whether the editor's `library` (id or name) names this view.
    fn is(&self, library: Option<&str>) -> bool {
        match (self.library, library) {
            (None, None) => true,
            (Some((id, name)), Some(l)) => l == id || l == name,
            _ => false,
        }
    }

    /// Collection `c` (id or name).
    fn collection(&self, c: &str) -> Option<&'a Collection> {
        self.collections.iter().copied().find(|x| x.id == c).or_else(|| self.collections.iter().copied().find(|x| x.name == c))
    }

    fn in_collection(&self, c: &str) -> impl Iterator<Item = &'a Variable> {
        self.variables.iter().copied().filter(move |v| v.collection == c)
    }
}

/// Show the editor while it's open (call once per frame).
pub fn show(app: &mut LizardApp, ctx: &Context) {
    let Some(mut st) = app.ui.variables.take() else { return };
    let Ok(doc) = app.session.ds().map(|d| d.doc.clone()) else { return };
    let libs = crate::variable_ui::libraries(app, &doc);
    let views: Vec<View> = std::iter::once(View::local(&doc)).chain(libs.iter().map(|l| View::of(l))).collect();
    // The selected collection (its source's first when none is), else this file's first.
    let found = views.iter().find(|v| v.is(st.library.as_deref())).and_then(|v| {
        let c = match st.collection.as_deref() {
            Some(c) => v.collection(c),
            None => v.collections.first().copied(),
        };
        Some((v, c?))
    });
    let Some((view, coll)) = found.map(|(v, c)| (v, Some(c))).or_else(|| views.first().map(|v| (v, v.collections.first().copied()))) else {
        app.ui.variables = Some(st);
        return;
    };
    st.library = view.id().map(str::to_string);
    st.collection = coll.map(|c| c.id.clone());
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let available = vec2((screen.width() - 32.0).max(320.0), (screen.height() - 32.0).max(240.0));
    let size = if st.expanded { available } else { vec2(1131.0_f32.min(available.x), 491.0_f32.min(available.y)) };
    let mut open = true;
    egui::Window::new(tr("Variables"))
        .id(Id::new("variables_window"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .fixed_size(size)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(screen.center())
        .frame(Surface::palette().frame(t).inner_margin(egui::Margin::same(0)))
        .show(ctx, |ui| {
            ui.set_min_size(size);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            header(ui, &mut st, &mut open);
            let h = (size.y - 40.0).max(160.0);
            let top_down = egui::Layout::top_down(egui::Align::Min);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(vec2(SIDEBAR_W, h), top_down, |ui| sidebar(app, ui, &views, &doc.variables, &mut st));
                let (line, _) = ui.allocate_exact_size(vec2(1.0, h), Sense::hover());
                ui.painter().rect_filled(line, CornerRadius::ZERO, t.border);
                ui.allocate_ui_with_layout(vec2(size.x - SIDEBAR_W - 1.0, h), top_down, |ui| table(app, ui, view, &mut st));
            });
            Surface::palette().paint_top_highlight(ui.painter(), ui.max_rect());
        });
    details(app, ctx, &doc.variables, &mut st);
    app.ui.variables = open.then_some(st);
}

fn header(ui: &mut Ui, st: &mut Editor, open: &mut bool) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, tr("Variables"), semibold(11.0), t.text);
    let close = Rect::from_center_size(pos2(r.max.x - 20.0, r.center().y), vec2(24.0, 24.0));
    let expand = close.translate(vec2(-28.0, 0.0));
    let filter = close.translate(vec2(-56.0, 0.0));
    if r.width() >= 400.0 {
        let search = Rect::from_min_size(pos2(filter.min.x - 176.0, r.min.y + 8.0), vec2(175.0, 24.0));
        let mut search_ui = ui.new_child(UiBuilder::new().max_rect(search));
        SearchField::new(Id::new("vars_search"), tr("Search"))
            .compact()
            .width(ControlWidth::Fixed(search.width()))
            .show(&mut search_ui, &mut st.query);
    }
    let filter_resp = Button::icon(Icon::Adjust, tr("Filter variables"))
        .variant(ButtonVariant::Secondary)
        .behavior(ButtonBehavior::Toggle { selected: st.kind_filter.is_some() })
        .icon_size(14.0)
        .show_at(ui, Id::new("vars_filter"), filter);
    crate::right::menu_popup(&filter_resp).width(160.0).show(|ui| {
        if menu_check(ui, st.kind_filter.is_none(), tr("All"), "") {
            st.kind_filter = None;
            ui.close();
        }
        for (kind, label) in [
            (VariableType::Color, tr("Color")),
            (VariableType::Number, tr("Number")),
            (VariableType::String, tr("String")),
            (VariableType::Boolean, tr("Boolean")),
        ] {
            if menu_check(ui, st.kind_filter == Some(kind), label, "") {
                st.kind_filter = Some(kind);
                ui.close();
            }
        }
    });
    if Button::icon(if st.expanded { Icon::WinRestore } else { Icon::WinMaximize }, tr("Expand variables"))
        .icon_size(12.0)
        .show_at(ui, Id::new("vars_expand"), expand)
        .clicked()
    {
        st.expanded = !st.expanded;
    }
    if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, Id::new("vars_close"), close).clicked() {
        *open = false;
    }
    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, t.border));
    ui.painter().line_segment([pos2(r.min.x + SIDEBAR_W, r.min.y), pos2(r.min.x + SIDEBAR_W, r.max.y)], Stroke::new(1.0, t.border));
}

/// Export this file's collections through a save dialog, or as a download where the platform has
/// no save dialogs (the browser).
pub(crate) fn export_dialog(app: &mut LizardApp, format: &str) {
    let name = app.session.doc().map(|d| format!("{}.variables.{format}", d.name)).unwrap_or_else(|_| format!("variables.{format}"));
    if app.services.downloads() {
        let Some(r) = app.run("variable.export", json!({"format": format})) else { return };
        let text = r["text"].as_str().unwrap_or_default();
        if app.services.download(&name, text.as_bytes()) {
            app.toast(trf("Exported {file}", &[("file", &name)]));
        } else {
            app.toast(tr("Couldn't download the variables"));
        }
        return;
    }
    let format = format.to_owned();
    let ext = format.clone();
    app.pick_save(tr("Export variables"), &name, &[&ext], move |app, p| {
        if app.run("variable.export", json!({"format": format, "path": p.to_string_lossy()})).is_some() {
            let file = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            app.toast(trf("Exported {file}", &[("file", &file)]));
        }
    });
}

/// Import a variables JSON picked in an open dialog.
fn import_dialog(app: &mut LizardApp) {
    app.pick_open(tr("Import variables"), &["json"], |app, p| {
        let Some(r) = app.run("variable.import", json!({"variables": p.to_string_lossy()})) else { return };
        let (added, updated) = (r["variables"].as_u64().unwrap_or(0), r["updated"].as_u64().unwrap_or(0));
        let skipped = r["warnings"].as_array().map_or(0, Vec::len);
        let mut msg = trn("Imported {n} new variables, updated {updated}", added as usize, &[("updated", &updated)]);
        if skipped > 0 {
            msg = format!("{msg} {}", trn("({n} warnings: some entries were skipped)", skipped, &[]));
        }
        app.toast(msg);
    });
}

/// Extend library `lib`'s collection `c` and show the new collection.
fn extend(app: &mut LizardApp, lib: &str, c: &str, st: &mut Editor) {
    if let Some(id) = app.run("library.extendCollection", json!({"library": lib, "collection": c})).and_then(|r| r["id"].as_str().map(str::to_string))
    {
        st.library = None;
        st.collection = Some(id);
        st.group = None;
        st.query.clear();
    }
}

fn rename_id(target: &Rename) -> Id {
    Id::new(("var_rename", target))
}

/// Start renaming `target`. The field isn't drawn yet (a new collection, mode or variable shows next
/// frame), and focusing it now would put a node that isn't there in the screen reader tree, which
/// AccessKit aborts on (#224): the field takes the focus when drawn.
fn start_rename(st: &mut Editor, target: Rename) {
    st.rename = Some(target);
    st.rename_start = true;
}

/// The inline editor of a rename in progress: the new name once committed. The rename ends when
/// the editor closes.
fn rename_field(ui: &mut Ui, st: &mut Editor, target: &Rename, value: &str, rect: Rect, font: egui::FontId) -> Option<String> {
    let start = std::mem::take(&mut st.rename_start);
    let (out, open) = widgets::rename_field(ui, rename_id(target), value, rect, font, start);
    if !open {
        st.rename = None;
    }
    out
}

/// The first of `base`, `base 2`, `base 3`… that `taken` doesn't hold.
fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    (1..10_000).map(|k| if k == 1 { base.to_string() } else { format!("{base} {k}") }).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

fn sidebar(app: &mut LizardApp, ui: &mut Ui, views: &[View], local: &Variables, st: &mut Editor) {
    let t = Tokens::get(ui.ctx());
    let (hr, _) = ui.allocate_exact_size(vec2(SIDEBAR_W, 40.0), Sense::hover());
    ui.painter().text(pos2(hr.min.x + 16.0, hr.center().y), Align2::LEFT_CENTER, tr("Collections"), semibold(11.0), t.text);
    let options = Button::icon(Icon::Adjust, tr("Collection options")).icon_size(14.0).show_at(
        ui,
        Id::new("vars_collection_options"),
        Rect::from_center_size(pos2(hr.max.x - 48.0, hr.center().y), vec2(24.0, 24.0)),
    );
    crate::right::menu_popup(&options).width(190.0).show(|ui| {
        if menu_item(ui, tr("Import…"), "") {
            import_dialog(app);
            ui.close();
        }
        crate::primitives::Menu::separator(ui, Some(tr("Export")));
        for (label, format) in [(tr("Variables JSON…"), "json"), (tr("CSS custom properties…"), "css")] {
            if menu_item(ui, label, "") {
                export_dialog(app, format);
                ui.close();
            }
        }
    });
    if Button::icon(Icon::Plus, tr("Create collection"))
        .icon_size(14.0)
        .show_at(ui, Id::new("vars_add_coll"), Rect::from_center_size(pos2(hr.max.x - 20.0, hr.center().y), vec2(24.0, 24.0)))
        .clicked()
    {
        create_collection(app, local, st);
    }
    let collection_rows: f32 = views
        .iter()
        .map(|view| view.collections.len() as f32 * 32.0 + if view.library.is_some() && !view.collections.is_empty() { 34.0 } else { 0.0 })
        .sum();
    let collections_height = (collection_rows + 8.0).min((ui.available_height() * 0.5).max(72.0));
    egui::ScrollArea::vertical().id_salt("vars_collections").max_height(collections_height).auto_shrink([false, false]).show(ui, |ui| {
        for view in views {
            if let Some((_, name)) = view.library
                && !view.collections.is_empty()
            {
                library_title(ui, name);
            }
            for c in &view.collections {
                collection_row(app, ui, view, c, st);
            }
        }
    });
    ui.add_space(8.0);
    let (divider, _) = ui.allocate_exact_size(vec2(SIDEBAR_W, 1.0), Sense::hover());
    ui.painter().rect_filled(divider, CornerRadius::ZERO, t.border);
    let (groups_header, _) = ui.allocate_exact_size(vec2(SIDEBAR_W, 40.0), Sense::hover());
    ui.painter().text(pos2(groups_header.min.x + 16.0, groups_header.center().y), Align2::LEFT_CENTER, tr("Groups"), semibold(11.0), t.text);
    egui::ScrollArea::vertical().id_salt("vars_groups").auto_shrink([false, false]).show(ui, |ui| {
        if let Some((view, coll)) = views.iter().find_map(|view| {
            view.is(st.library.as_deref()).then(|| st.collection.as_deref().and_then(|id| view.collection(id)).map(|coll| (view, coll))).flatten()
        }) {
            group_rows(app, ui, view, coll, st);
        }
    });
}

/// A library's heading in the sidebar: its collections follow, read-only.
fn library_title(ui: &mut Ui, name: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(8.0);
    let (r, resp) = ui.allocate_exact_size(vec2(SIDEBAR_W, 26.0), Sense::hover());
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 22.0, r.center().y), vec2(12.0, 12.0)), Icon::Library, t.icon_secondary);
    let name_r = Rect::from_min_max(pos2(r.min.x + 34.0, r.min.y), pos2(r.max.x - 8.0, r.max.y));
    ui.painter().with_clip_rect(name_r).text(name_r.left_center(), Align2::LEFT_CENTER, name, semibold(11.0), t.text_secondary);
    resp.on_hover_text(tr("Library: its variables are read-only here; edit them in the library file and publish it"));
}

/// One collection in the sidebar (with its groups while selected).
fn collection_row(app: &mut LizardApp, ui: &mut Ui, view: &View, c: &Collection, st: &mut Editor) {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(SIDEBAR_W, 32.0), Sense::click());
    let r = r.shrink2(vec2(10.0, 4.0));
    let selected = view.is(st.library.as_deref()) && st.collection.as_deref() == Some(c.id.as_str());
    if selected {
        ui.painter().rect_filled(r, CornerRadius::same(5), t.field);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(5), t.hover);
    }
    let target = Rename::Collection(c.id.clone());
    let count = view.in_collection(&c.id).count();
    ui.painter().text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, count.to_string(), regular(11.0), t.text_tertiary);
    let indent = if view.editable() { 8.0 } else { 26.0 };
    let name_r = Rect::from_min_max(pos2(r.min.x + indent, r.min.y + 4.0), pos2(r.max.x - 32.0, r.max.y - 4.0));
    if view.editable() && st.rename.as_ref() == Some(&target) {
        if let Some(name) = rename_field(ui, st, &target, &c.name, name_r, regular(11.0)) {
            app.run("variable.renameCollection", json!({"collection": c.id, "name": name}));
        }
    } else {
        let font = if selected { semibold(11.0) } else { regular(11.0) };
        ui.painter().with_clip_rect(name_r).text(name_r.left_center(), Align2::LEFT_CENTER, &c.name, font, t.text);
    }
    if resp.clicked() {
        if !selected {
            st.group = None;
            st.query.clear();
        }
        st.library = view.id().map(str::to_string);
        st.collection = Some(c.id.clone());
    }
    if view.editable() {
        if resp.double_clicked() {
            start_rename(st, target.clone());
        }
        crate::right::context_popup(&resp).show(|ui| {
            if menu_item(ui, tr("Rename"), "") {
                start_rename(st, target.clone());
                ui.close();
            }
            if menu_item(ui, tr("Delete collection"), "") {
                app.run("variable.deleteCollection", json!({"collection": c.id}));
                ui.close();
            }
        });
    } else if let Some(lib) = view.id() {
        crate::right::context_popup(&resp).show(|ui| {
            if menu_item(ui, tr("Extend collection"), "") {
                extend(app, lib, &c.id, st);
                ui.close();
            }
        });
    }
}

/// Every group path in `names` with its ancestors ("a/b/c" → "a", "a/b"), sorted.
fn groups<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let mut path = group_of(name);
        while !path.is_empty() {
            if !out.iter().any(|g| g == path) {
                out.push(path.to_string());
            }
            path = group_of(path);
        }
    }
    out.sort();
    out
}

/// "All variables" and the collection's groups (indented by depth): picking one filters the
/// table; this file's groups can be renamed (every variable in them follows) or deleted.
fn group_rows(app: &mut LizardApp, ui: &mut Ui, view: &View, coll: &Collection, st: &mut Editor) {
    let t = Tokens::get(ui.ctx());
    let members: Vec<&Variable> = view.in_collection(&coll.id).collect();
    let names: Vec<&str> = members.iter().map(|v| v.name.as_str()).collect();
    let list = groups(names.iter().copied());
    let rows = std::iter::once(None).chain(list.iter().map(Some));
    for g in rows {
        let (r, resp) = ui.allocate_exact_size(vec2(SIDEBAR_W, 32.0), Sense::click());
        let r = r.shrink2(vec2(10.0, 4.0));
        let on = st.group.as_ref() == g;
        if on {
            ui.painter().rect_filled(r, CornerRadius::same(5), t.selected_row);
        } else if resp.hovered() {
            ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, t.border), StrokeKind::Inside);
        }
        let depth = g.map_or(0, |g| g.matches('/').count());
        let x = r.min.x + 8.0 + depth as f32 * 12.0;
        let label = g.map_or(tr("All"), |g| g.rsplit('/').next().unwrap_or(g));
        let count = names.iter().filter(|n| g.is_none_or(|g| n.starts_with(&format!("{g}/")))).count();
        ui.painter().text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, count.to_string(), regular(10.0), t.text_tertiary);
        let name_r = Rect::from_min_max(pos2(x, r.min.y + 3.0), pos2(r.max.x - 28.0, r.max.y - 3.0));
        let Some(g) = g else {
            ui.painter().text(name_r.left_center(), Align2::LEFT_CENTER, label, regular(11.0), t.text_secondary);
            if resp.clicked() {
                st.group = None;
            }
            continue;
        };
        let target = Rename::Group(coll.id.clone(), g.clone());
        if view.editable() && st.rename.as_ref() == Some(&target) {
            if let Some(new) = rename_field(ui, st, &target, label, name_r, regular(11.0)) {
                rename_group(app, &members, g, &new, st);
            }
        } else {
            ui.painter().with_clip_rect(name_r).text(name_r.left_center(), Align2::LEFT_CENTER, label, regular(11.0), t.text);
        }
        if resp.clicked() {
            st.group = Some(g.clone());
        }
        if !view.editable() {
            continue;
        }
        if resp.double_clicked() {
            start_rename(st, target.clone());
        }
        crate::right::context_popup(&resp).show(|ui| {
            if menu_item(ui, tr("Rename group"), "") {
                start_rename(st, target.clone());
                ui.close();
            }
            if menu_item(ui, tr("Delete group"), "") {
                let steps: Vec<serde_json::Value> = members
                    .iter()
                    .filter(|v| v.name.starts_with(&format!("{g}/")))
                    .map(|v| json!({"command": "variable.delete", "params": {"variable": v.id}}))
                    .collect();
                app.run("command.batch", json!({"label": "Delete group", "commands": steps}));
                if st.group.as_ref().is_some_and(|s| s == g || s.starts_with(&format!("{g}/"))) {
                    st.group = None;
                }
                ui.close();
            }
        });
    }
}

/// Rename the last part of group `g` of a collection (its variables `members`) to `new`: every
/// variable under it moves along (one undo step).
fn rename_group(app: &mut LizardApp, members: &[&Variable], g: &str, new: &str, st: &mut Editor) {
    let new = new.trim().replace('/', "-");
    if new.is_empty() {
        return;
    }
    let renamed = match g.rsplit_once('/') {
        Some((parent, _)) => format!("{parent}/{new}"),
        None => new,
    };
    let prefix = format!("{g}/");
    let steps: Vec<serde_json::Value> = members
        .iter()
        .filter(|v| v.name.starts_with(&prefix))
        .map(|v| json!({"command": "variable.update", "params": {"variable": v.id, "name": format!("{renamed}/{}", &v.name[prefix.len()..])}}))
        .collect();
    if app.run("command.batch", json!({"label": "Rename group", "commands": steps})).is_some()
        && st.group.as_deref().is_some_and(|s| s == g || s.starts_with(&prefix))
    {
        st.group = st.group.as_ref().map(|s| format!("{renamed}{}", &s[g.len()..]));
    }
}

fn create_collection(app: &mut LizardApp, vars: &Variables, st: &mut Editor) {
    let name = unique("Collection", |n| vars.collections.iter().any(|c| c.name == n));
    if let Some(id) =
        app.run("variable.createCollection", json!({"name": name, "modes": ["Mode 1"]})).and_then(|r| r["id"].as_str().map(str::to_string))
    {
        st.library = None;
        st.collection = Some(id.clone());
        start_rename(st, Rename::Collection(id));
    }
}

/// The group part of a variable name ("space/md" → "space"; "" when ungrouped).
fn group_of(name: &str) -> &str {
    name.rsplit_once('/').map(|(g, _)| g).unwrap_or("")
}

fn table(app: &mut LizardApp, ui: &mut Ui, view: &View, st: &mut Editor) {
    let t = Tokens::get(ui.ctx());
    // A sidebar click this frame picked another source: shown next frame.
    if !view.is(st.library.as_deref()) {
        ui.ctx().request_repaint();
        return;
    }
    let Some(coll) = st.collection.as_deref().and_then(|c| view.collection(c)) else {
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.vertical(|ui| {
                widgets::label(
                    ui,
                    tr("Variables store reusable values (colours, numbers, strings, booleans) with modes such as Light and Dark."),
                    t.text_secondary,
                );
                ui.add_space(12.0);
                if Button::new(tr("Create collection")).show(ui).clicked() {
                    create_collection(app, view.vars, st);
                }
            });
        });
        return;
    };
    let q = st.query.trim().to_lowercase();
    let in_group = |v: &Variable| st.group.as_ref().is_none_or(|g| v.name.starts_with(&format!("{g}/")));
    let mut list: Vec<&Variable> = view
        .in_collection(&coll.id)
        .filter(|v| in_group(v) && st.kind_filter.is_none_or(|kind| v.kind == kind) && (q.is_empty() || v.name.to_lowercase().contains(&q)))
        .collect();
    list.sort_by(|a, b| group_of(&a.name).cmp(group_of(&b.name)));
    let width = (NAME_W + coll.modes.len() as f32 * MODE_W + 40.0).max(ui.available_width());
    let table_height = (ui.available_height() - 41.0).max(80.0);
    egui::ScrollArea::both().id_salt("vars_table").auto_shrink([false, false]).max_height(table_height).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        header_row(app, ui, view, coll, st, width);
        for v in &list {
            var_row(app, ui, view, coll, v, st, width);
        }
        if list.is_empty() && (!q.is_empty() || st.kind_filter.is_some()) {
            let (r, _) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
            ui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, tr("No matching variables"), regular(11.0), t.text_secondary);
        }
    });
    create_row(app, ui, view, coll, st);
}

fn header_row(app: &mut LizardApp, ui: &mut Ui, view: &View, coll: &Collection, st: &mut Editor, width: f32) {
    let t = Tokens::get(ui.ctx());
    // An extended collection has the modes of the one it extends.
    let editable = view.editable() && coll.extends.is_none();
    let (r, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    ui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, tr("Name"), semibold(11.0), t.text);
    for (i, m) in coll.modes.iter().enumerate() {
        let x = r.min.x + NAME_W + i as f32 * MODE_W;
        let cell = Rect::from_min_max(pos2(x + 12.0, r.min.y + 4.0), pos2(x + MODE_W - 8.0, r.max.y - 4.0));
        let target = Rename::Mode(coll.id.clone(), m.id.clone());
        if editable && st.rename.as_ref() == Some(&target) {
            if let Some(name) = rename_field(ui, st, &target, &m.name, cell.shrink2(vec2(4.0, 2.0)), regular(11.0)) {
                app.run("variable.renameMode", json!({"collection": coll.id, "mode": m.id, "name": name}));
            }
            continue;
        }
        let name_pos = pos2(cell.min.x + 4.0, cell.center().y);
        let label = if coll.modes.len() == 1 { tr("Value") } else { &m.name };
        if !editable {
            ui.painter().with_clip_rect(cell).text(name_pos, Align2::LEFT_CENTER, label, semibold(11.0), t.text);
            continue;
        }
        let resp = ui.interact(cell, Id::new(("vars_mode", &m.id)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(cell, CornerRadius::same(4), t.hover);
        }
        ui.painter().with_clip_rect(cell).text(name_pos, Align2::LEFT_CENTER, label, semibold(11.0), t.text);
        if resp.double_clicked() {
            start_rename(st, target.clone());
        }
        crate::right::context_popup(&resp).show(|ui| {
            if menu_item(ui, tr("Rename mode"), "") {
                start_rename(st, target.clone());
                ui.close();
            }
            if menu_item(ui, tr("Duplicate mode"), "") {
                let name = unique(&format!("{} copy", m.name), |n| coll.modes.iter().any(|x| x.name == n));
                let made = app.run("variable.addMode", json!({"collection": coll.id, "name": name, "from": m.id}));
                if let Some(id) = made.and_then(|r| r["id"].as_str().map(str::to_string)) {
                    start_rename(st, Rename::Mode(coll.id.clone(), id));
                }
                ui.close();
            }
            // The default mode is the first column: moving a column changes it.
            let last = coll.modes.len() - 1;
            for (show, label, to) in [
                (i > 0, tr_noop("Set as default"), 0),
                (i > 0, tr_noop("Move column left"), i.saturating_sub(1)),
                (i < last, tr_noop("Move column right"), i + 1),
            ] {
                if show && menu_item(ui, tr(label), "") {
                    app.run("variable.moveMode", json!({"collection": coll.id, "mode": m.id, "to": to}));
                    ui.close();
                }
            }
            if coll.modes.len() > 1 && menu_item(ui, tr("Delete mode"), "") {
                app.run("variable.removeMode", json!({"collection": coll.id, "mode": m.id}));
                ui.close();
            }
        });
    }
    if editable
        && Button::icon(Icon::Plus, tr("Add mode"))
            .icon_size(14.0)
            .show_at(ui, Id::new("vars_add_mode"), Rect::from_center_size(pos2(r.max.x - 20.0, r.center().y), vec2(24.0, 24.0)))
            .clicked()
    {
        let name = unique("Mode", |n| coll.modes.iter().any(|m| m.name == n));
        if let Some(id) = app.run("variable.addMode", json!({"collection": coll.id, "name": name})).and_then(|r| r["id"].as_str().map(str::to_string))
        {
            start_rename(st, Rename::Mode(coll.id.clone(), id));
        }
    }
    table_grid(ui, r, coll.modes.len());
}

fn table_grid(ui: &Ui, row: Rect, mode_count: usize) {
    let stroke = Stroke::new(1.0, Tokens::get(ui.ctx()).border);
    ui.painter().line_segment([row.left_bottom(), row.right_bottom()], stroke);
    for index in 0..=mode_count {
        let x = row.left() + NAME_W + index as f32 * MODE_W;
        if x < row.right() - 40.0 {
            ui.painter().line_segment([pos2(x, row.top()), pos2(x, row.bottom())], stroke);
        }
    }
    let trailing = row.right() - 40.0;
    ui.painter().line_segment([pos2(trailing, row.top()), pos2(trailing, row.bottom())], stroke);
}

fn var_row(app: &mut LizardApp, ui: &mut Ui, view: &View, coll: &Collection, v: &Variable, st: &mut Editor, width: f32) {
    let t = Tokens::get(ui.ctx());
    let editable = view.editable();
    let (r, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    if resp.hovered() || (editable && st.details.as_deref() == Some(v.id.as_str())) {
        ui.painter().rect_filled(r, CornerRadius::ZERO, t.hover);
    }
    kind_glyph(ui, view.vars, v, coll, Rect::from_center_size(pos2(r.min.x + 24.0, r.center().y), vec2(14.0, 14.0)));
    let name_r = Rect::from_min_max(pos2(r.min.x + 44.0, r.min.y + 8.0), pos2(r.min.x + NAME_W - 8.0, r.max.y - 8.0));
    let target = Rename::Variable(v.id.clone());
    // An inherited variable is named (and deleted) in the collection it extends.
    let inherited = v.extends.is_some();
    if editable && st.rename.as_ref() == Some(&target) {
        if let Some(name) = rename_field(ui, st, &target, &v.name, name_r, regular(11.0)) {
            app.run("variable.update", json!({"variable": v.id, "name": name}));
        }
    } else {
        // Grouped names show their last part under the group's header.
        let short = v.name.rsplit('/').next().unwrap_or(&v.name);
        ui.painter().with_clip_rect(name_r).text(name_r.left_center(), Align2::LEFT_CENTER, short, regular(11.0), t.text);
    }
    for (i, m) in coll.modes.iter().enumerate() {
        let x = r.min.x + NAME_W + i as f32 * MODE_W;
        let cell = Rect::from_min_max(pos2(x + 16.0, r.center().y - 12.0), pos2(x + MODE_W - 8.0, r.center().y + 12.0));
        value_cell(app, ui, view, v, coll, &m.id, cell);
    }
    if editable {
        if !inherited && resp.double_clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| p.x < name_r.max.x) {
            start_rename(st, target.clone());
        }
        crate::right::context_popup(&resp).show(|ui| {
            if menu_item(ui, tr("Edit variable"), "") {
                st.details = Some(v.id.clone());
                ui.close();
            }
            if !inherited && menu_item(ui, tr("Rename"), "") {
                start_rename(st, target.clone());
                ui.close();
            }
            if !v.overrides.is_empty() && menu_item(ui, tr("Reset overrides"), "") {
                app.run("variable.resetOverride", json!({"variable": v.id}));
                ui.close();
            }
            if menu_item(ui, tr("Duplicate"), "") {
                app.run("variable.duplicate", json!({"variable": v.id}));
                ui.close();
            }
            if !inherited && menu_item(ui, tr("Delete variable"), "") {
                app.run("variable.delete", json!({"variable": v.id}));
                ui.close();
            }
        });
    }
    table_grid(ui, r, coll.modes.len());
}

/// The type marker before a variable's name: its colour (default mode), or a type glyph.
fn kind_glyph(ui: &Ui, _vars: &Variables, v: &Variable, _coll: &Collection, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let icon = match v.kind {
        VariableType::Color => Icon::VariableColor,
        VariableType::Number => Icon::VariableNumber,
        VariableType::String => Icon::VariableString,
        VariableType::Boolean => Icon::VariableBoolean,
    };
    icons::paint(ui.painter(), rect, icon, t.icon);
}

/// `val` as a `variable.set` value.
fn value_json(val: &VariableValue) -> Value {
    match val {
        VariableValue::Color(c) => json!(c.to_css_hex()),
        VariableValue::Number(x) => json!(x),
        VariableValue::String(s) => json!(s),
        VariableValue::Boolean(b) => json!(b),
        VariableValue::Alias(a) => json!({"alias": a}),
    }
}

fn set(app: &mut LizardApp, v: &Variable, mode: &str, value: Value) {
    app.run("variable.set", json!({"variable": v.id, "mode": mode, "value": value}));
}

/// One value cell: an editor for the value, or the aliased variable's chip; the variable button
/// on the right makes it an alias. A library's values only show.
fn value_cell(app: &mut LizardApp, ui: &mut Ui, view: &View, v: &Variable, coll: &Collection, mode: &str, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let Some(val) = v.values.get(mode) else { return };
    let editable = view.editable();
    let id = Id::new(("var_cell", &v.id, mode));
    // The value editor leaves room for the alias button (and in extended collections an
    // override's reset button, so the column stays even).
    let overridden = v.overrides.contains(mode);
    let body = Rect::from_min_max(rect.min, pos2(rect.max.x - if coll.extends.is_some() { 50.0 } else { 26.0 }, rect.max.y));
    let alias_btn = Rect::from_center_size(pos2(rect.max.x - 11.0, rect.center().y), vec2(22.0, 22.0));
    let hovered = editable && ui.rect_contains_pointer(rect.expand2(vec2(4.0, 6.0)));
    if overridden {
        let mark = Rect::from_min_size(pos2(rect.min.x - 4.0, rect.min.y + 3.0), vec2(2.0, rect.height() - 6.0));
        ui.painter().rect_filled(mark, CornerRadius::same(1), t.accent);
        ui.interact(mark.expand2(vec2(2.0, 0.0)), id.with("mark"), Sense::hover())
            .tip(tr("Overridden here (the collection it extends has another value)"));
        if hovered
            && Button::icon(Icon::Reset, tr("Reset to the inherited value"))
                .icon_size(14.0)
                .show_at(ui, id.with("reset"), alias_btn.translate(vec2(-24.0, 0.0)))
                .clicked()
        {
            app.run("variable.resetOverride", json!({"variable": v.id, "mode": mode}));
        }
    }
    if let VariableValue::Alias(target) = val {
        let full = view.vars.get(target).map(|x| x.name.as_str()).unwrap_or(tr("Missing variable"));
        let resp = AliasValue::new(id.with("alias"), full).editable(editable).show_at(ui, body);
        let resp = if editable { resp.on_hover_text(tr("Change alias")) } else { resp };
        if resp.clicked() {
            crate::variable_ui::open_alias(ui, &v.id, mode, v.kind, body.left_bottom());
        }
        // Detach: keep the value it resolves to now.
        if hovered && Button::icon(Icon::Detach, tr("Detach alias")).icon_size(14.0).show_at(ui, id.with("detach"), alias_btn).clicked() {
            let modes = [(coll.id.clone(), mode.to_string())].into_iter().collect();
            if let Some(resolved) = view.vars.resolve(&v.id, &modes) {
                set(app, v, mode, value_json(&resolved));
            }
        }
        return;
    }
    if !editable {
        read_only_value(ui, val, body);
        return;
    }
    match val {
        VariableValue::Color(c) => {
            let paint = lizard_color::Paint::solid(*c);
            let hex = c.to_hex();
            let edit = ColorControl::new(id, &paint, ColorValue::EditableHex(&hex), f64::from(c.a) * 100.0).inline().show_at(ui, body);
            if edit.open_picker {
                app.ui.picker = Some(PickerTarget::Variable { id: v.id.clone(), mode: mode.to_string() });
            }
            if let Some(h) = edit.edited_hex
                && let Some(nc) = lizard_color::Color::from_hex_or_name(&h)
            {
                // Six digits (or a colour name) keep the alpha; eight digits set it.
                let digits = h.trim().trim_start_matches('#');
                let nc = if digits.len() == 8 && digits.chars().all(|ch| ch.is_ascii_hexdigit()) { nc } else { nc.with_alpha(c.a) };
                set(app, v, mode, json!(nc.to_css_hex()));
            }
        }
        VariableValue::Number(x) => {
            let displayed = widgets::fmt_num(*x);
            let (edit, scrub) = InlineField::new(id.with("num"), &displayed).show_numeric_at(ui, body, *x, 1.0);
            if let Some(text) = edit
                && let Ok(value) = text.trim().parse::<f64>()
                && value.is_finite()
            {
                set(app, v, mode, json!(value));
            }
            match scrub {
                Some(FieldEdit::Scrub(value, done)) => {
                    crate::typography::live(app, "Edit variable", "variable.set", json!({"variable": v.id, "mode": mode, "value": value}), done);
                }
                Some(FieldEdit::Set(value)) => set(app, v, mode, json!(value)),
                None => {}
            }
        }
        VariableValue::String(s) => {
            if let Some(n) = InlineField::new(id.with("str"), s).show_at(ui, body) {
                set(app, v, mode, json!(n));
            }
        }
        VariableValue::Boolean(b) => {
            if Switch::new(id.with("boolean"), *b, if *b { tr("True") } else { tr("False") }).show_at(ui, body).clicked() {
                set(app, v, mode, json!(!b));
            }
        }
        VariableValue::Alias(_) => {}
    }
    if hovered && Button::icon(Icon::Variable, tr("Create alias")).icon_size(14.0).show_at(ui, id.with("mkalias"), alias_btn).clicked() {
        crate::variable_ui::open_alias(ui, &v.id, mode, v.kind, body.left_bottom());
    }
}

/// A value that only shows (a library's): its swatch and hex for colours, else its text.
fn read_only_value(ui: &Ui, val: &VariableValue, body: Rect) {
    let t = Tokens::get(ui.ctx());
    let mut x = body.min.x;
    if let VariableValue::Color(c) = val {
        let sw = Rect::from_center_size(pos2(body.min.x + 8.0, body.center().y), vec2(16.0, 16.0));
        widgets::paint_swatch(ui.painter(), sw, &lizard_color::Paint::solid(*c));
        x = sw.max.x + 4.0;
    }
    let text = match val {
        VariableValue::Boolean(b) => (if *b { tr("True") } else { tr("False") }).to_string(),
        other => other.display(),
    };
    let text_r = Rect::from_min_max(pos2(x, body.min.y), body.max);
    ui.painter().with_clip_rect(text_r).text(text_r.left_center(), Align2::LEFT_CENTER, text, regular(11.0), t.text);
}

fn create_row(app: &mut LizardApp, ui: &mut Ui, view: &View, coll: &Collection, st: &mut Editor) {
    let (footer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 41.0), Sense::hover());
    ui.painter().line_segment([footer.left_top(), footer.right_top()], Stroke::new(1.0, Tokens::get(ui.ctx()).border));
    if view.editable() {
        let button = Rect::from_min_size(pos2(footer.left() + 8.0, footer.top() + 8.0), vec2(142.0, 24.0));
        let resp = Button::new(tr("+ Create variable").trim_start_matches("+ ")).variant(ButtonVariant::Ghost).leading(Icon::Plus).show_at(
            ui,
            Id::new("vars_create_variable"),
            button,
        );
        crate::right::menu_popup(&resp).show(|ui| {
            for kind in TYPES {
                let label = match kind {
                    // The English label also names the new variable (document content).
                    VariableType::Color => tr_noop("Color"),
                    VariableType::Number => tr_noop("Number"),
                    VariableType::String => tr_noop("String"),
                    VariableType::Boolean => tr_noop("Boolean"),
                };
                if menu_item(ui, tr(label), "") {
                    let base = st.group.as_ref().map_or_else(|| label.to_string(), |group| format!("{group}/{label}"));
                    let name = unique(&base, |n| view.in_collection(&coll.id).any(|v| v.name == n));
                    let params = json!({"collection": coll.id, "name": name, "type": kind.id()});
                    if let Some(id) = app.run("variable.create", params).and_then(|r| r["id"].as_str().map(str::to_string)) {
                        st.query.clear();
                        start_rename(st, Rename::Variable(id));
                    }
                    ui.close();
                }
            }
        });
    } else if let Some((library, _)) = view.library {
        let button = Rect::from_min_size(pos2(footer.left() + 8.0, footer.top() + 8.0), vec2(142.0, 24.0));
        if Button::new(tr("Extend collection"))
            .variant(ButtonVariant::Ghost)
            .leading(Icon::Plus)
            .show_at(ui, Id::new("vars_extend"), button)
            .clicked()
        {
            extend(app, library, &coll.id, st);
        }
    }
}

/// The details of one of this file's variables: name, description, where pickers offer it and
/// whether publishing the file as a library leaves it out.
fn details(app: &mut LizardApp, ctx: &Context, vars: &Variables, st: &mut Editor) {
    let Some(v) = st.details.as_ref().filter(|_| st.library.is_none()).and_then(|id| vars.get(id)).cloned() else {
        st.details = None;
        return;
    };
    let t = Tokens::get(ctx);
    let mut open = true;
    egui::Window::new(tr("Edit variable"))
        .id(Id::new("variable_details"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(280.0)
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.panel).inner_margin(egui::Margin::same(16)))
        .show(ctx, |ui| {
            ui.set_width(260.0);
            widgets::label(ui, tr("Name"), t.text_secondary);
            if let Some(n) = widgets::text_field(ui, Id::new(("var_detail_name", &v.id)), &v.name, 260.0, "") {
                app.run("variable.update", json!({"variable": v.id, "name": n}));
            }
            ui.add_space(6.0);
            widgets::label(ui, tr("Description"), t.text_secondary);
            if let Some(d) = widgets::text_field(ui, Id::new(("var_detail_desc", &v.id)), &v.description, 260.0, tr("What it's for")) {
                app.run("variable.update", json!({"variable": v.id, "description": d}));
            }
            ui.add_space(6.0);
            if widgets::checkbox(ui, v.hidden_from_publishing, tr("Hide from publishing")) {
                app.run("variable.update", json!({"variable": v.id, "hiddenFromPublishing": !v.hidden_from_publishing}));
            }
            // The names code uses for it per platform (generated CSS uses the Web one).
            ui.add_space(10.0);
            widgets::label(ui, tr("Code syntax"), t.text_secondary);
            for (key, label) in [("WEB", tr_noop("Web")), ("iOS", tr_noop("iOS")), ("ANDROID", tr_noop("Android"))] {
                ui.horizontal(|ui| {
                    let (lr, _) = ui.allocate_exact_size(vec2(60.0, 24.0), Sense::hover());
                    ui.painter().text(lr.left_center(), Align2::LEFT_CENTER, tr(label), regular(11.0), t.text);
                    let cur = v.code_syntax.get(key).map_or("", String::as_str);
                    if let Some(n) = widgets::text_field_named(ui, Id::new(("var_detail_code", key, &v.id)), cur, 192.0, tr(label)) {
                        app.run("variable.update", json!({"variable": v.id, "codeSyntax": {key: n}}));
                    }
                });
            }
            let scopes = scopes_for(v.kind);
            if scopes.is_empty() {
                return;
            }
            ui.add_space(10.0);
            widgets::label(ui, tr("Scoping: where pickers offer it"), t.text_secondary);
            // "All" off hides it everywhere; on, from everywhere it fits.
            if widgets::checkbox(ui, v.scopes.is_none(), tr("All supported properties")) {
                app.run("variable.update", json!({"variable": v.id, "scopes": if v.scopes.is_none() { json!([]) } else { Value::Null }}));
            }
            let on: Vec<&str> = scopes.iter().map(|(id, _)| *id).filter(|id| v.in_scope(id)).collect();
            for (sid, label) in scopes {
                let checked = on.contains(sid);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    if widgets::checkbox(ui, checked, tr(label)) {
                        let mut list = on.clone();
                        if checked {
                            list.retain(|x| x != sid);
                        } else {
                            list.push(sid);
                        }
                        app.run("variable.update", json!({"variable": v.id, "scopes": list}));
                    }
                });
            }
        });
    if !open {
        st.details = None;
    }
}

#[cfg(test)]
mod tests {
    use super::groups;

    #[test]
    fn groups_include_parents_once_and_sorted() {
        let names = ["space/sm", "space/md", "color/brand/blue", "color/text", "plain", "color/brand/red"];
        assert_eq!(groups(names.into_iter()), ["color", "color/brand", "space"]);
        assert!(groups(["a", "b"].into_iter()).is_empty());
    }
}
