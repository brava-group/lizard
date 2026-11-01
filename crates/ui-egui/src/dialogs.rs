//! Dialogs: About, Preferences, keyboard shortcuts, find & replace, rename layers, variables,
//! export.

use std::path::Path;

use egui::{Align, Align2, Color32, Context, CornerRadius, Id, Layout, Rect, RichText, Sense, Stroke, UiBuilder, pos2, vec2};
use lizard_color::ColorProfile;
use lizard_doc::{ExportFormat, ExportSetting, ExportSize, NodeId};
use lizard_engine::cmd::file::{export_jobs, exportable_layers};
use serde_json::{Value, json};

use crate::LizardApp;
use crate::agent_panel::{AgentConnection, AgentKind};
use crate::export_panel::{export_layers, save_one, suggested_name};
use crate::i18n::{self, tr, tr_noop, trf, trn};
use crate::icons::{self, Icon};
use crate::primitives::{Button, ButtonAlign, ButtonBehavior, ButtonSize, ButtonVariant, ControlWidth};
use crate::theme::{Tokens, regular, semibold};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where a report goes.
const ISSUES: &str = "https://github.com/brava-group/lizard/issues";

/// What the desktop app says when its window couldn't open with `error` (no frame was drawn: the
/// language is the one [`i18n::set`] picked from the preferences). `backend_set`: `WGPU_BACKEND`
/// named the graphics API, so the hint is to unset it rather than to try another one.
pub fn window_failed(error: &str, backend_set: bool) -> String {
    let hint = if backend_set {
        tr("WGPU_BACKEND is set and names the graphics API to use; unsetting it lets Lizard choose.").to_string()
    } else if cfg!(windows) {
        // Direct3D 12 and OpenGL are both offered, but wgpu keeps the Direct3D 12 adapter it picked
        // even when its device then fails, so naming OpenGL alone can still help.
        format!(
            "{}

set WGPU_BACKEND=gl
lizard.exe",
            tr("Updating the graphics driver often helps. To use OpenGL only, start Lizard from a Command Prompt with:")
        )
    } else {
        tr("Updating the graphics driver often helps. WGPU_BACKEND (vulkan, gl, metal) names another graphics API to try.").to_string()
    };
    format!(
        "{}

{hint}

{}",
        trf("Lizard couldn't open its window: {error}", &[("error", &error)]),
        trf("If it keeps happening, please report it at {link} with this message.", &[("link", &ISSUES)])
    )
}

/// A dialog window, centred.
pub(crate) fn window<'a>(title: &'a str, ctx: &Context) -> egui::Window<'a> {
    floating(title, ctx).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
}

/// A window styled like the dialogs that the user can move.
pub(crate) fn floating<'a>(title: &'a str, ctx: &Context) -> egui::Window<'a> {
    let t = Tokens::get(ctx);
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.panel).inner_margin(egui::Margin::same(20)))
}

/// The About window's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AboutTab {
    #[default]
    About,
    Contributors,
    Models,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SettingsSection {
    #[default]
    General,
    Agents,
    Appearance,
    Language,
    Sharing,
}

impl SettingsSection {
    const ALL: [Self; 5] = [Self::General, Self::Agents, Self::Appearance, Self::Language, Self::Sharing];

    fn label(self) -> &'static str {
        match self {
            Self::General => tr_noop("General"),
            Self::Agents => tr_noop("Agents"),
            Self::Appearance => tr_noop("Appearance"),
            Self::Language => tr_noop("Language"),
            Self::Sharing => tr_noop("Sharing"),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::General => Icon::Adjust,
            Self::Agents => Icon::Actions,
            Self::Appearance => Icon::Sun,
            Self::Language => Icon::Text,
            Self::Sharing => Icon::Link,
        }
    }
}

const AGENT_PROVIDERS: [AgentKind; 4] = [AgentKind::Codex, AgentKind::Claude, AgentKind::Gemini, AgentKind::Kimi];

#[derive(Clone, Default)]
struct AgentSettingsState {
    detail: Option<AgentKind>,
    connections: [Option<AgentConnection>; 4],
    login_was_pending: bool,
}

fn provider_index(kind: AgentKind) -> usize {
    match kind {
        AgentKind::Codex => 0,
        AgentKind::Claude => 1,
        AgentKind::Gemini => 2,
        AgentKind::Kimi => 3,
    }
}

/// Values collected by a menu or Actions command before dispatching it to the engine.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuInput {
    Link(String),
    Offset(f64),
    MoveToPage,
}

impl AboutTab {
    pub const ALL: [AboutTab; 3] = [AboutTab::About, AboutTab::Contributors, AboutTab::Models];

    pub fn label(self) -> &'static str {
        match self {
            AboutTab::About => tr("About"),
            AboutTab::Contributors => tr("Contributors"),
            AboutTab::Models => tr("Models"),
        }
    }
}

/// The About window's width, the same on every tab (the Contributors table needs the room).
pub const ABOUT_WIDTH: f32 = 660.0;

/// The About window's tab row, drawn like the Design / Prototype tabs of the right panel.
/// A row of tabs (the About window's, the shortcuts panel's): the current one highlighted.
pub(crate) fn tab_strip<T: Copy + PartialEq>(ui: &mut egui::Ui, _t: &Tokens, tabs: &[T], label: fn(T) -> &'static str, current: &mut T) {
    let items: Vec<_> = tabs.iter().copied().map(|tab| crate::primitives::Tab::new(label(tab))).collect();
    let selected = tabs.iter().position(|tab| tab == current);
    if let Some(index) =
        crate::primitives::Tabs::new(ui.id().with("dialog_tabs"), &items, selected).width(crate::primitives::ControlWidth::Hug).show(ui)
        && let Some(tab) = tabs.get(index)
    {
        *current = *tab;
    }
}

