//! Plugins in the UI: the installed plugins in the Plugins menu (widgets in Plugins ▸ Widgets), the
//! parameters form a command with parameters asks for before it runs, the panels running plugins
//! show, and the Manage plugins window (installed plugins, and a server's plugin directory to
//! browse, install from and update from). Everything runs through the engine's `plugin.*`,
//! `plugin.directory.*` and `widget.*` commands.

use egui::{Align2, Context, Id, RichText, Ui};
use figcraft_engine::plugin_directory::{PluginEntry, installed_version, newer_version};
use figcraft_engine::plugins::{Access, InstalledPlugin, Param, ParamKind};
use figcraft_figma_plugin::catalog::{self, Page};
use serde_json::{Map, Value, json};

use crate::FigApp;
use crate::colorpicker::PickerTarget;
use crate::dialogs::{floating, tab_strip, window};
use crate::i18n::{tr, trf};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets::{self, FieldEdit, Lead};

/// The parameters form of a plugin command, with the values entered so far.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginForm {
    pub id: String,
    pub command: String,
    pub values: Map<String, Value>,
}

/// The Manage plugins window's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginsTab {
    #[default]
    Installed,
    Browse,
    FigmaLocal,
    FigmaCommunity,
}

impl PluginsTab {
    pub const ALL: [PluginsTab; 4] = [PluginsTab::Installed, PluginsTab::Browse, PluginsTab::FigmaLocal, PluginsTab::FigmaCommunity];

    fn label(self) -> &'static str {
        match self {
            PluginsTab::Installed => tr("Installed"),
            PluginsTab::Browse => tr("Browse"),
            PluginsTab::FigmaLocal => "Figma local",
            PluginsTab::FigmaCommunity => "Figma Community",
        }
    }
}

/// The Manage plugins window's state: its tab, the Browse tab's fields and the plugin waiting for
/// the user to confirm its install (by id).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginsWindow {
    pub tab: PluginsTab,
    /// The server address typed (`None`: the one browsed last).
    pub server: Option<String>,
    pub query: String,
    pub confirm: Option<String>,
    /// Last inspection report for a local JavaScript plugin.
    pub figma_report: Option<Value>,
    pub figma_parameters: Map<String, Value>,
    pub figma_feedback: Map<String, Value>,
    pub figma_review_target: Option<(figcraft_doc::NodeId, String)>,
    pub community_query: String,
    pub community_page: Option<Page>,
    pub community_error: Option<String>,
    pub community_selected: Option<String>,
    pub community_from: usize,
    pub community_loading: bool,
}

/// A local HTML plugin window. The desktop shell owns the native WebView; egui reserves its
/// content rectangle and carries messages between it and engine commands.
#[derive(Clone, Debug)]
pub struct FigmaUiWindow {
    pub id: String,
    pub title: String,
    pub html: String,
    pub allowed_domains: Vec<String>,
    pub width: f32,
    pub height: f32,
    pub rect: Option<egui::Rect>,
    pub outgoing: Vec<Value>,
}

#[derive(Clone, Debug, Default)]
pub struct FigmaTextReview {
    #[cfg(not(target_arch = "wasm32"))]
    registry: Vec<(String, String)>,
    #[cfg(not(target_arch = "wasm32"))]
    plugins: Vec<String>,
    #[cfg(not(target_arch = "wasm32"))]
    document: Option<u64>,
    pub(crate) node: Option<figcraft_doc::NodeId>,
    #[cfg(not(target_arch = "wasm32"))]
    text: String,
    #[cfg(not(target_arch = "wasm32"))]
    due: f64,
    #[cfg(not(target_arch = "wasm32"))]
    resolved: bool,
    pub marks: Vec<ReviewMark>,
}

#[derive(Clone, Debug)]
pub struct ReviewMark {
    pub start: usize,
    pub end: usize,
    pub color: egui::Color32,
}

/// Width of the plugin windows' fields.
const FIELD_W: f32 = 220.0;
/// How far apart plugin panels open, side by side (a panel and a gap).
const PANEL_STEP: f32 = 372.0;

/// What the user did to a parameter or panel control.
enum Used {
    /// Changed a field to a value, or pressed a button (null).
    Set(String, Value),
    /// Asked for the colour picker for a colour field.
    Pick(String),
}

/// The value of colour field `control` of plugin `plugin`'s parameters form (when it's open) or
/// panel, as hex.
pub(crate) fn field_value(app: &FigApp, plugin: &str, control: &str) -> Option<String> {
    let values = match &app.ui.plugin_form {
        Some(f) if f.id == plugin => &f.values,
        _ => &app.session.plugins.panel(plugin)?.values,
    };
    values.get(control)?.as_str().map(str::to_string)
}

/// Where the window holding plugin `plugin`'s colour fields is (its parameters form when it's
/// open, else its panel), as last drawn.
pub(crate) fn field_window(app: &FigApp, ctx: &Context, plugin: &str) -> Option<egui::Rect> {
    let id = match &app.ui.plugin_form {
        Some(f) if f.id == plugin => Id::new("plugin_form"),
        _ => Id::new(("plugin_panel", plugin)),
    };
    ctx.memory(|m| m.area_rect(id))
}

/// Set colour field `control` of plugin `plugin`'s parameters form (when it's open), or of its
/// panel (calling the plugin back), to `hex`.
pub(crate) fn set_field(app: &mut FigApp, plugin: &str, control: &str, hex: &str) {
    match &mut app.ui.plugin_form {
        Some(f) if f.id == plugin => {
            f.values.insert(control.to_string(), json!(hex));
        }
        _ => run(app, "plugin.event", json!({"id": plugin, "control": control, "value": hex})),
    }
}

/// Start plugin `id`'s `command` (empty: its first): ask for its parameters first when it has any.
pub fn start(app: &mut FigApp, id: &str, command: &str) {
    let found = app.session.prefs.plugins.iter().find(|p| p.manifest.id == id).and_then(|p| p.manifest.command(Some(command)));
    match found {
        Some(c) if !c.params.is_empty() => {
            let values = c.params.iter().map(|p| (p.name.clone(), p.default_value())).collect();
            app.ui.plugin_form = Some(PluginForm { id: id.to_string(), command: c.id.clone(), values });
        }
        _ => run(app, "plugin.run", json!({"id": id, "command": command})),
    }
}

/// Run `plugin.run` or `plugin.event`; a text output is shown as a toast unless the plugin showed
/// a message.
fn run(app: &mut FigApp, cmd: &str, params: Value) {
    let toast = app.ui.toast.clone();
    let Some(out) = app.run(cmd, params) else { return };
    if app.ui.toast == toast
        && let Some(text) = out["output"].as_str().filter(|t| !t.trim().is_empty())
    {
        app.toast(text.chars().take(200).collect::<String>());
    }
}

