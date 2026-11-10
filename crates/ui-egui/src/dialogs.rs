//! Dialogs: Preferences, find & replace, rename layers, variables,
//! export.

use crate::primitives::TooltipResponseExt as _;
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
use crate::icons::{self, Icon, SettingsIcon};
use crate::primitives::{
    Button, ButtonAlign, ButtonBehavior, ButtonSize, ButtonVariant, ControlWidth, FloatingPanel, FloatingPanelHeight, FloatingPanelWidth,
    NumberField, SearchField, Switch,
};
use crate::theme::{Tokens, regular, semibold};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SettingsSection {
    #[default]
    General,
    Agents,
    Appearance,
    Language,
    Sharing,
    Shortcuts,
}

impl SettingsSection {
    const ALL: [Self; 6] = [Self::General, Self::Agents, Self::Appearance, Self::Language, Self::Sharing, Self::Shortcuts];

    fn label(self) -> &'static str {
        match self {
            Self::General => tr_noop("General"),
            Self::Agents => tr_noop("Agents"),
            Self::Appearance => tr_noop("Appearance"),
            Self::Language => tr_noop("Language"),
            Self::Sharing => tr_noop("Sharing"),
            Self::Shortcuts => tr_noop("Keyboard shortcuts"),
        }
    }

    fn icon(self) -> SettingsIcon {
        match self {
            Self::General => SettingsIcon::General,
            Self::Agents => SettingsIcon::Agents,
            Self::Appearance => SettingsIcon::Appearance,
            Self::Language => SettingsIcon::Language,
            Self::Sharing => SettingsIcon::Sharing,
            Self::Shortcuts => SettingsIcon::Shortcuts,
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

/// A row of tabs: the current one highlighted.
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
        || u.preferences;
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
    for flag in [&mut u.confirm_close, &mut u.export_preview, &mut u.preferences] {
        if std::mem::take(flag) {
            return;
        }
    }
}

pub fn show(app: &mut LizardApp, ctx: &Context) {
    crate::updates::show(app, ctx);
    // Dropped files waiting to open: their card, then (once it has shown) the files.
    crate::pending_open::pending_opens(app, ctx);
    crate::import_report::show(app, ctx);
    crate::missing_fonts::show(app, ctx);
    let t = Tokens::get(ctx);
    if app.ui.preferences {
        settings_modal(app, ctx, &t);
    } else {
        ctx.data_mut(|data| data.remove::<AgentSettingsState>(Id::new("settings_agent_state")));
    }
    find_replace_dialog(app, ctx);
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

fn find_replace_dialog(app: &mut LizardApp, ctx: &Context) {
    let Some((mut find, mut replace)) = app.ui.find.clone() else { return };
    let panel_id = Id::new("find_replace_panel");
    let expanded_id = panel_id.with("expanded");
    let mut expanded = ctx.data(|data| data.get_temp::<bool>(expanded_id)).unwrap_or(true);
    let mut open = true;
    let mut close_requested = false;
    let mut action = None;
    FloatingPanel::new(panel_id, pos2(260.0, 120.0))
        .width(FloatingPanelWidth::Fixed(340.0))
        .height(FloatingPanelHeight::Fixed(if expanded { 160.0 } else { 44.0 }))
        .header_height(44.0)
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            let t = Tokens::get(ui.ctx());
            ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));
            ui.painter().text(header.center(), Align2::CENTER_CENTER, tr("Find and replace"), semibold(13.0), t.text);
            let collapse = Rect::from_center_size(pos2(header.left() + 24.0, header.center().y), vec2(24.0, 24.0));
            if Button::icon(if expanded { Icon::Chevron } else { Icon::ChevronRight }, if expanded { tr("Collapse") } else { tr("Expand") })
                .icon_size(14.0)
                .show_at(ui, panel_id.with("collapse"), collapse)
                .clicked()
            {
                expanded = !expanded;
            }
            let close = Rect::from_center_size(pos2(header.right() - 24.0, header.center().y), vec2(24.0, 24.0));
            close_requested = Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, panel_id.with("close"), close).clicked();
            if !expanded {
                return;
            }
            let body_rect = Rect::from_min_size(pos2(header.left() + 16.0, header.bottom() + 12.0), vec2(308.0, 92.0));
            let mut body = ui.new_child(UiBuilder::new().max_rect(body_rect).layout(Layout::top_down(Align::Min)));
            body.spacing_mut().item_spacing.y = 8.0;
            let find_response =
                SearchField::new(panel_id.with("find"), tr("Find")).width(ControlWidth::Fill).compact().clearable().show(&mut body, &mut find);
            crate::widgets::select_all_on_focus(&body, &find_response, &find);
            if find_response.has_focus() && body.input(|input| input.key_pressed(egui::Key::Enter)) {
                action = Some(false);
            }
            let replace_response = crate::widgets::text_input(&mut body, panel_id.with("replace"), &mut replace, 308.0, tr("Replace with"));
            if replace_response.has_focus() && body.input(|input| input.key_pressed(egui::Key::Enter)) {
                action = Some(true);
            }
            body.add_space(4.0);
            body.horizontal(|ui| {
                if Button::new(tr("Find")).variant(ButtonVariant::Secondary).enabled(!find.is_empty()).show(ui).clicked() {
                    action = Some(false);
                }
                if Button::new(tr("Replace all")).variant(ButtonVariant::Secondary).enabled(!find.is_empty()).show(ui).clicked() {
                    action = Some(true);
                }
            });
        });
    ctx.data_mut(|data| data.insert_temp(expanded_id, expanded));
    if let Some(rep) = action
        && !find.is_empty()
    {
        let result = if rep {
            app.run("edit.findReplace", json!({"find": find, "replace": replace, "layerNames": false}))
        } else {
            app.run("edit.findReplace", json!({"find": find, "layerNames": true}))
        };
        if let Some(value) = result {
            let count = value["matches"].as_array().map(|matches| matches.len()).unwrap_or(0);
            app.toast(trn("{n} matches", count, &[]));
        }
    }
    app.ui.find = if open && !close_requested { Some((find, replace)) } else { None };
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
    let width = (screen.width() - 32.0).clamp(320.0, 820.0);
    let height = (screen.height() - 32.0).clamp(320.0, 560.0);
    let sidebar_width = if width < 600.0 { 150.0 } else { 188.0 };
    let id = Id::new("settings_modal");
    let section_id = id.with("section");
    let mut section = ctx.data(|data| data.get_temp::<SettingsSection>(section_id)).unwrap_or_default();
    let shortcuts_requested = std::mem::take(&mut app.ui.settings_shortcuts_requested);
    let profile_requested = std::mem::take(&mut app.ui.settings_profile_requested);
    if shortcuts_requested {
        section = SettingsSection::Shortcuts;
    } else if profile_requested {
        section = SettingsSection::Sharing;
    }
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
                        .leading_settings(choice.icon())
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
                            SettingsSection::Shortcuts => crate::settings_shortcuts::show(app, ui, t),
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
        if Switch::new(ui.id().with(cmd), val, label).show(ui).clicked() {
            toggles.push(cmd);
        }
        ui.add_space(8.0);
    }
    if app.services.reduce_motion() {
        crate::widgets::label(ui, tr("Your system asks for reduced motion: animations stay off."), t.text_secondary);
    }
    if !cfg!(target_arch = "wasm32") {
        for (label, value, cmd) in [
            (tr("Save files automatically"), prefs.auto_save, "pref.autoSave"),
            (tr("Start on the Home screen"), prefs.start_on_home, "pref.startOnHome"),
        ] {
            if Switch::new(ui.id().with(cmd), value, label).show(ui).clicked() {
                toggles.push(cmd);
            }
            ui.add_space(8.0);
        }
    }
    for cmd in toggles {
        app.run(cmd, json!({}));
    }
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(20.0);
    ui.label(RichText::new(tr("Nudge amount")).font(semibold(13.0)));
    ui.add_space(14.0);
    let gap = 16.0;
    let field_width = ((ui.available_width() - gap) / 2.0).max(96.0);
    ui.horizontal(|ui| {
        for (index, label, value, key, step) in [(0, tr("Small"), prefs.nudge_small, "small", 1.0), (1, tr("Large"), prefs.nudge_big, "big", 10.0)] {
            if index > 0 {
                ui.add_space(gap);
            }
            ui.vertical(|ui| {
                ui.set_width(field_width);
                ui.label(RichText::new(label).font(regular(11.0)).color(t.text_secondary));
                ui.add_space(7.0);
                let field = NumberField::new(ui.id().with(("nudge", key)), crate::widgets::Lead::Named(label), Some(value), "px")
                    .step(step)
                    .show(ui, ControlWidth::Fill);
                if let Some(edit) = field.edit.and_then(|edit| edit.settled()) {
                    let next = match edit {
                        crate::widgets::FieldEdit::Set(value) | crate::widgets::FieldEdit::Scrub(value, _) => value,
                    };
                    let params = if key == "small" { json!({"small": next}) } else { json!({"big": next}) };
                    app.run("pref.nudge", params);
                }
            });
        }
    });
    if !cfg!(target_arch = "wasm32") {
        ui.add_space(20.0);
        ui.separator();
        ui.add_space(20.0);
        ui.label(RichText::new(tr("Local image processing")).font(semibold(13.0)));
        ui.add_space(8.0);
        crate::widgets::label(
            ui,
            tr("Background removal uses a local model. First use may download about 1.5 GB; images stay on this computer."),
            t.text_secondary,
        );
    }
    crate::updates::settings(app, ui);
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