/// About ▸ About: the logo, version, blurb and links.
fn about_body(ui: &mut egui::Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(10), t.field);
        icons::paint(ui.painter(), r.shrink(10.0), Icon::Logo, t.icon);
        ui.vertical(|ui| {
            ui.label(RichText::new("Lizard").font(semibold(18.0)));
            ui.label(RichText::new(trf("Version {version}", &[("version", &VERSION)])).color(t.text_secondary));
        });
    });
    ui.add_space(12.0);
    ui.label(tr("Lizard is an open-source interface design and prototyping app built with Rust."));
    ui.add_space(8.0);
    ui.hyperlink_to("getartcraft.com/apps/lizard", "https://getartcraft.com/apps/lizard");
    ui.hyperlink_to(tr("Join us on Discord"), "https://discord.gg/artcraft");
    ui.hyperlink_to("Background removal: withoutBG Open Model", "https://withoutbg.com/open-model/license");
    ui.hyperlink_to("Built with DINOv3", "https://ai.meta.com/resources/models-and-libraries/dinov3-license/");
    ui.add_space(8.0);
    ui.label(
        RichText::new(tr("Dual-licensed MIT OR Apache-2.0. Made by the ArtCraft team and community.")).color(t.text_secondary).font(regular(11.0)),
    );
}

/// Escape closes the topmost simple dialog (one without menus of its own), before the canvas's
/// shortcuts see it (it doesn't deselect), like a system dialog: from one of its fields too. A menu
/// open over the dialog (Preferences ▸ Interface size) takes the first Escape and closes alone.
pub fn keys(app: &mut LizardApp, ctx: &Context) {
    if egui::Popup::is_any_open(ctx) {
        return;
    }
    let u = &mut app.ui;
    let open = u.color_profile.is_some()
        || u.menu_input.is_some()
        || u.find.is_some()
        || u.batch_rename.is_some()
        || u.import_report.is_some()
        || u.missing_fonts.is_some()
        || u.confirm_close
        || u.export_preview
        || u.shortcuts_panel
        || u.preferences
        || u.about;
    if !open || !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return;
    }
    if u.menu_input.take().is_some()
        || u.color_profile.take().is_some()
        || u.find.take().is_some()
        || u.batch_rename.take().is_some()
        || u.import_report.take().is_some()
        || u.missing_fonts.take().is_some()
    {
        return;
    }
    for flag in [&mut u.confirm_close, &mut u.export_preview, &mut u.shortcuts_panel, &mut u.preferences, &mut u.about] {
        if std::mem::take(flag) {
            return;
        }
    }
}

pub fn show(app: &mut LizardApp, ctx: &Context) {
    // Dropped files waiting to open: their card, then (once it has shown) the files.
    crate::pending_open::pending_opens(app, ctx);
    crate::import_report::show(app, ctx);
    crate::missing_fonts::show(app, ctx);
    let t = Tokens::get(ctx);
    if app.ui.about {
        let mut open = true;
        window(tr("About Lizard"), ctx).open(&mut open).show(ctx, |ui| {
            ui.set_width(ABOUT_WIDTH);
            tab_strip(ui, &t, &AboutTab::ALL, AboutTab::label, &mut app.ui.about_tab);
            ui.add_space(12.0);
            match app.ui.about_tab {
                AboutTab::About => about_body(ui, &t),
                AboutTab::Contributors => crate::credits::contributors_ui(ui),
                AboutTab::Models => crate::credits::models_ui(ui),
            }
        });
        if !open {
            app.ui.about = false;
        }
    }
    if app.ui.preferences {
        settings_modal(app, ctx, &t);
    } else {
        ctx.data_mut(|data| data.remove::<AgentSettingsState>(Id::new("settings_agent_state")));
    }
    crate::shortcuts_panel::show(app, ctx);
    if let Some((mut find, mut replace)) = app.ui.find.clone() {
        let mut open = true;
        let mut action = None;
        egui::Window::new(tr("Find and replace"))
            .id(egui::Id::new("Find and replace"))
            .open(&mut open)
            .anchor(Align2::LEFT_TOP, vec2(260.0, 120.0))
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(280.0);
                let r = ui.add(egui::TextEdit::singleline(&mut find).hint_text(tr("Find")).desired_width(260.0));
                crate::widgets::select_all_on_focus(ui, &r, &find);
                let r = ui.add(egui::TextEdit::singleline(&mut replace).hint_text(tr("Replace with")).desired_width(260.0));
                crate::widgets::select_all_on_focus(ui, &r, &replace);
                ui.horizontal(|ui| {
                    if ui.button(tr("Find")).clicked() {
                        action = Some(false);
                    }
                    if ui.button(tr("Replace all")).clicked() {
                        action = Some(true);
                    }
                });
            });
        if let Some(rep) = action
            && !find.is_empty()
        {
            let r = if rep {
                app.run("edit.findReplace", json!({"find": find, "replace": replace, "layerNames": false}))
            } else {
                app.run("edit.findReplace", json!({"find": find, "layerNames": true}))
            };
            if let Some(v) = r {
                let n = v["matches"].as_array().map(|a| a.len()).unwrap_or(0);
                app.toast(trn("{n} matches", n, &[]));
            }
        }
        app.ui.find = if open { Some((find, replace)) } else { None };
    }
    batch_rename_dialog(app, ctx);
    menu_input_dialog(app, ctx);
    crate::variables_panel::show(app, ctx);
    crate::dev_compare::show(app, ctx);
    crate::branch_ui::show(app, ctx);
    if app.ui.export_preview {
        export_dialog(app, ctx);
    }
    color_profile_dialog(app, ctx);
    crate::plugins_ui::show(app, ctx);
    crate::live::join_dialog(app, ctx);
    crate::live::publish_dialog(app, ctx);
    crate::online::dialogs(app, ctx);
    crate::windows::confirm_close(app, ctx);
}