/// Plugins ▸ Development ▸ Import plugin from manifest…: pick a `figcraft-plugin.json`.
pub fn import_dialog(app: &mut FigApp) {
    app.pick_open(tr("Import plugin from manifest"), &["json"], |app, p| {
        if let Some(v) = app.run("plugin.import", json!({"path": p.to_string_lossy()})) {
            app.toast(trf("Imported {name}", &[("name", &v["name"].as_str().unwrap_or(tr("the plugin")))]));
        }
    });
}

/// The installed plugins at the top of the Plugins menu: a plugin with one command is an item, one
/// with several a submenu. True when one was clicked.
pub fn menu(app: &mut FigApp, ui: &mut Ui, t: &Tokens) -> bool {
    if app.session.prefs.plugins.is_empty() {
        return false;
    }
    let mut chosen: Option<(String, String)> = None;
    for p in &app.session.prefs.plugins {
        let m = &p.manifest;
        match m.commands.as_slice() {
            // A widget may have no commands: it's in Widgets.
            [] => {}
            [only] => {
                // An ellipsis: it asks for parameters first.
                let label = if only.params.is_empty() { m.name.clone() } else { format!("{}…", m.name) };
                if crate::menus::item(ui, &label, "", true, false, t) {
                    chosen = Some((m.id.clone(), only.id.clone()));
                }
            }
            commands => {
                let picked = crate::menus::submenu(ui, &format!("Plugins/plugin:{}", m.id), &m.name, t, |ui| {
                    commands.iter().find(|c| crate::menus::item(ui, &c.label, "", true, false, t)).map(|c| c.id.clone())
                });
                if let Some(c) = picked {
                    chosen = Some((m.id.clone(), c));
                }
            }
        }
    }
    let widgets: Vec<(&str, &str)> =
        app.session.prefs.plugins.iter().filter(|p| p.manifest.widget).map(|p| (p.manifest.id.as_str(), p.manifest.name.as_str())).collect();
    let widget = (!widgets.is_empty())
        .then(|| {
            crate::menus::submenu(ui, "Plugins/widgets", tr("Widgets"), t, |ui| {
                widgets.iter().find(|(_, name)| crate::menus::item(ui, name, "", true, false, t)).map(|(id, _)| id.to_string())
            })
        })
        .flatten();
    crate::menus::separator(ui);
    if let Some(plugin) = widget {
        app.run("widget.insert", json!({"plugin": plugin}));
        return true;
    }
    let Some((id, command)) = chosen else { return false };
    start(app, &id, &command);
    true
}

/// The plugin windows (Manage plugins, a command's parameters, the plugins' panels).
pub fn show(app: &mut FigApp, ctx: &Context) {
    if app.ui.plugins_window {
        manage_window(app, ctx);
    }
    if app.ui.plugin_form.is_some() {
        form_window(app, ctx);
    }
    if !app.session.plugins.panels.is_empty() {
        panel_windows(app, ctx);
    }
    figma_ui_window(app, ctx);
}

fn figma_ui_window(app: &mut FigApp, ctx: &Context) {
    let Some(open) = app.ui.figma_ui.as_mut() else { return };
    let mut keep = true;
    let mut rect = None;
    let size = egui::vec2(open.width, open.height);
    window(&open.title, ctx).id(Id::new(("figma_plugin_ui", &open.id))).default_size(size).resizable(false).open(&mut keep).show(ctx, |ui| {
        rect = Some(ui.allocate_exact_size(size, egui::Sense::hover()).0);
    });
    if keep {
        open.rect = rect;
    } else {
        app.run("plugin.figma.ui.close", json!({}));
        app.ui.figma_ui = None;
    }
}

/// Called by the desktop WebView bridge after a plugin page posts `pluginMessage`.
pub fn figma_ui_message(app: &mut FigApp, payload: Value) {
    let Some(id) = app.ui.figma_ui.as_ref().map(|open| open.id.clone()) else { return };
    if app.run("plugin.figma.ui.message", json!({"id": id, "payload": payload})).is_none() {
        app.ui.figma_ui = None;
    }
}

pub(crate) fn accept_ui_reply(app: &mut FigApp, id: &str, reply: &Value) {
    if reply["closed"] == true {
        if let Some(message) = reply["message"].as_str().filter(|text| !text.is_empty()) {
            app.toast(message.to_string());
        }
        app.ui.figma_ui = None;
        return;
    }
    if let Some(show) = reply.get("show").filter(|v| !v.is_null())
        && let Some(html) = show["html"].as_str()
    {
        let title = show["title"].as_str().filter(|text| !text.is_empty()).unwrap_or(id).to_string();
        app.ui.figma_ui = Some(FigmaUiWindow {
            id: id.to_string(),
            title,
            html: html.to_string(),
            allowed_domains: show["allowedDomains"]
                .as_array()
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
            width: show["width"].as_f64().unwrap_or(300.0) as f32,
            height: show["height"].as_f64().unwrap_or(200.0) as f32,
            rect: None,
            outgoing: Vec::new(),
        });
    }
    if let Some(open) = app.ui.figma_ui.as_mut()
        && let Some(messages) = reply["messages"].as_array()
    {
        open.outgoing.extend(messages.iter().cloned());
    }
}

/// Run registered text-review callbacks after typing settles, then paint their returned ranges
/// over the native canvas text. The worker runs only when the text changes, never every frame.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn text_review_tick(app: &mut FigApp, ctx: &Context) {
    let registry: Vec<_> = app.session.prefs.figma_plugins.iter().map(|plugin| (plugin.id.clone(), plugin.digest.clone())).collect();
    if app.ui.figma_text_review.registry != registry {
        let plugins = registry
            .iter()
            .filter_map(|(id, _)| {
                let report = app.session.execute("plugin.figma.status", &json!({"id": id})).ok()?;
                report["capabilities"].as_array().is_some_and(|items| items.iter().any(|item| item == "textreview")).then(|| id.clone())
            })
            .collect();
        app.ui.figma_text_review = FigmaTextReview { registry, plugins, ..FigmaTextReview::default() };
    }
    let current = app.session.ds().ok().and_then(|ds| {
        let edit = ds.text_edit.as_ref()?;
        let node = ds.doc.node(edit.id)?;
        let figcraft_doc::NodeKind::Text(text) = &node.kind else { return None };
        Some((ds.uid, edit.id, text.characters.clone()))
    });
    let state = &mut app.ui.figma_text_review;
    let Some((document, node, text)) = current else {
        state.node = None;
        state.marks.clear();
        return;
    };
    if state.plugins.is_empty() || text.len() > 16_384 {
        state.marks.clear();
        return;
    }
    let now = ctx.input(|input| input.time);
    if state.document != Some(document) || state.node != Some(node) || state.text != text {
        state.document = Some(document);
        state.node = Some(node);
        state.text = text.clone();
        state.marks.clear();
        state.due = now + 0.2;
        state.resolved = false;
        ctx.request_repaint_after(std::time::Duration::from_millis(210));
        return;
    }
    if state.resolved {
        return;
    }
    if now < state.due {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((state.due - now).max(0.001)));
        return;
    }
    let plugins = state.plugins.clone();
    let mut marks = Vec::new();
    for id in plugins {
        let Ok(reply) = app.session.execute("plugin.figma.run", &json!({"id": id, "command":"textreview", "text":text})) else { continue };
        let Some(results) = reply["eventResult"].as_array() else { continue };
        for item in results.iter().take(100) {
            let (Some(start), Some(end)) = (item["start"].as_u64(), item["end"].as_u64()) else { continue };
            let (Some(start), Some(end)) = (utf16_char_index(&text, start as usize), utf16_char_index(&text, end as usize)) else { continue };
            if start >= end {
                continue;
            }
            let color = match item["color"].as_str() {
                Some("RED") => egui::Color32::from_rgb(238, 98, 98),
                Some("YELLOW") => egui::Color32::from_rgb(239, 194, 77),
                _ => egui::Color32::from_rgb(95, 199, 132),
            };
            marks.push(ReviewMark { start, end, color });
        }
    }
    app.ui.figma_text_review.marks = marks;
    app.ui.figma_text_review.resolved = true;
}