fn provider_row(ui: &mut egui::Ui, t: &Tokens, kind: AgentKind, connection: &AgentConnection, pending: bool, login_error: bool) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 78.0), Sense::click());
    let fill = if response.hovered() { t.hover } else { t.field };
    ui.painter().rect(rect, CornerRadius::same(9), fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let emblem = Rect::from_min_size(pos2(rect.left() + 14.0, rect.top() + 17.0), vec2(44.0, 44.0));
    provider_emblem(ui, t, kind, emblem);
    let title_x = emblem.right() + 14.0;
    ui.painter().text(pos2(title_x, rect.top() + 23.0), Align2::LEFT_CENTER, tr(provider_title(kind)), semibold(13.0), t.text);
    ui.painter().text(pos2(title_x, rect.top() + 51.0), Align2::LEFT_CENTER, tr(provider_description(kind)), regular(11.0), t.text_secondary);
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
            if pending && Button::new(tr("Cancel")).variant(ButtonVariant::Secondary).show(ui).clicked() {
                crate::agent_panel::cancel_login(app);
                app.services.agent_refresh_connection(kind);
                state.connections[index] = None;
                state.login_was_pending = false;
                ui.ctx().request_repaint();
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
    ui.label(RichText::new(tr("Theme")).font(semibold(13.0)));
    ui.add_space(14.0);
    let gap = 14.0;
    let card_width = ((ui.available_width() - 2.0 * gap) / 3.0).clamp(88.0, 172.0);
    ui.horizontal(|ui| {
        for (label, value, cmd) in [
            (tr_noop("Light"), "light", "view.themeLight"),
            (tr_noop("Dark"), "dark", "view.themeDark"),
            (tr_noop("System"), "system", "view.themeSystem"),
        ] {
            let selected = prefs.theme.eq_ignore_ascii_case(value);
            if theme_preview_card(ui, tr(label), value, selected, card_width).clicked() {
                app.run(cmd, json!({}));
            }
            ui.add_space(gap);
        }
    });
    ui.add_space(28.0);
    ui.separator();
    ui.add_space(22.0);
    interface_size(app, ui, prefs.ui_scale);
}

/// A miniature window makes each theme understandable before applying it.
fn theme_preview_card(ui: &mut egui::Ui, label: &str, theme: &str, selected: bool, width: f32) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(width, 150.0), Sense::click());
    crate::a11y::radio(&response, label, selected);
    let preview = Rect::from_min_size(rect.min, vec2(width, 116.0));
    let light = Color32::from_rgb(238, 238, 241);
    let dark = Color32::from_rgb(64, 64, 69);
    let light_window = Color32::from_rgb(252, 252, 253);
    let dark_window = Color32::from_rgb(29, 29, 32);
    let painter = ui.painter();
    painter.rect_filled(preview, CornerRadius::same(14), if theme == "light" { light } else { dark });
    if theme == "system" {
        let right = Rect::from_min_max(pos2(preview.center().x, preview.top()), preview.max);
        painter.rect_filled(right, CornerRadius { nw: 0, ne: 14, sw: 0, se: 14 }, light);
    }
    let window = Rect::from_min_max(pos2(preview.left() + 22.0, preview.top() + 33.0), pos2(preview.right() - 12.0, preview.bottom() + 15.0));
    let clipped = painter.with_clip_rect(preview);
    clipped.rect_filled(window, CornerRadius::same(12), if theme == "light" { light_window } else { dark_window });
    if theme == "system" {
        let right = Rect::from_min_max(pos2(preview.center().x, window.top()), window.max);
        clipped.rect_filled(right, CornerRadius { nw: 0, ne: 12, sw: 0, se: 12 }, light_window);
    }
    let window_border = if theme == "light" { Color32::from_rgb(213, 213, 218) } else { Color32::from_rgb(86, 86, 91) };
    clipped.rect_stroke(window, CornerRadius::same(12), Stroke::new(1.0, window_border), egui::StrokeKind::Inside);
    let sample_y = window.top() + 25.0;
    if theme == "system" {
        for (x, color) in [(window.left() + 17.0, Color32::WHITE), (preview.center().x + 12.0, Color32::from_rgb(30, 30, 33))] {
            clipped.text(pos2(x, sample_y), Align2::LEFT_CENTER, "Aa", semibold(22.0), color);
        }
    } else {
        let ink = if theme == "light" { Color32::from_rgb(30, 30, 33) } else { Color32::WHITE };
        clipped.text(pos2(window.left() + 17.0, sample_y), Align2::LEFT_CENTER, "Aa", semibold(22.0), ink);
    }
    painter.rect_stroke(
        preview,
        CornerRadius::same(14),
        Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { t.accent } else { t.border_strong }),
        egui::StrokeKind::Inside,
    );
    if selected {
        let center = pos2(preview.right() - 16.0, preview.bottom() - 16.0);
        painter.circle_filled(center, 11.0, t.accent);
        icons::paint(painter, Rect::from_center_size(center, vec2(14.0, 14.0)), Icon::Check, t.accent_on);
    }
    painter.text(pos2(rect.left(), preview.bottom() + 18.0), Align2::LEFT_CENTER, label, semibold(12.0), t.text);
    if response.hovered() && !selected {
        painter.rect_stroke(preview, CornerRadius::same(14), Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    }
    if response.has_focus() {
        painter.rect_stroke(preview.expand(2.0), CornerRadius::same(16), Stroke::new(1.0, t.focus), egui::StrokeKind::Outside);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn settings_language(app: &mut LizardApp, ui: &mut egui::Ui, t: &Tokens) {
    crate::widgets::label(ui, tr("Choose the language used throughout Lizard."), t.text_secondary);
    ui.add_space(16.0);
    let search_id = Id::new("settings_language_search");
    let query_id = search_id.with("query");
    let mut search = ui.ctx().data(|data| data.get_temp::<String>(query_id)).unwrap_or_default();
    SearchField::new(search_id, tr("Search languages")).clearable().width(ControlWidth::Fill).show(ui, &mut search);
    ui.ctx().data_mut(|data| data.insert_temp(query_id, search.clone()));
    ui.add_space(12.0);

    let needle = search.trim().to_lowercase();
    let matches = |code: &str, name: &str| {
        needle.is_empty()
            || name.to_lowercase().contains(&needle)
            || code.to_lowercase().contains(&needle)
            || language_search_alias(code).contains(&needle)
    };
    let languages = [("system", tr("System"))]
        .into_iter()
        .chain(i18n::languages().iter().filter(|lang| crate::cjk::can_show(lang.code)).map(|lang| (lang.code, lang.name)))
        .filter(|(code, name)| matches(code, name))
        .collect::<Vec<_>>();
    let mut pick = None;
    if languages.is_empty() {
        ui.add_space(24.0);
        crate::widgets::label(ui, tr("No languages found"), t.text_secondary);
    } else {
        egui::ScrollArea::vertical().id_salt("settings_languages").max_height(310.0).show(ui, |ui| {
            for (code, name) in languages {
                let selected = app.session.prefs.language.eq_ignore_ascii_case(code);
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width() - 8.0, 42.0), Sense::click());
                crate::a11y::radio(&response, name, selected);
                if selected || response.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(7), if selected { t.selected_row } else { t.hover });
                }
                let flag = Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(24.0, 24.0));
                paint_language_flag(ui, code, flag, t);
                ui.painter().text(pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, name, regular(12.0), t.text);
                if selected {
                    icons::paint(
                        ui.painter(),
                        Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), vec2(16.0, 16.0)),
                        Icon::Check,
                        t.accent,
                    );
                }
                if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    pick = Some(code);
                }
                ui.add_space(3.0);
            }
        });
    }
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