/// Menus and Actions cannot pass a value themselves. Gather it here, then run the regular
/// command so the same validation and undo behaviour apply as for programmatic callers.
fn menu_input_dialog(app: &mut LizardApp, ctx: &Context) {
    let Some(mut input) = app.ui.menu_input.take() else { return };
    let t = Tokens::get(ctx);
    let title = match &input {
        MenuInput::Link(_) => i18n::command("text.link", "Create link"),
        MenuInput::Offset(_) => i18n::command("vector.offset", "Offset path"),
        MenuInput::MoveToPage => i18n::command("object.moveToPage", "Move to page"),
    };
    let mut open = true;
    let mut action: Option<(&str, serde_json::Value)> = None;
    window(&title, ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(320.0);
        match &mut input {
            MenuInput::Link(url) => {
                ui.horizontal(|ui| {
                    let field = crate::widgets::text_input(ui, egui::Id::new("menu_input_link"), url, 250.0, "https://");
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let ok = !url.trim().is_empty();
                    if (crate::widgets::button(ui, tr("Done"), true, ok).clicked() || enter) && ok {
                        action = Some(("text.link", json!({"url": url.trim()})));
                    }
                });
            }
            MenuInput::Offset(offset) => {
                ui.horizontal(|ui| {
                    let id = egui::Id::new("menu_input_offset");
                    if let Some(crate::widgets::FieldEdit::Set(v) | crate::widgets::FieldEdit::Scrub(v, _)) =
                        crate::widgets::number_field(ui, id, crate::widgets::Lead::None, Some(*offset), "", 120.0, 1.0)
                    {
                        *offset = v.clamp(-100_000.0, 100_000.0);
                    }
                    if crate::widgets::button(ui, tr("Offset"), true, true).clicked() {
                        action = Some(("vector.offset", json!({"offset": *offset})));
                    }
                });
            }
            MenuInput::MoveToPage => {
                let mut choices = 0;
                if let Ok(ds) = app.session.ds() {
                    for &page in &ds.doc.pages {
                        if page == ds.page || ds.doc.is_divider(page) {
                            continue;
                        }
                        if let Some(node) = ds.doc.node(page)
                            && crate::widgets::button(ui, &node.name, false, true).clicked()
                        {
                            action = Some(("object.moveToPage", json!({"page": page})));
                        }
                        choices += 1;
                    }
                }
                if choices == 0 {
                    crate::widgets::label(ui, tr("No other pages"), t.text_secondary);
                }
            }
        }
    });
    if let Some((id, params)) = action {
        app.run(id, params);
    } else if open {
        app.ui.menu_input = Some(input);
    }
}

/// The Rename layers dialog's fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchRenameUi {
    /// The layers, in stacking order (bottom first), as numbered.
    pub ids: Vec<NodeId>,
    /// Match: empty renames the whole name.
    pub find: String,
    pub regex: bool,
    /// Rename to.
    pub to: String,
    /// Where up numbers start and down numbers stop.
    pub start: u32,
}

/// Layers whose new names the dialog previews.
const PREVIEW_ROWS: usize = 5;

/// Open the Rename layers dialog for `ids` (several layers: Cmd/Ctrl+R, F2, the context menu).
pub(crate) fn open_batch_rename(app: &mut LizardApp, ids: &[NodeId]) {
    let Ok(doc) = app.session.doc() else { return };
    let ids = lizard_engine::cmd::edit::z_sorted(doc, ids);
    app.ui.batch_rename = Some(BatchRenameUi { ids, start: 1, ..Default::default() });
}

/// The rename the fields describe: a match replaced, or (no match) the whole name.
pub(crate) fn batch_rename_params(b: &BatchRenameUi) -> serde_json::Value {
    let mut p = json!({"ids": b.ids, "regex": b.regex, "start": b.start});
    if b.find.is_empty() {
        p["name"] = json!(b.to);
    } else {
        p["find"] = json!(b.find);
        p["replace"] = json!(b.to);
    }
    p
}

fn settings_modal(app: &mut LizardApp, ctx: &Context, t: &Tokens) {
    let screen = ctx.content_rect();
    let width = (screen.width() - 32.0).min(820.0).max(320.0);
    let height = (screen.height() - 32.0).min(560.0).max(320.0);
    let sidebar_width = if width < 600.0 { 150.0 } else { 188.0 };
    let id = Id::new("settings_modal");
    let section_id = id.with("section");
    let mut section = ctx.data(|data| data.get_temp::<SettingsSection>(section_id)).unwrap_or_default();
    let agent_state_id = Id::new("settings_agent_state");
    let mut agent_state = ctx.data(|data| data.get_temp::<AgentSettingsState>(agent_state_id)).unwrap_or_default();
    let mut close = false;

    let response = egui::Modal::new(id)
        .area(egui::Modal::default_area(id).constrain_to(screen))
        .backdrop_color(Color32::from_black_alpha(if t.dark { 120 } else { 75 }))
        .frame(egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(14)).shadow(egui::Shadow {
            offset: [0, 12],
            blur: 24,
            spread: 0,
            color: t.shadow,
        }))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let (header, _) = ui.allocate_exact_size(vec2(width, 56.0), Sense::hover());
            ui.painter().text(pos2(header.left() + 20.0, header.center().y), Align2::LEFT_CENTER, tr("Settings"), semibold(18.0), t.text);
            let close_rect = Rect::from_center_size(pos2(header.right() - 28.0, header.center().y), vec2(32.0, 32.0));
            if Button::icon(Icon::Close, tr("Close")).show_at(ui, id.with("close"), close_rect).clicked() {
                close = true;
            }
            ui.painter().line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, t.border));

            let (body, _) = ui.allocate_exact_size(vec2(width, height - 56.0), Sense::hover());
            let sidebar = Rect::from_min_max(body.min, pos2(body.left() + sidebar_width, body.bottom()));
            ui.painter().rect_filled(sidebar, CornerRadius { nw: 0, ne: 0, sw: 13, se: 0 }, t.panel);
            ui.painter().line_segment([sidebar.right_top(), sidebar.right_bottom()], Stroke::new(1.0, t.border));
            let nav_rect = sidebar.shrink2(vec2(12.0, 14.0));
            ui.scope_builder(UiBuilder::new().max_rect(nav_rect).layout(Layout::top_down(Align::Min)), |ui| {
                ui.set_width(nav_rect.width());
                ui.spacing_mut().item_spacing.y = 4.0;
                for choice in SettingsSection::ALL {
                    if Button::new(tr(choice.label()))
                        .leading(choice.icon())
                        .variant(ButtonVariant::Navigation)
                        .behavior(ButtonBehavior::Toggle { selected: section == choice })
                        .align(ButtonAlign::Start)
                        .size(ButtonSize::Large)
                        .width(ControlWidth::Fill)
                        .show(ui)
                        .clicked()
                    {
                        section = choice;
                    }
                }
            });

            let content_rect = Rect::from_min_max(pos2(sidebar.right() + 24.0, body.top() + 22.0), pos2(body.right() - 24.0, body.bottom() - 18.0));
            ui.scope_builder(UiBuilder::new().max_rect(content_rect).layout(Layout::top_down(Align::Min)), |ui| {
                ui.set_width(content_rect.width());
                egui::ScrollArea::vertical().id_salt("settings_content").auto_shrink([false, false]).max_height(content_rect.height()).show(
                    ui,
                    |ui| {
                        ui.set_width(content_rect.width() - 12.0);
                        ui.label(RichText::new(tr(section.label())).font(semibold(18.0)));
                        ui.add_space(16.0);
                        match section {
                            SettingsSection::General => settings_general(app, ui, t),
                            SettingsSection::Agents => settings_agents(app, ui, t, &mut agent_state),
                            SettingsSection::Appearance => settings_appearance(app, ui),
                            SettingsSection::Language => settings_language(app, ui, t),
                            SettingsSection::Sharing => settings_sharing(app, ui, t),
                        }
                    },
                );
            });
        });
    ctx.data_mut(|data| data.insert_temp(section_id, section));
    ctx.data_mut(|data| data.insert_temp(agent_state_id, agent_state));
    if close || response.should_close() {
        app.ui.preferences = false;
    }
}