fn utf16_char_index(text: &str, offset: usize) -> Option<usize> {
    let mut units = 0;
    for (index, character) in text.chars().enumerate() {
        if units == offset {
            return Some(index);
        }
        units += character.len_utf16();
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(text.chars().count())
}

fn replace_review_range(text: &str, start: usize, end: usize, replacement: &str) -> Option<String> {
    let start = utf16_char_index(text, start)?;
    let end = utf16_char_index(text, end)?;
    if start >= end {
        return None;
    }
    let byte_at = |index| text.char_indices().nth(index).map(|(byte, _)| byte).unwrap_or(text.len());
    let (from, to) = (byte_at(start), byte_at(end));
    Some(format!("{}{replacement}{}", text.get(..from)?, text.get(to..)?))
}

/// The panels running plugins show, at first beside the properties panel and each other (they
/// can be moved): a pressed button or a changed value calls its plugin
/// back (`plugin.event`); closing one ends its plugin's session.
fn panel_windows(app: &mut FigApp, ctx: &Context) {
    let t = Tokens::get(ctx);
    let right = ctx.content_rect().right() - 252.0;
    // (plugin, what was used; None: closed)
    let mut actions: Vec<(String, Option<Used>)> = Vec::new();
    for (i, open) in app.session.plugins.panels.iter().enumerate() {
        let id = open.manifest.id.as_str();
        let mut keep = true;
        let mut used = None;
        // Side by side leftwards from the properties panel, three to a row.
        let at = egui::pos2(right - PANEL_STEP * (i % 3) as f32, 52.0 + 28.0 * (i / 3) as f32);
        floating(open.title(), ctx).id(Id::new(("plugin_panel", id))).pivot(Align2::RIGHT_TOP).default_pos(at).open(&mut keep).show(ctx, |ui| {
            ui.set_width(320.0);
            used = controls(ui, id, &open.panel.controls, &open.values, &t);
        });
        if !keep {
            actions.push((id.to_string(), None));
        } else if used.is_some() {
            actions.push((id.to_string(), used));
        }
    }
    for (id, used) in actions {
        match used {
            None => {
                app.run("plugin.close", json!({"id": id}));
            }
            Some(Used::Set(control, value)) => run(app, "plugin.event", json!({"id": id, "control": control, "value": value})),
            Some(Used::Pick(control)) => app.ui.picker = Some(PickerTarget::PluginField { plugin: id, control, pending: None }),
        }
    }
}

/// What the Manage plugins window asks for.
enum Manage {
    Remove(String),
    Import,
    InspectFigma,
    ImportFigma(String),
    RemoveFigma(String),
    StatusFigma(String),
    RunFigma(String, Option<String>),
    AcceptReview(usize, usize, String),
    Browse(String),
    Confirm(String),
    /// Stop waiting for the directory's listing.
    StopBrowsing,
    SearchCommunity(usize),
    SelectCommunity(String),
    OpenCommunity(String),
}

/// The directory requests the window sends without waiting (the server answers later).
const LIST: &str = "plugin.directory.list";
const INSTALL: &str = "plugin.directory.install";

/// Width of the Manage plugins window, and of a plugin's text beside its button.
const MANAGE_W: f32 = 400.0;
const ROW_TEXT_W: f32 = 300.0;

fn manage_window(app: &mut FigApp, ctx: &Context) {
    poll_community(app);
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut asked = None;
    let mut tab = app.ui.plugins_manage.tab;
    window(tr("Plugins"), ctx).id(Id::new("Plugins")).open(&mut open).show(ctx, |ui| {
        ui.set_width(MANAGE_W);
        tab_strip(ui, &t, &PluginsTab::ALL, PluginsTab::label, &mut tab);
        ui.add_space(12.0);
        asked = match tab {
            PluginsTab::Installed => installed_tab(app, ui, &t),
            PluginsTab::Browse => browse_tab(app, ui, &t),
            PluginsTab::FigmaLocal => figma_local_tab(app, ui, &t),
            PluginsTab::FigmaCommunity => community_tab(app, ui, &t),
        };
    });
    app.ui.plugins_manage.tab = tab;
    app.ui.plugins_window = open;
    match asked {
        Some(Manage::Remove(id)) => {
            app.run("plugin.remove", json!({"id": id}));
        }
        Some(Manage::Import) => import_dialog(app),
        Some(Manage::InspectFigma) => inspect_figma_dialog(app),
        Some(Manage::ImportFigma(path)) => {
            if let Some(result) = app.run("plugin.figma.import", json!({"path": path, "mode": "development"})) {
                app.ui.plugins_manage.figma_report = Some(result["report"].clone());
            }
        }
        Some(Manage::RemoveFigma(id)) if app.run("plugin.figma.remove", json!({"id": id})).is_some() => {
            app.ui.plugins_manage.figma_report = None;
        }
        Some(Manage::RemoveFigma(_)) => {}
        Some(Manage::StatusFigma(id)) => {
            if let Some(report) = app.run("plugin.figma.status", json!({"id": id})) {
                app.ui.plugins_manage.figma_report = Some(report);
            }
        }
        Some(Manage::RunFigma(id, command)) => {
            if let Some(report) = app.ui.plugins_manage.figma_report.as_mut() {
                report["lastRun"] = Value::Null;
            }
            app.ui.plugins_manage.figma_review_target = (command.as_deref() == Some("textreview"))
                .then(|| {
                    let node_id = *app.session.selection().first()?;
                    let doc = app.session.doc().ok()?;
                    let node = doc.node(node_id)?;
                    let figcraft_doc::NodeKind::Text(text) = &node.kind else { return None };
                    Some((node_id, text.characters.clone()))
                })
                .flatten();
            let has_ui = app.ui.plugins_manage.figma_report.as_ref().is_some_and(|report| report["hasUi"] == true);
            if has_ui && !cfg!(target_os = "macos") {
                app.toast("A prévia de HTML dos plugins locais está disponível no macOS por enquanto".to_string());
            } else {
                let command_id = if has_ui { "plugin.figma.ui.start" } else { "plugin.figma.run" };
                let parameters = app.ui.plugins_manage.figma_parameters.clone();
                if let Some(result) = app.run(command_id, json!({"id": id, "command": command, "parameters": parameters})) {
                    if let Some(notices) = result["notifications"].as_array()
                        && let Some(last) = notices.last().and_then(Value::as_str)
                    {
                        app.toast(last.to_string());
                    }
                    if let Some(report) = app.ui.plugins_manage.figma_report.as_mut() {
                        report["lastRun"] = result.clone();
                    }
                    if has_ui {
                        app.ui.plugins_window = false;
                    }
                } else {
                    app.ui.plugins_manage.figma_review_target = None;
                }
            }
        }
        Some(Manage::AcceptReview(start, end, replacement)) => {
            if let Some((node_id, original)) = app.ui.plugins_manage.figma_review_target.clone() {
                let current = app.session.doc().ok().and_then(|doc| doc.node(node_id)).and_then(|node| {
                    let figcraft_doc::NodeKind::Text(text) = &node.kind else { return None };
                    Some(text.characters.clone())
                });
                if current.as_deref() != Some(original.as_str()) {
                    app.toast("O texto mudou; revise novamente antes de aplicar a sugestão");
                } else if let Some(revised) = replace_review_range(&original, start, end, &replacement)
                    && app.run("text.set", json!({"id": node_id, "characters": revised})).is_some()
                {
                    app.ui.plugins_manage.figma_review_target = None;
                    if let Some(report) = app.ui.plugins_manage.figma_report.as_mut() {
                        report["lastRun"] = Value::Null;
                    }
                }
            }
        }
        Some(Manage::Browse(server)) => {
            app.ui.plugins_manage.confirm = None;
            app.cancel_background(INSTALL);
            app.run_background(ctx, LIST, json!({"server": server}), Box::new(|_, _, _| {}));
        }
        Some(Manage::Confirm(id)) => app.ui.plugins_manage.confirm = Some(id),
        Some(Manage::StopBrowsing) => app.cancel_background(LIST),
        Some(Manage::SearchCommunity(from)) => search_community(app, ctx, from),
        Some(Manage::SelectCommunity(id)) => app.ui.plugins_manage.community_selected = Some(id),
        Some(Manage::OpenCommunity(url)) => ctx.open_url(egui::OpenUrl::new_tab(url)),
        None => {}
    }
    if !open {
        // Closing the window stops waiting for its server.
        app.cancel_background(LIST);
        app.cancel_background(INSTALL);
        app.ui.plugins_manage.confirm = None;
        app.figma_catalog_rx = None;
        app.ui.plugins_manage.community_loading = false;
    }
    if app.ui.plugins_manage.confirm.is_some() {
        confirm_window(app, ctx, &t);
    }
}

fn inspect_figma_dialog(app: &mut FigApp) {
    app.pick_open("Escolher manifest.json do plugin Figma", &["json"], |app, path| {
        if let Some(report) = app.run("plugin.figma.inspect", json!({"path": path.to_string_lossy()})) {
            app.ui.plugins_manage.figma_report = Some(report);
            app.ui.plugins_manage.tab = PluginsTab::FigmaLocal;
        }
    });
}

fn figma_local_tab(app: &mut FigApp, ui: &mut Ui, t: &Tokens) -> Option<Manage> {
    widgets::label(
        ui,
        "Registre um plugin local confiável. A execução aceita um subconjunto de nós, estilos, variáveis e mensagens de UI.",
        t.text_secondary,
    );
    ui.add_space(8.0);
    let mut asked = widgets::button(ui, "Escolher manifest.json…", true, true).clicked().then_some(Manage::InspectFigma);
    for plugin in &app.session.prefs.figma_plugins {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&plugin.name).font(semibold(12.0)).color(t.text));
            if widgets::button(ui, "Verificar", false, true).clicked() {
                asked = Some(Manage::StatusFigma(plugin.id.clone()));
            }
            if widgets::button(ui, tr("Remove"), false, true).clicked() {
                asked = Some(Manage::RemoveFigma(plugin.id.clone()));
            }
        });
    }
    let Some(report) = app.ui.plugins_manage.figma_report.clone() else { return asked };
    ui.add_space(14.0);
    egui::ScrollArea::vertical().id_salt("figma_plugin_report").max_height(360.0).show(ui, |ui| {
        let name = report["name"].as_str().unwrap_or("Plugin");
        ui.label(RichText::new(name).font(semibold(13.0)).color(t.text));
        widgets::label(ui, "Diagnosticado · execução limitada", t.text_secondary);
        let api = report["api"].as_str().unwrap_or("?");
        let editors =
            report["editorType"].as_array().map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        widgets::label(ui, &format!("API {api} · {editors}"), t.text_secondary);
        if let Some(source) = report["source"].as_str() {
            ui.add(egui::Label::new(RichText::new(source).font(regular(11.0)).color(t.text_secondary)).wrap());
        }
        if let Some(digest) = report["digest"].as_str() {
            ui.label(RichText::new(format!("SHA-256: {}…", digest.get(..24).unwrap_or(digest))).font(regular(11.0)).color(t.text_secondary))
                .on_hover_text(digest);
        }
        if let Some(domains) = report["networkAccess"]["allowedDomains"].as_array() {
            let list = domains.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ");
            widgets::label(ui, &format!("Domínios declarados: {list}"), t.text_secondary);
        }
        if report["changedSinceImport"] == true {
            widgets::label(ui, "Os arquivos mudaram desde o registro. Verifique e registre novamente.", t.text_secondary);
        }
        if let Some(path) = report["source"].as_str()
            && !app
                .session
                .prefs
                .figma_plugins
                .iter()
                .any(|plugin| report["id"].as_str() == Some(plugin.id.as_str()) && report["digest"].as_str() == Some(plugin.digest.as_str()))
            && widgets::button(ui, "Registrar para desenvolvimento", false, true).clicked()
        {
            asked = Some(Manage::ImportFigma(path.to_string()));
        }
        ui.add_space(8.0);
        let registered = app
            .session
            .prefs
            .figma_plugins
            .iter()
            .any(|plugin| report["id"].as_str() == Some(plugin.id.as_str()) && report["digest"].as_str() == Some(plugin.digest.as_str()));
        if registered
            && report["commands"].as_array().is_none_or(Vec::is_empty)
            && let Some(id) = report["id"].as_str()
            && widgets::button(ui, "Executar plugin", false, true).clicked()
        {
            asked = Some(Manage::RunFigma(id.to_string(), None));
        }
        if registered
            && report["capabilities"].as_array().is_some_and(|items| items.iter().any(|item| item == "textreview"))
            && let Some(id) = report["id"].as_str()
            && widgets::button(ui, "Revisar texto selecionado", false, true).clicked()
        {
            asked = Some(Manage::RunFigma(id.to_string(), Some("textreview".into())));
        }
        if let Some(commands) = report["commands"].as_array()
            && !commands.is_empty()
        {
            widgets::label(ui, "Comandos do menu:", t.text_secondary);
            for command in commands {
                if let (Some(label), Some(id)) = (command["label"].as_str(), command["command"].as_str()) {
                    if registered {
                        if let Some(parameters) = command["parameters"].as_array() {
                            for parameter in parameters {
                                let Some(key) = parameter["key"].as_str() else { continue };
                                let name = parameter["name"].as_str().unwrap_or(key);
                                let mut input =
                                    app.ui.plugins_manage.figma_parameters.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
                                let mut changed = false;
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(name).font(regular(11.0)).color(t.text_secondary));
                                    changed = ui.add(egui::TextEdit::singleline(&mut input).desired_width(110.0)).changed();
                                });
                                app.ui.plugins_manage.figma_parameters.insert(key.to_string(), json!(input));
                                let feedback_key = format!("{id}/{key}");
                                if changed
                                    && let Some(plugin_id) = report["id"].as_str()
                                    && let Some(reply) = app.run(
                                        "plugin.figma.run",
                                        json!({
                                            "id":plugin_id, "command":id,
                                            "parameterInput":{"key":key,"query":input}
                                        }),
                                    )
                                {
                                    app.ui.plugins_manage.figma_feedback.insert(feedback_key.clone(), reply["eventResult"].clone());
                                }
                                if let Some(feedback) = app.ui.plugins_manage.figma_feedback.get(&feedback_key).cloned() {
                                    if let Some(error) = feedback["error"].as_str() {
                                        ui.label(RichText::new(error).font(regular(11.0)).color(t.danger));
                                    }
                                    if let Some(suggestions) = feedback["suggestions"].as_array() {
                                        ui.horizontal_wrapped(|ui| {
                                            for suggestion in suggestions.iter().filter_map(Value::as_str).take(6) {
                                                if widgets::button(ui, suggestion, false, true).clicked() {
                                                    app.ui.plugins_manage.figma_parameters.insert(key.to_string(), json!(suggestion));
                                                }
                                            }
                                        });
                                    }
                                }
                            }
                        }
                        if widgets::button(ui, label, false, true).clicked()
                            && let Some(plugin_id) = report["id"].as_str()
                        {
                            asked = Some(Manage::RunFigma(plugin_id.to_string(), Some(id.to_string())));
                        }
                    } else {
                        widgets::label(ui, &format!("• {label} ({id})"), t.text_secondary);
                    }
                }
            }
        }
        if report["lastRun"]["status"] == "executed" {
            let operations = report["lastRun"]["operations"].as_u64().unwrap_or(0);
            widgets::label(ui, &format!("Executado · {operations} alteração(ões)"), t.text_secondary);
            if let Some(results) = report["lastRun"]["eventResult"].as_array() {
                widgets::label(ui, &format!("{} sugestão(ões) de revisão", results.len()), t.text_secondary);
                for item in results.iter().take(20) {
                    let (Some(start), Some(end)) = (
                        item["start"].as_u64().and_then(|value| usize::try_from(value).ok()),
                        item["end"].as_u64().and_then(|value| usize::try_from(value).ok()),
                    ) else {
                        continue;
                    };
                    ui.horizontal_wrapped(|ui| {
                        widgets::label(ui, &format!("{start}–{end}:"), t.text_secondary);
                        for choice in item["suggestions"].as_array().into_iter().flatten().filter_map(Value::as_str).take(6) {
                            if app.ui.plugins_manage.figma_review_target.is_some() && widgets::button(ui, choice, false, true).clicked() {
                                asked = Some(Manage::AcceptReview(start, end, choice.to_string()));
                            } else if app.ui.plugins_manage.figma_review_target.is_none() {
                                widgets::label(ui, choice, t.text_secondary);
                            }
                        }
                    });
                }
            }
            if let Some(message) = report["lastRun"]["message"].as_str() {
                widgets::label(ui, message, t.text_secondary);
            }
        }
        if let Some(files) = report["files"].as_array() {
            widgets::label(ui, "Arquivos:", t.text_secondary);
            for file in files {
                if let Some(path) = file["path"].as_str() {
                    widgets::label(ui, &format!("• {path} ({} bytes)", file["bytes"]), t.text_secondary);
                }
            }
        }
        ui.add_space(8.0);
        if let Some(diagnostics) = report["diagnostics"].as_array() {
            for reason in diagnostics.iter().filter_map(Value::as_str) {
                ui.add(egui::Label::new(RichText::new(format!("• {reason}")).font(regular(11.0)).color(t.text_secondary)).wrap());
            }
        }
    });
    asked
}