fn language_search_alias(code: &str) -> &'static str {
    match code {
        "system" => "automatic default",
        "en" => "english united states",
        "es" => "spanish spain",
        "es-419" => "latin american spanish mexico",
        "pt-BR" => "portuguese brazil",
        "zh-Hans" => "simplified chinese china",
        "ko" => "korean south korea",
        "cs" => "czech czechia",
        "bn" => "bengali bangladesh",
        "fi" => "finnish finland",
        "fr" => "french france",
        "nl" => "dutch netherlands",
        "pl" => "polish poland",
        "de" => "german germany",
        "id" => "indonesian indonesia",
        "ja" => "japanese japan",
        "sv" => "swedish sweden",
        "ru" => "russian russia",
        "tr" => "turkish turkey",
        "ka" => "georgian georgia",
        _ => "",
    }
}

fn language_flag_bytes(code: &str) -> Option<&'static [u8]> {
    Some(match code {
        "en" => include_bytes!("../assets/language-flags/us.png"),
        "es" => include_bytes!("../assets/language-flags/es.png"),
        "es-419" => include_bytes!("../assets/language-flags/mx.png"),
        "pt-BR" => include_bytes!("../assets/language-flags/br.png"),
        "zh-Hans" => include_bytes!("../assets/language-flags/cn.png"),
        "ko" => include_bytes!("../assets/language-flags/kr.png"),
        "cs" => include_bytes!("../assets/language-flags/cz.png"),
        "bn" => include_bytes!("../assets/language-flags/bd.png"),
        "fi" => include_bytes!("../assets/language-flags/fi.png"),
        "fr" => include_bytes!("../assets/language-flags/fr.png"),
        "nl" => include_bytes!("../assets/language-flags/nl.png"),
        "pl" => include_bytes!("../assets/language-flags/pl.png"),
        "de" => include_bytes!("../assets/language-flags/de.png"),
        "id" => include_bytes!("../assets/language-flags/id.png"),
        "ja" => include_bytes!("../assets/language-flags/jp.png"),
        "sv" => include_bytes!("../assets/language-flags/se.png"),
        "ru" => include_bytes!("../assets/language-flags/ru.png"),
        "tr" => include_bytes!("../assets/language-flags/tr.png"),
        "ka" => include_bytes!("../assets/language-flags/ge.png"),
        _ => return None,
    })
}