fn settings_general(app: &mut LizardApp, ui: &mut egui::Ui, t: &Tokens) {
    let prefs = app.session.prefs.clone();
    let mut toggles = vec![];
    for (label, val, cmd) in [
        (tr("Keep tool selected after use"), prefs.keep_tool, "pref.keepTool"),
        (tr("Highlight layers on hover"), prefs.highlight_on_hover, "pref.highlightHover"),
        (tr("Rename duplicated layers"), prefs.rename_duplicates, "pref.renameDuplicates"),
        (tr("Show dimensions on objects"), prefs.show_dimensions, "pref.showDimensions"),
        (tr("Use number keys for opacity"), prefs.number_keys_opacity, "pref.numberKeysOpacity"),
        (tr("Reduce motion"), prefs.reduce_motion, "pref.reduceMotion"),
    ] {
        if crate::widgets::checkbox(ui, val, label) {
            toggles.push(cmd);
        }
    }
    if app.services.reduce_motion() {
        crate::widgets::label(ui, tr("Your system asks for reduced motion: animations stay off."), t.text_secondary);
    }
    if !cfg!(target_arch = "wasm32") && crate::widgets::checkbox(ui, prefs.auto_save, tr("Save files automatically")) {
        toggles.push("pref.autoSave");
    }
    if !cfg!(target_arch = "wasm32") && crate::widgets::checkbox(ui, prefs.start_on_home, tr("Start on the Home screen")) {
        toggles.push("pref.startOnHome");
    }
    for cmd in toggles {
        app.run(cmd, json!({}));
    }
    ui.add_space(18.0);
    ui.label(RichText::new(tr("Nudge amount")).font(semibold(12.0)));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let mut small = prefs.nudge_small;
        let mut big = prefs.nudge_big;
        ui.label(tr("Small"));
        let a = ui.add(egui::DragValue::new(&mut small).range(0.1..=100.0));
        ui.add_space(20.0);
        ui.label(tr("Large"));
        let b = ui.add(egui::DragValue::new(&mut big).range(1.0..=1000.0));
        if a.changed() || b.changed() {
            app.run("pref.nudge", json!({"small": small, "big": big}));
        }
    });
}

fn provider_title(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => tr_noop("OpenAI · Codex"),
        AgentKind::Claude => tr_noop("Anthropic · Claude Code"),
        AgentKind::Gemini => tr_noop("Google · Gemini CLI"),
        AgentKind::Kimi => tr_noop("Kimi Code"),
    }
}

fn provider_description(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => tr_noop("Connect your ChatGPT subscription."),
        AgentKind::Claude => tr_noop("Connect your Claude Pro or Max subscription."),
        AgentKind::Gemini => tr_noop("Connect your Google account."),
        AgentKind::Kimi => tr_noop("Connect your Kimi Code subscription."),
    }
}

fn provider_login_label(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Codex => tr_noop("Continue with ChatGPT"),
        AgentKind::Claude => tr_noop("Continue with Claude"),
        AgentKind::Gemini => tr_noop("Continue with Google"),
        AgentKind::Kimi => tr_noop("Continue with Kimi"),
    }
}

fn provider_status(connection: &AgentConnection, pending: bool, login_error: bool) -> &'static str {
    if pending {
        tr_noop("Connecting…")
    } else if connection.detail == "checking" {
        tr_noop("Checking account…")
    } else if !connection.installed {
        tr_noop("Setup needed")
    } else if login_error {
        tr_noop("Sign-in failed")
    } else if connection.authenticated {
        tr_noop("Connected")
    } else {
        tr_noop("Sign-in required")
    }
}

fn provider_emblem(ui: &egui::Ui, t: &Tokens, kind: AgentKind, rect: Rect) {
    ui.painter().rect_filled(rect, CornerRadius::same(8), t.surface_tertiary);
    icons::paint_provider(ui.painter(), rect.shrink(6.5), provider_index(kind), t.icon);
}