/// Search the read-only public catalogue. This is an internal endpoint: errors and schema changes
/// are shown in the window, and the response never reaches the plugin runner.
fn search_community(app: &mut FigApp, ctx: &Context, from: usize) {
    let query = app.ui.plugins_manage.community_query.clone();
    let url = match catalog::search_url(&query, from) {
        Ok(url) => url,
        Err(error) => {
            app.ui.plugins_manage.community_error = Some(error);
            return;
        }
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    app.figma_catalog_rx = Some(receiver);
    app.ui.plugins_manage.community_loading = true;
    app.ui.plugins_manage.community_page = None;
    app.ui.plugins_manage.community_error = None;
    app.ui.plugins_manage.community_selected = None;
    app.ui.plugins_manage.community_from = from;
    let repaint = ctx.clone();
    let mut request = ehttp::Request::get(&url);
    request.headers.insert("Accept", "application/json");
    request.timeout = Some(std::time::Duration::from_secs(12));
    ehttp::fetch(request, move |response| {
        let result = response.and_then(|response| {
            if !response.url.starts_with("https://www.figma.com/api/search/resources?") {
                return Err("Catálogo redirecionou a consulta para outro endereço.".into());
            }
            if !response.ok {
                return Err(format!("Catálogo indisponível (HTTP {}).", response.status));
            }
            if response.bytes.len() > catalog::MAX_RESPONSE_BYTES {
                return Err("Resposta do catálogo muito grande.".into());
            }
            Ok(response.bytes)
        });
        let _ = sender.send(result);
        repaint.request_repaint();
    });
}

fn poll_community(app: &mut FigApp) {
    let Some(receiver) = app.figma_catalog_rx.as_ref() else { return };
    let result = match receiver.try_recv() {
        Ok(result) => result,
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("A consulta ao catálogo foi interrompida.".into()),
    };
    app.figma_catalog_rx = None;
    app.ui.plugins_manage.community_loading = false;
    match result.and_then(|bytes| catalog::parse_page(&bytes, app.ui.plugins_manage.community_from)) {
        Ok(page) => {
            app.ui.plugins_manage.community_page = Some(page);
            app.ui.plugins_manage.community_error = None;
        }
        Err(error) => {
            app.ui.plugins_manage.community_page = None;
            app.ui.plugins_manage.community_error = Some(error);
        }
    }
}

fn community_tab(app: &mut FigApp, ui: &mut Ui, t: &Tokens) -> Option<Manage> {
    let state = &mut app.ui.plugins_manage;
    let mut asked = None;
    ui.horizontal(|ui| {
        let mut enter = false;
        if let Some(query) = widgets::text_field(ui, Id::new("figma_community_query"), &state.community_query, MANAGE_W - 95.0, "Buscar plugins") {
            enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
            state.community_query = query;
        }
        let clicked = widgets::button(ui, "Buscar", true, !state.community_loading).clicked();
        if !state.community_loading && (clicked || enter) && !state.community_query.trim().is_empty() {
            asked = Some(Manage::SearchCommunity(0));
        }
    });
    ui.add_space(8.0);
    widgets::label(ui, "Catálogo somente para consulta. Os resultados não incluem um pacote executável para o FigCraft.", t.text_secondary);
    if state.community_loading {
        ui.add_space(8.0);
        widgets::label(ui, "Buscando plugins…", t.text_secondary);
    }
    if let Some(error) = &state.community_error {
        ui.add_space(8.0);
        widgets::label(ui, error, t.text_secondary);
    }
    let Some(page) = &state.community_page else { return asked };
    ui.add_space(10.0);
    widgets::label(
        ui,
        &format!(
            "{} resultados · exibindo {}–{}",
            page.total,
            page.from.saturating_add(1).min(page.total),
            page.from.saturating_add(page.entries.len()).min(page.total)
        ),
        t.text_secondary,
    );
    if page.entries.is_empty() {
        widgets::label(ui, "Nenhum plugin encontrado.", t.text_secondary);
    }
    egui::ScrollArea::vertical().id_salt("figma_community_results").max_height(300.0).show(ui, |ui| {
        for entry in &page.entries {
            ui.add_space(7.0);
            let name =
                if entry.name.chars().count() > 43 { format!("{}…", entry.name.chars().take(42).collect::<String>()) } else { entry.name.clone() };
            if widgets::button(ui, &name, state.community_selected.as_deref() == Some(&entry.plugin_id), true).on_hover_text(&entry.name).clicked() {
                asked = Some(Manage::SelectCommunity(entry.plugin_id.clone()));
            }
            if !entry.tagline.is_empty() {
                ui.add(egui::Label::new(RichText::new(&entry.tagline).font(regular(11.0)).color(t.text_secondary)).wrap());
            }
        }
    });
    ui.horizontal(|ui| {
        if page.from > 0 && widgets::button(ui, "Anterior", false, !state.community_loading).clicked() {
            asked = Some(Manage::SearchCommunity(page.from.saturating_sub(catalog::PAGE_SIZE)));
        }
        if page.from.saturating_add(catalog::PAGE_SIZE) < page.total && widgets::button(ui, "Próxima", false, !state.community_loading).clicked() {
            asked = Some(Manage::SearchCommunity(page.from.saturating_add(catalog::PAGE_SIZE)));
        }
    });
    if let Some(entry) = page.entries.iter().find(|entry| state.community_selected.as_deref() == Some(&entry.plugin_id)) {
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new(&entry.name).font(semibold(13.0)).color(t.text));
        widgets::label(ui, &format!("Plugin ID: {} · Recurso: {}", entry.plugin_id, entry.resource_id), t.text_secondary);
        if let Some(version) = &entry.version_id {
            widgets::label(ui, &format!("Versão publicada: {version}"), t.text_secondary);
        }
        widgets::label(
            ui,
            match entry.has_ui {
                Some(true) => "Declara interface HTML · execução indisponível",
                Some(false) => "Sem interface HTML declarada · execução indisponível",
                None => "Manifesto indisponível · execução indisponível",
            },
            t.text_secondary,
        );
        ui.add_space(8.0);
        if widgets::button(ui, "Ver página original", false, true).clicked() {
            asked = Some(Manage::OpenCommunity(entry.page_url.clone()));
        }
    }
    asked
}