fn paint_language_flag(ui: &egui::Ui, code: &str, rect: Rect, t: &Tokens) {
    if let Some(bytes) = language_flag_bytes(code) {
        let id = Id::new(("language_flag", code));
        let texture = ui.ctx().data(|data| data.get_temp::<egui::TextureHandle>(id)).or_else(|| {
            let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
            let image = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
            let texture = ui.ctx().load_texture(format!("language-flag-{code}"), image, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|data| data.insert_temp(id, texture.clone()));
            Some(texture)
        });
        if let Some(texture) = texture {
            ui.painter().image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            return;
        }
    }
    icons::paint_language_system(ui.painter(), rect, t.icon_secondary);
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
        if crate::widgets::button(ui, tr("Reset"), false, !usual).lizard_tooltip(tr("Clear saved server")).clicked() {
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
        ui.label(RichText::new(tr("Interface size")).font(semibold(13.0)));
        ui.add_space(10.0);
        crate::widgets::label(ui, tr("Use your browser's zoom to change the interface size."), t.text_secondary);
        return;
    }
    let percent = |size: f64| trf("{percent}%", &[("percent", &crate::ui_scale::percent(size as f32))]);
    let shown = percent(chosen);
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        ui.label(RichText::new(tr("Interface size")).font(semibold(13.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
    });
    ui.add_space(8.0);
    ui.vertical(|ui| {
        ui.set_max_width(450.0);
        crate::widgets::label(
            ui,
            tr("Relative to your system's display scaling (100% is the system's own size). Document zoom is separate."),
            t.text_secondary,
        );
    });
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
    let panel_id = Id::new("batch_rename_panel");
    let extra_preview = usize::from(ready && n > PREVIEW_ROWS);
    let preview_height = if rename.is_err() { 40.0 } else { (preview.len() + extra_preview) as f32 * 20.0 };
    let panel_height = 300.0 + preview_height;
    let center = ctx.content_rect().center();
    FloatingPanel::new(panel_id, pos2(center.x - 180.0, center.y - panel_height / 2.0))
        .width(FloatingPanelWidth::Fixed(360.0))
        .height(FloatingPanelHeight::Fixed(panel_height))
        .header_height(44.0)
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, t.border));
            ui.painter().text(header.center(), Align2::CENTER_CENTER, tr("Rename layers"), semibold(13.0), t.text);
            let close = Rect::from_center_size(pos2(header.right() - 24.0, header.center().y), vec2(24.0, 24.0));
            cancel = Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, panel_id.with("close"), close).clicked();

            let body_rect = Rect::from_min_size(pos2(header.left() + 20.0, header.bottom() + 16.0), vec2(320.0, panel_height - 76.0));
            let mut body = ui.new_child(UiBuilder::new().max_rect(body_rect).layout(Layout::top_down(Align::Min)));
            body.spacing_mut().item_spacing.y = 0.0;
            body.label(RichText::new(trn("{n} layers", n, &[])).font(regular(11.0)).color(t.text_secondary));
            body.add_space(12.0);
            crate::widgets::text_input(&mut body, panel_id.with("match"), &mut b.find, 320.0, tr("Match (empty: the whole name)"));
            body.add_space(8.0);
            if crate::widgets::checkbox(&mut body, b.regex, tr("Regular expression")) {
                b.regex = !b.regex;
            }
            body.add_space(8.0);
            let to = crate::widgets::text_input(&mut body, panel_id.with("rename_to"), &mut b.to, 320.0, tr("Rename to"));
            body.add_space(8.0);
            body.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (label, code) in [(tr("Current name"), "$&"), (tr("Number ↑"), "$n"), (tr("Number ↓"), "$N")] {
                    if Button::new(label).variant(ButtonVariant::Ghost).show(ui).clicked() {
                        b.to.push_str(code);
                        to.request_focus();
                    }
                }
            });
            body.add_space(8.0);
            body.horizontal(|ui| {
                ui.label(RichText::new(tr("Start ascending from / stop descending at")).font(regular(11.0)).color(t.text_secondary));
                let number = crate::primitives::NumberField::new(panel_id.with("start"), crate::widgets::Lead::None, Some(f64::from(b.start)), "")
                    .show(ui, ControlWidth::Fixed(64.0));
                let next = match number.edit {
                    Some(crate::widgets::Edit::All(crate::widgets::FieldEdit::Set(value) | crate::widgets::FieldEdit::Scrub(value, _))) => {
                        Some(value)
                    }
                    Some(crate::widgets::Edit::Each(relative)) => relative.apply(f64::from(b.start)),
                    None => None,
                };
                if let Some(value) = next.filter(|value| value.is_finite()) {
                    b.start = value.round().clamp(0.0, 1_000_000.0) as u32;
                }
            });
            body.add_space(8.0);
            match &rename {
                Err(error) => {
                    body.label(RichText::new(error).font(regular(11.0)).color(t.danger));
                }
                Ok(_) => {
                    for (old, new) in &preview {
                        let (rect, _) = body.allocate_exact_size(vec2(320.0, 20.0), Sense::hover());
                        let old = crate::widgets::elide_font(&body, old, 132.0, regular(11.0));
                        let new = crate::widgets::elide_font(&body, new, 154.0, regular(11.0));
                        body.painter().text(rect.left_center(), Align2::LEFT_CENTER, old, regular(11.0), t.text_secondary);
                        body.painter().text(pos2(rect.left() + 140.0, rect.center().y), Align2::LEFT_CENTER, "→", regular(11.0), t.text_secondary);
                        body.painter().text(pos2(rect.left() + 162.0, rect.center().y), Align2::LEFT_CENTER, new, regular(11.0), t.text);
                    }
                    if extra_preview > 0 {
                        body.label(RichText::new(trn("and {n} more", n - PREVIEW_ROWS, &[])).font(regular(11.0)).color(t.text_secondary));
                    }
                }
            }
            body.add_space(12.0);
            body.horizontal(|ui| {
                cancel |= Button::new(tr("Cancel")).variant(ButtonVariant::Secondary).show(ui).clicked();
                let enter = to.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                apply = (Button::new(tr("Rename")).enabled(ready).show(ui).clicked() || enter) && ready;
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