fn provider_row(
    ui: &mut egui::Ui,
    t: &Tokens,
    kind: AgentKind,
    connection: &AgentConnection,
    selected: bool,
    pending: bool,
    login_error: bool,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 78.0), Sense::click());
    let fill = if response.hovered() { t.hover } else { t.field };
    ui.painter().rect(rect, CornerRadius::same(9), fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let emblem = Rect::from_min_size(pos2(rect.left() + 14.0, rect.top() + 17.0), vec2(44.0, 44.0));
    provider_emblem(ui, t, kind, emblem);
    let title_x = emblem.right() + 14.0;
    ui.painter().text(pos2(title_x, rect.top() + 23.0), Align2::LEFT_CENTER, tr(provider_title(kind)), semibold(13.0), t.text);
    ui.painter().text(pos2(title_x, rect.top() + 51.0), Align2::LEFT_CENTER, tr(provider_description(kind)), regular(11.0), t.text_secondary);
    if selected {
        let dot = pos2(rect.right() - 18.0, rect.top() + 21.0);
        ui.painter().circle_filled(dot, 4.0, t.accent);
    }
    if width >= 470.0 {
        let label = tr(provider_status(connection, pending, login_error));
        let text_width = ui.painter().layout_no_wrap(label.to_owned(), semibold(11.0), t.text).size().x;
        let badge_width = text_width + 34.0;
        let badge = Rect::from_min_size(pos2(rect.right() - 16.0 - badge_width, rect.center().y - 13.0), vec2(badge_width, 26.0));
        let status_color = if login_error && connection.installed && !pending {
            t.danger
        } else if connection.authenticated {
            t.success
        } else {
            t.text_secondary
        };
        ui.painter().rect_filled(badge, CornerRadius::same(13), t.surface_tertiary);
        ui.painter().circle_filled(pos2(badge.left() + 12.0, badge.center().y), 3.5, status_color);
        ui.painter().text(pos2(badge.left() + 23.0, badge.center().y), Align2::LEFT_CENTER, label, semibold(11.0), t.text_secondary);
    }
    crate::a11y::button(&response, tr(provider_title(kind)));
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn settings_agents(app: &mut LizardApp, ui: &mut egui::Ui, t: &Tokens, state: &mut AgentSettingsState) {
    if state.login_was_pending && !app.ui.agent.login_pending {
        state.connections = Default::default();
        state.login_was_pending = false;
    }
    for kind in AGENT_PROVIDERS {
        let index = provider_index(kind);
        if state.connections[index].as_ref().is_none_or(|connection| connection.detail == "checking") {
            state.connections[index] = Some(app.services.agent_connection(kind));
            if state.connections[index].as_ref().is_some_and(|connection| connection.detail == "checking") {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
    }
    if let Some(kind) = state.detail {
        settings_agent_detail(app, ui, t, state, kind);
        return;
    }

    ui.label(RichText::new(tr("Connect AI providers")).font(semibold(18.0)));
    ui.add_space(6.0);
    crate::widgets::label(ui, tr("Connect an available provider, then choose it in the agent chat."), t.text_secondary);
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr("Available providers")).font(semibold(12.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if Button::new(tr("Refresh connections")).variant(ButtonVariant::Ghost).show(ui).clicked() {
                state.connections = Default::default();
                for kind in AGENT_PROVIDERS {
                    app.services.agent_refresh_connection(kind);
                }
                ui.ctx().request_repaint();
            }
        });
    });
    ui.add_space(12.0);
    for kind in AGENT_PROVIDERS {
        let index = provider_index(kind);
        if let Some(connection) = &state.connections[index]
            && provider_row(
                ui,
                t,
                kind,
                connection,
                app.ui.agent.agent == kind,
                app.ui.agent.login_pending && app.ui.agent.login_kind == Some(kind),
                app.ui.agent.login_error.is_some() && app.ui.agent.login_kind == Some(kind),
            )
        {
            state.detail = Some(kind);
        }
        ui.add_space(9.0);
    }
}

fn start_provider_login(app: &mut LizardApp, state: &mut AgentSettingsState, kind: AgentKind) {
    match app.services.agent_login(kind) {
        Ok(()) => {
            app.ui.agent.login_pending = true;
            app.ui.agent.login_kind = Some(kind);
            app.ui.agent.login_error = None;
            app.ui.agent.login_instruction = None;
            state.login_was_pending = true;
        }
        Err(error) => {
            app.ui.agent.login_kind = Some(kind);
            app.ui.agent.login_error = Some(error.clone());
            app.toast(error);
        }
    }
}

fn settings_agent_detail(app: &mut LizardApp, ui: &mut egui::Ui, t: &Tokens, state: &mut AgentSettingsState, kind: AgentKind) {
    if Button::new(tr("All providers")).leading(Icon::ChevronLeft).variant(ButtonVariant::Ghost).show(ui).clicked() {
        state.detail = None;
    }
    ui.add_space(20.0);
    ui.horizontal(|ui| {
        let (emblem, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
        provider_emblem(ui, t, kind, emblem);
        ui.add_space(14.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(tr(provider_title(kind))).font(semibold(19.0)));
            ui.add_space(6.0);
            crate::widgets::label(ui, tr(provider_description(kind)), t.text_secondary);
        });
    });
    ui.add_space(28.0);
    ui.label(RichText::new(tr("Connection")).font(semibold(12.0)));
    ui.add_space(10.0);

    let index = provider_index(kind);
    let Some(connection) = state.connections[index].clone() else { return };
    let pending = app.ui.agent.login_pending && app.ui.agent.login_kind == Some(kind);
    let login_error = app.ui.agent.login_error.is_some() && app.ui.agent.login_kind == Some(kind);
    let status = tr(provider_status(&connection, pending, login_error));
    let card_width = ui.available_width();
    egui::Frame::NONE
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_width((card_width - 32.0).max(0.0));
            ui.label(RichText::new(status).font(semibold(13.0)));
            ui.add_space(6.0);
            let description = if connection.detail == "checking" {
                tr("Checking your saved account…")
            } else if !connection.installed {
                tr("Lizard will download the provider helper, then open sign-in.")
            } else if connection.authenticated {
                tr("Your account is connected. You can use this provider in the agent chat.")
            } else if pending {
                tr("Lizard is preparing sign-in. Continue in your browser when it opens.")
            } else if login_error {
                tr("Sign-in was not completed. Try again.")
            } else {
                tr("Sign in with your provider account to use your subscription in Lizard.")
            };
            crate::widgets::label(ui, description, t.text_secondary);
        });
    if pending && let Some(instruction) = &app.ui.agent.login_instruction {
        ui.add_space(12.0);
        crate::widgets::label(ui, tr("Enter this code on the provider's page:"), t.text_secondary);
        ui.horizontal(|ui| {
            ui.label(RichText::new(instruction).font(semibold(16.0)));
            ui.add_space(12.0);
            if Button::new(tr("Copy code")).variant(ButtonVariant::Secondary).show(ui).clicked() {
                app.services.clipboard_set_text(instruction);
            }
        });
    }
    ui.add_space(20.0);

    ui.horizontal(|ui| {
        if !connection.authenticated {
            if Button::new(tr(if pending { "Connecting…" } else { provider_login_label(kind) }))
                .enabled(!app.ui.agent.login_pending)
                .show(ui)
                .clicked()
            {
                start_provider_login(app, state, kind);
            }
        } else {
            if Button::new(tr("Use in chat")).show(ui).clicked() {
                crate::agent_panel::select_agent(app, kind);
                app.select_left_tab(crate::LeftTab::Agent);
                app.ui.preferences = false;
            }
        }
        if Button::new(tr("Check again")).variant(ButtonVariant::Ghost).show(ui).clicked() {
            state.connections[index] = None;
            app.services.agent_refresh_connection(kind);
            ui.ctx().request_repaint();
        }
    });
    ui.add_space(26.0);
    ui.label(RichText::new(tr("Advanced setup")).font(semibold(12.0)));
    ui.add_space(6.0);
    crate::widgets::label(ui, tr("If you prefer to use a provider's command-line tool, you can set it up separately."), t.text_secondary);
    ui.add_space(8.0);
    if Button::new(tr("Open setup guide")).variant(ButtonVariant::Secondary).show(ui).clicked() {
        app.services.open_url(crate::agent_panel::setup_url(kind));
    }
}