fn installed_tab(app: &FigApp, ui: &mut Ui, t: &Tokens) -> Option<Manage> {
    let mut asked = None;
    let plugins = &app.session.prefs.plugins;
    if plugins.is_empty() {
        widgets::label(ui, tr("No plugins yet. Import a development plugin from its figcraft-plugin.json."), t.text_secondary);
    }
    for (i, p) in plugins.iter().enumerate() {
        if i > 0 {
            ui.add_space(8.0);
        }
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(ROW_TEXT_W);
                ui.label(RichText::new(&p.manifest.name).font(semibold(12.0)).color(t.text));
                widgets::label(ui, &summary(p), t.text_secondary);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::button(ui, tr("Remove"), false, true).clicked() {
                    asked = Some(Manage::Remove(p.manifest.id.clone()));
                }
            });
        });
    }
    ui.add_space(12.0);
    if widgets::button(ui, tr("Import plugin from manifest…"), true, true).clicked() {
        asked = Some(Manage::Import);
    }
    asked
}

/// A server's plugin directory: its address, a search, and its plugins with Install, Update or
/// Remove. Nothing is fetched until Browse is pressed.
fn browse_tab(app: &mut FigApp, ui: &mut Ui, t: &Tokens) -> Option<Manage> {
    let mut asked = None;
    let mut server = app.ui.plugins_manage.server.clone().or_else(|| app.session.prefs.plugin_directory.clone()).unwrap_or_default();
    let loading = app.session.waiting_for(LIST);
    ui.horizontal(|ui| {
        // The field commits on Enter (which also browses) or when Browse takes the focus.
        let mut enter = false;
        if let Some(s) =
            widgets::text_field(ui, Id::new("plugin_directory_server"), &server, MANAGE_W - 90.0, tr("Server address (ws://host:47900/)"))
        {
            enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            server = s.clone();
            app.ui.plugins_manage.server = Some(s);
        }
        if loading {
            if widgets::button(ui, tr("Cancel"), false, true).clicked() {
                asked = Some(Manage::StopBrowsing);
            }
        } else if (widgets::button(ui, tr("Browse"), true, true).clicked() || enter) && !server.trim().is_empty() {
            asked = Some(Manage::Browse(server.trim().to_string()));
        }
    });
    if loading {
        ui.add_space(8.0);
        widgets::label(ui, tr("Fetching from the server…"), t.text_secondary);
        return asked;
    }
    let Some(listing) = app.session.plugins.directory.as_ref() else {
        ui.add_space(8.0);
        widgets::label(ui, tr("Enter the address of a figcraft-server and press Browse to see the plugins it offers."), t.text_secondary);
        return asked;
    };
    ui.add_space(8.0);
    if let Some(q) = widgets::text_field(ui, Id::new("plugin_directory_query"), &app.ui.plugins_manage.query, MANAGE_W, tr("Search plugins")) {
        app.ui.plugins_manage.query = q;
    }
    ui.add_space(8.0);
    let q = app.ui.plugins_manage.query.trim().to_lowercase();
    let shown: Vec<&PluginEntry> = listing
        .entries
        .iter()
        .filter(|e| q.is_empty() || [&e.name, &e.description, &e.author, &e.id].iter().any(|f| f.to_lowercase().contains(&q)))
        .collect();
    if shown.is_empty() {
        widgets::label(ui, &trf("No plugins on {server}.", &[("server", &listing.server)]), t.text_secondary);
    }
    // One install at a time: the rows wait for it.
    let installing = app.session.waiting_for(INSTALL);
    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
        for (i, e) in shown.iter().enumerate() {
            if i > 0 {
                ui.add_space(10.0);
            }
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(ROW_TEXT_W);
                    ui.label(RichText::new(&e.name).font(semibold(12.0)).color(t.text));
                    widgets::label(ui, &entry_summary(e), t.text_secondary);
                    if !e.description.is_empty() {
                        ui.add(egui::Label::new(RichText::new(&e.description).font(regular(11.0)).color(t.text)).wrap());
                    }
                });
                let (label, primary, ask): (&str, bool, fn(String) -> Manage) = match installed_version(&app.session, &e.id) {
                    None => (tr("Install"), true, Manage::Confirm),
                    Some(have) if newer_version(&e.version, have) => (tr("Update"), true, Manage::Confirm),
                    Some(_) => (tr("Remove"), false, Manage::Remove),
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if widgets::button(ui, label, primary, !installing).clicked() {
                        asked = Some(ask(e.id.clone()));
                    }
                });
            });
        }
    });
    asked
}

/// "1.2.0 · by Ada · 54 KB · can edit the document" under a directory plugin's name.
fn entry_summary(e: &PluginEntry) -> String {
    let mut parts = vec![e.version.clone()];
    if !e.author.is_empty() {
        parts.push(trf("by {author}", &[("author", &e.author)]));
    }
    parts.push(size(e.bytes));
    parts.push(access_text(&e.access).to_string());
    parts.join(" · ")
}

fn access_text(access: &str) -> &'static str {
    if access == "write" { tr("can edit the document") } else { tr("reads the document") }
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / f64::from(1 << 20)),
        b if b >= 1 << 10 => format!("{} KB", b.div_ceil(1 << 10)),
        b => format!("{b} B"),
    }
}

/// Installing runs a plugin's code: this window shows what it asks permission for, where it comes
/// from and its SHA-256, and installs only when Install is pressed.
fn confirm_window(app: &mut FigApp, ctx: &Context, t: &Tokens) {
    let id = app.ui.plugins_manage.confirm.clone().unwrap_or_default();
    let found = app.session.plugins.directory.as_ref().and_then(|l| Some((l.server.clone(), l.entries.iter().find(|e| e.id == id)?.clone())));
    let Some((server, e)) = found else {
        app.ui.plugins_manage.confirm = None;
        return;
    };
    let update = installed_version(&app.session, &e.id).is_some();
    let installing = app.session.waiting_for(INSTALL);
    let title = if update { tr("Update plugin") } else { tr("Install plugin") };
    let (mut open, mut install, mut cancel) = (true, false, false);
    window(title, ctx).id(Id::new("plugin_install_confirm")).open(&mut open).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(RichText::new(format!("{} {}", e.name, e.version)).font(semibold(13.0)).color(t.text));
        widgets::label(ui, &trf("From {server}", &[("server", &server)]), t.text_secondary);
        ui.add_space(10.0);
        ui.label(RichText::new(tr("It asks permission to:")).font(semibold(11.0)).color(t.text));
        let mut asks = vec![if e.access == "write" { tr("read and edit the document") } else { tr("read the document") }.to_string()];
        asks.push(if e.commands.is_empty() {
            tr("call any command its access allows").to_string()
        } else {
            trf("call only: {commands}", &[("commands", &e.commands.join(", "))])
        });
        if e.widget {
            asks.push(tr("put a widget on the canvas").to_string());
        }
        for a in asks {
            ui.add(egui::Label::new(RichText::new(format!("•  {a}")).font(regular(11.0)).color(t.text)).wrap());
        }
        ui.add_space(8.0);
        widgets::label(
            ui,
            tr("Plugins run in FigCraft's sandbox, without files, network or clock. Install only plugins you trust."),
            t.text_secondary,
        );
        ui.add_space(4.0);
        widgets::label(ui, &trf("SHA-256: {sha}…", &[("sha", &e.sha256.get(..16).unwrap_or_default())]), t.text_secondary);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if installing {
                widgets::label(ui, tr("Installing…"), t.text_secondary);
            } else {
                install = widgets::button(ui, if update { tr("Update") } else { tr("Install") }, true, true).clicked();
            }
            cancel = widgets::button(ui, tr("Cancel"), false, true).clicked();
        });
    });
    if !open || cancel {
        app.cancel_background(INSTALL);
        app.ui.plugins_manage.confirm = None;
    }
    if install {
        // The window stays open (Installing…) until the download is in; an error leaves it open.
        let params = json!({"id": e.id, "version": e.version, "sha256": e.sha256, "server": server, "confirm": true});
        app.run_background(
            ctx,
            INSTALL,
            params,
            Box::new(move |app, _, v| {
                app.ui.plugins_manage.confirm = None;
                let name = v["name"].as_str().unwrap_or(tr("the plugin")).to_string();
                let args: &[(&str, &dyn std::fmt::Display)] = &[("name", &name)];
                app.toast(if update { trf("Updated {name}", args) } else { trf("Installed {name}", args) });
            }),
        );
    }
}