fn settings_appearance(app: &mut LizardApp, ui: &mut egui::Ui) {
    let prefs = app.session.prefs.clone();
    ui.label(RichText::new(tr("Theme")).font(semibold(12.0)));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (label, value, cmd) in [
            (tr_noop("Light"), "light", "view.themeLight"),
            (tr_noop("Dark"), "dark", "view.themeDark"),
            (tr_noop("System"), "system", "view.themeSystem"),
        ] {
            if Button::new(tr(label))
                .variant(ButtonVariant::Ghost)
                .behavior(ButtonBehavior::Toggle { selected: prefs.theme.eq_ignore_ascii_case(value) })
                .show(ui)
                .clicked()
            {
                app.run(cmd, json!({}));
            }
            ui.add_space(8.0);
        }
    });
    ui.add_space(20.0);
    interface_size(app, ui, prefs.ui_scale);
}

fn settings_language(app: &mut LizardApp, ui: &mut egui::Ui, t: &Tokens) {
    crate::widgets::label(ui, tr("Choose the language used throughout Lizard."), t.text_secondary);
    ui.add_space(12.0);
    let langs =
        || [("system", tr("System"))].into_iter().chain(i18n::languages().iter().filter(|l| crate::cjk::can_show(l.code)).map(|l| (l.code, l.name)));
    let on = |code: &str| app.session.prefs.language.eq_ignore_ascii_case(code);
    let selected = langs().find(|(code, _)| on(code)).map_or(tr("System"), |(_, name)| name);
    let resp = crate::primitives::Select::trigger(ui, selected, ui.available_width().min(280.0), None);
    let mut pick = None;
    crate::right::menu_popup(&resp).show(|ui| {
        for (code, name) in langs() {
            if crate::right::menu_check(ui, on(code), name, "") {
                pick = Some(code);
                ui.close();
            }
        }
    });
    if let Some(code) = pick {
        app.run("pref.language", json!({"language": code}));
        ui.ctx().request_repaint();
    }
    if let Some(lang) = i18n::current().filter(|l| l.draft) {
        ui.add_space(12.0);
        let note = trf("{language}: draft translation, waiting for a native speaker's review.", &[("language", &lang.name)]);
        crate::widgets::label(ui, &note, t.text_secondary);
    }
}

fn settings_sharing(app: &mut LizardApp, ui: &mut egui::Ui, t: &Tokens) {
    let prefs = app.session.prefs.clone();
    if let Some(on) = app.session.invite_links_on() {
        if crate::widgets::checkbox(ui, on, tr("Open invite links in Lizard")) {
            app.run("collab.registerLinks", json!({"on": !on}));
        }
        crate::widgets::label(ui, tr("lizard:// links from a browser, chat or email open this app and join the session."), t.text_secondary);
        ui.add_space(18.0);
    }
    ui.label(RichText::new(tr("Your name in comments")).font(semibold(12.0)));
    ui.add_space(8.0);
    if let Some(name) = crate::widgets::text_field(ui, Id::new("pref_user_name"), &prefs.user_name, ui.available_width().min(320.0), tr("You")) {
        app.run("pref.userName", json!({"name": name}));
    }
    ui.add_space(20.0);
    let name = tr("Server for online sharing");
    ui.label(RichText::new(name).font(semibold(12.0)));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if let Some(url) =
            crate::widgets::text_field_named(ui, Id::new("pref_publish_server"), &prefs.publish_server, ui.available_width().min(320.0), name)
        {
            app.run("pref.publishServer", json!({"url": url}));
        }
        let usual = prefs.publish_server == lizard_engine::live::DEFAULT_SERVER;
        if crate::widgets::button(ui, tr("Reset"), false, !usual).on_hover_text(tr("Clear saved server")).clicked() {
            app.run("pref.publishServer", json!({}));
        }
    });
    if prefs.publish_server.is_empty() {
        ui.add_space(8.0);
        crate::widgets::label(ui, tr("Online sharing is off: files are shared on the local network only."), t.text_secondary);
    }
}