/// "1.0 · can edit · development: <path>" under a plugin's name.
fn summary(p: &InstalledPlugin) -> String {
    let m = &p.manifest;
    let access = match m.permissions.access {
        Access::Read => tr("reads the document"),
        Access::Write => tr("can edit the document"),
    };
    let mut parts: Vec<String> = Vec::new();
    if !m.version.is_empty() {
        parts.push(m.version.clone());
    }
    parts.push(access.to_string());
    parts.push(match (&p.source, &p.path) {
        (Some(server), _) => trf("from {server}", &[("server", server)]),
        (None, Some(path)) => trf("development: {path}", &[("path", path)]),
        (None, None) => tr("this session only").into(),
    });
    parts.join(" · ")
}

fn form_window(app: &mut FigApp, ctx: &Context) {
    let Some(mut form) = app.ui.plugin_form.clone() else { return };
    let Some((name, command)) = app
        .session
        .prefs
        .plugins
        .iter()
        .find(|p| p.manifest.id == form.id)
        .and_then(|p| Some((p.manifest.name.clone(), p.manifest.command(Some(&form.command))?.clone())))
    else {
        app.ui.plugin_form = None;
        return;
    };
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut submit = false;
    let mut cancel = false;
    let mut pick = None;
    let label = command.label.trim_end_matches(['…', '.']).trim();
    let title = if label.contains(name.as_str()) { label.to_string() } else { format!("{name}: {label}") };
    window(&title, ctx).id(Id::new("plugin_form")).open(&mut open).show(ctx, |ui| {
        ui.set_width(320.0);
        match controls(ui, &format!("{}/{}", form.id, form.command), &command.params, &form.values, &t) {
            Some(Used::Set(name, v)) => {
                form.values.insert(name, v);
            }
            Some(Used::Pick(control)) => pick = Some(control),
            None => {}
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            submit = widgets::button(ui, tr("Run"), true, true).clicked();
            cancel = widgets::button(ui, tr("Cancel"), false, true).clicked();
        });
    });
    app.ui.plugin_form = (open && !submit && !cancel).then_some(form.clone());
    if let Some(control) = pick {
        app.ui.picker = Some(PickerTarget::PluginField { plugin: form.id.clone(), control, pending: None });
    }
    if submit {
        run(app, "plugin.run", json!({"id": form.id, "command": form.command, "params": form.values}));
    }
}

/// Parameters or panel controls in two columns (label, field); buttons in a row share one line,
/// the first button filled. Returns what the user did to which control.
fn controls(ui: &mut Ui, salt: &str, params: &[Param], values: &Map<String, Value>, t: &Tokens) -> Option<Used> {
    let mut changed = None;
    let mut first_button = true;
    egui::Grid::new(("plugin_controls", salt)).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        for row in params.chunk_by(|a, b| a.kind == ParamKind::Button && b.kind == ParamKind::Button) {
            let Some(p) = row.first() else { continue };
            // Checkboxes and buttons carry their own label; a list's sits by its first item.
            let caption = if matches!(p.kind, ParamKind::Boolean | ParamKind::Button | ParamKind::Note) { "" } else { p.title() };
            let caption = RichText::new(caption).font(regular(11.0)).color(t.text_secondary);
            if p.kind == ParamKind::List {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| ui.label(caption));
            } else {
                ui.label(caption);
            }
            let hit = match p.kind {
                ParamKind::Note => {
                    ui.vertical(|ui| {
                        ui.set_max_width(FIELD_W);
                        ui.add(egui::Label::new(RichText::new(&p.label).font(regular(11.0)).color(t.text)).wrap());
                    });
                    None
                }
                ParamKind::Button => {
                    ui.horizontal(|ui| {
                        let pressed = row.iter().filter(|b| widgets::button(ui, b.title(), std::mem::take(&mut first_button), true).clicked());
                        pressed.last().map(|b| Used::Set(b.name.clone(), Value::Null))
                    })
                    .inner
                }
                _ => {
                    let current = values.get(&p.name).cloned().unwrap_or_else(|| p.default_value());
                    field(ui, Id::new(("plugin_control", salt, &p.name)), p, &current)
                }
            };
            if hit.is_some() {
                changed = hit;
            }
            ui.end_row();
        }
    });
    changed
}

/// One parameter's field, named for screen readers: its new value when it changed.
fn field(ui: &mut Ui, id: Id, p: &Param, current: &Value) -> Option<Used> {
    let name = p.title();
    let v = match p.kind {
        ParamKind::Text => widgets::text_field_named(ui, id, current.as_str().unwrap_or_default(), FIELD_W, name).map(Value::String),
        ParamKind::Number => match widgets::number_field(ui, id, Lead::Named(name), current.as_f64(), "", FIELD_W, 1.0)? {
            FieldEdit::Set(v) | FieldEdit::Scrub(v, true) => p.coerce(&json!(v)).ok(),
            FieldEdit::Scrub(_, false) => None,
        },
        ParamKind::Boolean => {
            let on = current.as_bool().unwrap_or(false);
            widgets::checkbox(ui, on, name).then_some(Value::Bool(!on))
        }
        ParamKind::Select => {
            let value = current.as_str().unwrap_or_default();
            let resp = widgets::dropdown(ui, value, FIELD_W, None);
            crate::a11y::combo(&resp, name, value, crate::a11y::menu_open(&resp));
            let mut picked = None;
            crate::right::menu_popup(&resp).show(|ui| {
                for o in &p.options {
                    if crate::right::menu_item(ui, o, "") {
                        picked = Some(Value::String(o.clone()));
                        ui.close();
                    }
                }
            });
            picked
        }
        ParamKind::List => {
            let value = current.as_str();
            ui.vertical(|ui| {
                p.options
                    .iter()
                    .filter(|o| widgets::radio(ui, value == Some(o.as_str()), o) && value != Some(o.as_str()))
                    .last()
                    .map(|o| Value::String(o.clone()))
            })
            .inner
        }
        // A swatch that opens the colour picker, and the hex.
        ParamKind::Color => {
            let hex = current.as_str().unwrap_or_default();
            let (picked, typed) = ui
                .horizontal(|ui| {
                    let color = figcraft_color::Color::from_hex(hex).unwrap_or(figcraft_color::Color::BLACK);
                    let picked = widgets::swatch(ui, &figcraft_color::Paint::solid(color), widgets::FIELD_H).clicked();
                    (picked, widgets::text_field_named(ui, id, hex, FIELD_W - widgets::FIELD_H - 8.0, name).and_then(|s| p.coerce(&json!(s)).ok()))
                })
                .inner;
            if picked {
                return Some(Used::Pick(p.name.clone()));
            }
            typed
        }
        ParamKind::Button | ParamKind::Note => None,
    };
    v.map(|v| Used::Set(p.name.clone(), v))
}

#[cfg(test)]
mod text_review_tests {
    use super::utf16_char_index;

    #[test]
    fn review_offsets_follow_utf16_without_splitting_emoji() {
        let text = "✨Figma✨ 😀 figma";
        assert_eq!(utf16_char_index(text, 11), Some(10));
        assert_eq!(utf16_char_index(text, 12), Some(11));
        assert_eq!(utf16_char_index(text, 9), None);
        assert_eq!(utf16_char_index(text, 16), Some(15));
        assert_eq!(super::replace_review_range("😀 figma", 3, 8, "✨Figma✨").as_deref(), Some("😀 ✨Figma✨"));
    }
}