/// Preferences ▸ Interface size: a size relative to the system's display scale (100 % is the
/// system's own), as a drop-down that takes effect at once. The browser has its own zoom, so there
/// the row only says so.
fn interface_size(app: &mut LizardApp, ui: &mut egui::Ui, chosen: f64) {
    let t = Tokens::get(ui.ctx());
    if cfg!(target_arch = "wasm32") {
        ui.label(tr("Interface size"));
        crate::widgets::label(ui, tr("Use your browser's zoom to change the interface size."), t.text_secondary);
        return;
    }
    let percent = |size: f64| trf("{percent}%", &[("percent", &crate::ui_scale::percent(size as f32))]);
    let shown = percent(chosen);
    ui.horizontal(|ui| {
        ui.label(tr("Interface size"));
        let resp = crate::primitives::Select::trigger(ui, &shown, 120.0, None);
        crate::a11y::combo(&resp, tr("Interface size"), &shown, crate::a11y::menu_open(&resp));
        crate::right::menu_popup(&resp).show(|ui| {
            for size in crate::ui_scale::SIZES {
                if crate::right::menu_check(ui, (chosen - size).abs() < 1e-6, &percent(size), "") {
                    app.run("pref.uiScale", json!({"scale": size}));
                    ui.close();
                }
            }
        });
    });
    crate::widgets::label(
        ui,
        tr("Relative to your system's display scaling (100% is the system's own size). Document zoom is separate."),
        t.text_secondary,
    );
    // Said in words, not only by the menu's value: the window is too small for the size chosen.
    if app.ui.ui_scale.limited {
        let note = trf(
            "The window is too small for {size}%, so the interface is drawn at {drawn}%. Make the window larger to use {size}%.",
            &[("size", &crate::ui_scale::percent(chosen as f32)), ("drawn", &crate::ui_scale::percent(app.ui.ui_scale.zoom))],
        );
        crate::widgets::label(ui, &note, t.text_secondary);
    }
}

/// Rename layers: Match, Rename to with buttons for the current name and numbers, where numbers
/// start, a preview of the first new names, then one `object.batchRename`.
fn batch_rename_dialog(app: &mut LizardApp, ctx: &Context) {
    let Some(mut b) = app.ui.batch_rename.clone() else { return };
    let t = Tokens::get(ctx);
    let find = (!b.find.is_empty()).then_some(b.find.as_str());
    let rename = lizard_engine::rename::BatchRename::new(find, b.regex, &b.to, find.is_none().then_some(b.to.as_str()), Some(f64::from(b.start)));
    let n = b.ids.len();
    let ready = rename.is_ok() && !(b.find.is_empty() && b.to.is_empty());
    let preview: Vec<(String, String)> = match (&rename, app.session.doc()) {
        (Ok(r), Ok(doc)) if ready => b
            .ids
            .iter()
            .enumerate()
            .take(PREVIEW_ROWS)
            .filter_map(|(k, i)| doc.node(*i).map(|node| (node.name.clone(), r.apply(&node.name, k, n))))
            .collect(),
        _ => Vec::new(),
    };
    let mut open = true;
    let mut cancel = false;
    let mut apply = false;
    window(tr("Rename layers"), ctx).id(egui::Id::new("Rename layers")).open(&mut open).show(ctx, |ui| {
        ui.set_width(320.0);
        ui.label(RichText::new(trn("{n} layers", n, &[])).color(t.text_secondary));
        ui.add_space(6.0);
        let r = ui.add(egui::TextEdit::singleline(&mut b.find).hint_text(tr("Match (empty: the whole name)")).desired_width(300.0));
        crate::widgets::select_all_on_focus(ui, &r, &b.find);
        if crate::widgets::checkbox(ui, b.regex, tr("Regular expression")) {
            b.regex = !b.regex;
        }
        let to = ui.add(egui::TextEdit::singleline(&mut b.to).hint_text(tr("Rename to")).desired_width(300.0));
        crate::widgets::select_all_on_focus(ui, &to, &b.to);
        ui.horizontal(|ui| {
            for (label, code) in [(tr("Current name"), "$&"), (tr("Number ↑"), "$n"), (tr("Number ↓"), "$N")] {
                if ui.button(label).clicked() {
                    b.to.push_str(code);
                    to.request_focus();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("Start ascending from / stop descending at"));
            ui.add(egui::DragValue::new(&mut b.start).range(0..=1_000_000));
        });
        ui.add_space(8.0);
        match &rename {
            Err(e) => {
                ui.label(RichText::new(e).color(t.danger));
            }
            Ok(_) => {
                for (old, new) in &preview {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(old).color(t.text_secondary));
                        ui.label("→");
                        ui.label(new);
                    });
                }
                if ready && n > PREVIEW_ROWS {
                    ui.label(RichText::new(trn("and {n} more", n - PREVIEW_ROWS, &[])).color(t.text_secondary));
                }
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            cancel = crate::widgets::button(ui, tr("Cancel"), false, true).clicked();
            let enter = to.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            apply = (crate::widgets::button(ui, tr("Rename"), true, ready).clicked() || enter) && ready;
        });
    });
    if apply {
        app.run("object.batchRename", batch_rename_params(&b));
    }
    app.ui.batch_rename = (open && !cancel && !apply).then_some(b);
}

/// File ▸ Color profile: pick sRGB or Display P3, then keep the colours' appearance (convert the
/// values) or keep the values (reinterpret them in the new space).
fn color_profile_dialog(app: &mut LizardApp, ctx: &Context) {
    let Some(mut chosen) = app.ui.color_profile else { return };
    let Ok(current) = app.session.doc().map(|d| d.color_profile) else {
        app.ui.color_profile = None;
        return;
    };
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut apply = None;
    window(tr("Color profile"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(RichText::new(tr("Color values in this file are in this color space.")).color(t.text_secondary).font(regular(11.0)));
        ui.add_space(8.0);
        for p in ColorProfile::ALL {
            let label = if p == current { trf("{profile} (current)", &[("profile", &p.label())]) } else { p.label().to_string() };
            if crate::widgets::radio(ui, chosen == p, &label) {
                chosen = p;
            }
        }
        ui.add_space(8.0);
        if chosen == current {
            ui.label(RichText::new(tr("Choose another profile to change it.")).color(t.text_secondary).font(regular(11.0)));
            return;
        }
        ui.label(
            RichText::new(trf(
                "Keep appearance converts every color value to {profile} so colors look the same. Keep values leaves the numbers as they are, so colors shift.",
                &[("profile", &chosen.label())],
            ))
            .font(regular(11.0)),
        );
        if !chosen.is_srgb() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(tr("The canvas previews colors in sRGB; exported PNG, JPG and WebP files are tagged Display P3."))
                    .color(t.text_secondary)
                    .font(regular(11.0)),
            );
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if crate::widgets::button(ui, tr("Keep appearance"), true, true).clicked() {
                apply = Some(true);
            }
            if crate::widgets::button(ui, tr("Keep values"), false, true).clicked() {
                apply = Some(false);
            }
        });
    });
    app.ui.color_profile = open.then_some(chosen);
    if let Some(convert) = apply {
        app.run("file.colorProfile", json!({"profile": chosen.id(), "convert": convert}));
        app.ui.color_profile = None;
    }
}

/// A `.fig` Save as result is an export; only native saves report that the document was saved.
pub(crate) fn file_saved(app: &mut LizardApp, result: &Value) {
    if result["format"] == "fig" {
        fig_exported(app, result);
    } else {
        app.toast(trf("Saved {name}", &[("name", &result_file_name(result))]));
    }
}

fn result_file_name(result: &Value) -> String {
    result["path"].as_str().and_then(|p| Path::new(p).file_name()).map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The `.fig` result stays an export: native save state is unchanged, and omissions are reported.
pub(crate) fn fig_exported(app: &mut LizardApp, result: &Value) {
    let file = result_file_name(result);
    let left_out = result["warnings"].as_array().map_or(0, Vec::len);
    app.toast(match left_out {
        0 => trf("Exported {name}", &[("name", &file)]),
        n => trn("Exported {name}; {n} kinds of content were left out", n, &[("name", &file)]),
    });
}

/// Save the whole file as `.fig`; the toast says how much was left out.
fn save_fig(app: &mut LizardApp, name: &str) {
    let filename = format!("{name}.fig");
    app.pick_save(tr("Export"), &filename, &["fig"], |app, p| {
        if let Some(r) = app.run("file.export", json!({"format": "fig", "path": p.to_string_lossy()})) {
            fig_exported(app, &r);
        }
    });
}

/// What the Export dialog was asked to do.
enum ExportChoice {
    /// Every layer with export settings, with those settings.
    Layers,
    /// The layers together as one file.
    Once(ExportSetting),
    /// The whole file as `.fig` (experimental).
    Fig,
}

/// The Export dialog: the layers' own export settings when they have some, and one-off presets for
/// the selection (or the page) as one file.
fn export_dialog(app: &mut LizardApp, ctx: &Context) {
    let t = Tokens::get(ctx);
    let mut open = true;
    let sel = app.session.selection();
    let Ok(ds) = app.session.ds() else {
        app.ui.export_preview = false;
        return;
    };
    let (doc, page) = (ds.doc.clone(), ds.page);
    let ids = if sel.is_empty() { doc.node(page).map(|p| p.children.clone()).unwrap_or_default() } else { sel.clone() };
    // With nothing selected, every layer on the page that has export settings.
    let with_settings = if sel.is_empty() { exportable_layers(&doc, page) } else { sel };
    let jobs = export_jobs(&doc, &with_settings);
    let print = app.ui.pdf_print;
    let preset = |size: f64, format| ExportSetting { size: ExportSize::Scale { value: size }, format, print, ..Default::default() };
    let presets = [
        preset(1.0, ExportFormat::Png),
        preset(2.0, ExportFormat::Png),
        preset(3.0, ExportFormat::Png),
        preset(2.0, ExportFormat::Jpg),
        preset(2.0, ExportFormat::Webp),
        preset(1.0, ExportFormat::Svg),
        preset(1.0, ExportFormat::Pdf),
    ];
    let mut choice = None;
    window(tr("Export"), ctx).open(&mut open).show(ctx, |ui| {
        ui.set_width(320.0);
        let caption = |ui: &mut egui::Ui, s: &str| {
            ui.label(RichText::new(s).font(regular(11.0)).color(t.text_secondary));
        };
        let divider = |ui: &mut egui::Ui| {
            ui.add_space(10.0);
            let (r, _) = ui.allocate_exact_size(vec2(320.0, 1.0), Sense::hover());
            ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(1.0, t.border));
            ui.add_space(10.0);
        };
        let row = |ui: &mut egui::Ui, label: &str| {
            let (r, resp) = ui.allocate_exact_size(vec2(320.0, 30.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(5), t.hover);
            }
            ui.painter().text(r.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, label, regular(12.0), t.text);
            resp.clicked()
        };
        if !jobs.is_empty() {
            caption(ui, tr("With the layers' export settings"));
            const SHOWN: usize = 6;
            for j in jobs.iter().take(SHOWN) {
                ui.label(RichText::new(&j.file).font(regular(12.0)).color(t.text));
            }
            if jobs.len() > SHOWN {
                caption(ui, &trn("and {n} more", jobs.len() - SHOWN, &[]));
            }
            ui.add_space(6.0);
            let layers = jobs.iter().map(|j| j.id).collect::<std::collections::BTreeSet<_>>().len();
            let label = trn("Export {n} layers", layers, &[]);
            if crate::widgets::button(ui, &label, true, true).clicked() {
                choice = Some(ExportChoice::Layers);
            }
            divider(ui);
        }
        caption(ui, &trn("{n} layers as one file", ids.len(), &[]));
        ui.add_space(4.0);
        for p in &presets {
            let label = if p.format.is_raster() { format!("{} @{}", p.format.label(), p.size.label()) } else { p.format.label().to_string() };
            if row(ui, &label) {
                choice = Some(ExportChoice::Once(p.clone()));
            }
        }
        // The PDF preset's print options.
        ui.add_space(6.0);
        caption(ui, tr("PDF print options"));
        if let Some(next) = crate::export_panel::print_options(ui, egui::Id::new("export_dialog_print"), &print) {
            app.ui.pdf_print = next;
        }
        divider(ui);
        caption(ui, tr("The whole file"));
        ui.add_space(4.0);
        if row(ui, tr(".fig (experimental)")) {
            choice = Some(ExportChoice::Fig);
        }
        caption(ui, tr("Other apps may not open every .fig file exported here."));
    });
    let close = choice.is_some() || !open;
    match choice {
        Some(ExportChoice::Layers) => export_layers(app, &with_settings, false),
        Some(ExportChoice::Once(s)) if !ids.is_empty() => save_one(app, &ids, &s, &suggested_name(&doc, &ids, &s, "Export")),
        Some(ExportChoice::Once(_)) => app.toast(tr("Nothing to export")),
        Some(ExportChoice::Fig) => save_fig(app, &doc.name),
        None => {}
    }
    if close {
        app.ui.export_preview = false;
    }
}
