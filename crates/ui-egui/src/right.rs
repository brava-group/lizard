//! The right panel: Design, Prototype and Inspect tabs.

use crate::primitives::TooltipResponseExt as _;
use egui::{Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use lizard_color::{BlendMode, Paint, PaintKind};
use lizard_doc::*;
use lizard_engine::cmd::lists::{moved_index, removed_index};
use serde_json::{Value, json};

use crate::a11y::{self, Tip};
use crate::i18n::{tr, trf, trn};
use crate::icons::{self, AppearanceIcon, HeaderIcon, Icon, PositionIcon};
use crate::list_drag::{ListDrag, move_flags, remove_flags};
use crate::primitives::{Button, ButtonAlign, ButtonBehavior, ButtonSize, ButtonVariant, ControlWidth, GlassLightControl, NumberField, Slider};
use crate::style_picker::{self, StyleSlot};
use crate::theme::{Tokens, regular, semibold};
use crate::widgets::{self, Edit, FIELD_H, FieldEdit, Lead, PAD};
use crate::{LizardApp, RightTab};

/// Common value of `f` across `ids` (None when mixed or empty).
pub(crate) fn common<T: PartialEq + Clone>(doc: &Document, ids: &[NodeId], f: impl Fn(&Node) -> T) -> Option<T> {
    crate::selection_cache::with_nodes(doc, ids, |nodes| {
        let mut present = nodes.flatten();
        let first = f(present.next()?);
        if present.all(|node| f(node) == first) { Some(first) } else { None }
    })
}

/// A value for the edited points, including the ends of selected segments. The outer `None`
/// means no points are selected; the inner `None` means their radii differ.
pub(crate) fn selected_point_radius(doc: &Document, edit: &lizard_engine::vectoredit::VectorEdit) -> Option<Option<f64>> {
    let mut first = None;
    let mut mixed = false;
    for part in edit.parts() {
        let Some(net) = lizard_engine::vectoredit::network(doc, part.id) else { continue };
        let mut points = part.vertices;
        for segment in part.segments.iter().filter_map(|index| net.segments.get(*index)) {
            points.extend([segment.a, segment.b]);
        }
        points.sort_unstable();
        points.dedup();
        for radius in points.iter().filter_map(|index| net.vertices.get(*index).map(|vertex| vertex.radius)) {
            let radius = (radius * 100.0).round() / 100.0;
            if let Some(value) = first {
                mixed |= value != radius;
            } else {
                first = Some(radius);
            }
        }
    }
    first.map(|radius| (!mixed).then_some(radius))
}

fn layer_corner_radius(node: &Node) -> Option<f64> {
    if let NodeKind::Vector { network } = &node.kind
        && let Some(first) = network.vertices.first()
    {
        let radius = (first.radius * 100.0).round() / 100.0;
        return network.vertices.iter().all(|point| (point.radius * 100.0).round() / 100.0 == radius).then_some(radius);
    }
    node.corners.uniform().map(|radius| (radius * 100.0).round() / 100.0)
}

fn layer_corner_radius_base(node: &Node) -> f64 {
    match &node.kind {
        NodeKind::Vector { network } => network.vertices.iter().map(|point| point.radius).fold(0.0, f64::max),
        _ => node.corners.uniform().unwrap_or_else(|| node.corners.radii.iter().copied().fold(0.0, f64::max)),
    }
}

impl LizardApp {
    /// Set properties on `ids` (one undo step).
    pub fn set_props(&mut self, ids: &[NodeId], props: Value) {
        if ids.is_empty() {
            return;
        }
        self.run("node.set", json!({"ids": ids, "props": props}));
    }

    /// Apply a field edit: scrubs are live previews committed as one step.
    pub fn field(&mut self, ids: &[NodeId], key: &str, edit: Option<FieldEdit>, map: impl Fn(f64) -> Value) {
        if ids.is_empty() {
            return;
        }
        self.field_run(edit, "node.set", |v| json!({"ids": ids, "props": { key: map(v) }}));
    }

    /// [`Self::field`] for a field showing `shown(layer)`: a relative edit of mixed values (`+10`,
    /// `*2`, an arrow key's step) goes to each layer from its own value, in one undo step.
    pub fn field_of(&mut self, ids: &[NodeId], key: &str, edit: Option<Edit>, shown: impl Fn(&Node) -> f64, map: impl Fn(f64) -> Value) {
        let rel = match edit {
            Some(Edit::Each(rel)) => rel,
            Some(Edit::All(e)) => return self.field(ids, key, Some(e), map),
            None => return,
        };
        let Ok(doc) = self.session.doc() else { return };
        let values: Vec<(NodeId, f64)> = ids.iter().filter_map(|i| Some((*i, rel.apply(shown(doc.node(*i)?))?))).collect();
        if values.is_empty() || self.session.begin_interaction("Edit").is_err() {
            return;
        }
        for (id, v) in values {
            if let Err(e) = self.session.execute("node.set", &json!({"ids": [id], "props": { key: map(v) }})) {
                self.failed("node.set", &e);
            }
        }
        let _ = self.session.commit_interaction();
    }

    /// Apply a field edit through any command: `params` builds the command's params from the value.
    pub fn field_run(&mut self, edit: Option<FieldEdit>, cmd: &str, params: impl Fn(f64) -> Value) {
        match edit {
            Some(FieldEdit::Set(v)) => {
                self.run(cmd, params(v));
            }
            Some(FieldEdit::Scrub(v, done)) => {
                let _ = self.session.begin_interaction("Edit");
                let _ = self.session.execute(cmd, &params(v));
                if done {
                    let _ = self.session.commit_interaction();
                }
            }
            None => {}
        }
    }
}

pub fn right_panel(app: &mut LizardApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0;
    let w = ui.available_width();
    let header_bg = t.panel;
    // Top bar: people (you and the others in a live session), present, share.
    let (top, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
    ui.painter().rect_filled(top, 0.0, header_bg);
    // As wide as its label needs (longer in some languages).
    let label = ui.painter().layout_no_wrap(tr("Share").to_string(), regular(12.0), Color32::WHITE);
    let sw = (label.size().x + 24.0).max(56.0);
    let share = Rect::from_min_size(pos2(top.max.x - 9.0 - sw, top.center().y - 16.0), vec2(sw, 32.0));
    let sresp = Button::new(tr("Share")).size(ButtonSize::Large).show_at(ui, ui.id().with("share"), share).tip(tr("Share: live session, export"));
    crate::live::share_popover(app, &sresp);
    // Play presents; its chevron opens the play menu (present, or preview in a pane).
    let play = Rect::from_min_size(pos2(share.min.x - 52.0, top.center().y - 16.0), vec2(48.0, 32.0));
    let (tri_r, chev_r) =
        (Rect::from_min_max(play.min, pos2(play.max.x - 16.0, play.max.y)), Rect::from_min_max(pos2(play.max.x - 16.0, play.min.y), play.max));
    let present_keys = crate::toolbar::pretty_shortcut(crate::shortcuts::shortcut("proto.present").unwrap_or(""));
    let presp = Button::header_icon(HeaderIcon::Present, tr("Present"))
        .size(ButtonSize::Large)
        .show_at(ui, ui.id().with("play"), tri_r)
        .tip(format!("{}  {present_keys}", tr("Present")));
    let mresp = Button::icon(Icon::Chevron, tr("Present or preview"))
        .size(ButtonSize::Large)
        .icon_size(10.0)
        .behavior(ButtonBehavior::Select { expanded: false })
        .show_at(ui, ui.id().with("play_menu"), chev_r);
    crate::live::people_bar(app, ui, Rect::from_min_max(pos2(top.min.x + 8.0, top.min.y), pos2(play.min.x - 6.0, top.max.y)));
    if presp.clicked() {
        app.run("proto.present", json!({}));
    }
    menu_popup(&mresp).width(200.0).show(|ui| {
        if menu_item(ui, tr("Present"), &present_keys) {
            app.run("proto.present", json!({}));
            ui.close();
        }
        let (label, cmd) = if app.ui.preview.is_some() { (tr("Close preview"), "proto.closePreview") } else { (tr("Preview"), "proto.preview") };
        if menu_item(ui, label, "") {
            app.run(cmd, json!({}));
            ui.close();
        }
    });
    // Tabs row.
    let (tabs, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    ui.painter().rect_filled(tabs, 0.0, header_bg);
    let inspecting = app.ui.right_tab == RightTab::Inspect;
    let tab_items = if inspecting {
        [crate::primitives::Tab::new(tr("Inspect")), crate::primitives::Tab::new(tr("Plugins"))]
    } else {
        [crate::primitives::Tab::new(tr("Create")), crate::primitives::Tab::new(tr("Prototype"))]
    };
    let selected = match app.ui.right_tab {
        RightTab::Design => Some(0),
        RightTab::Prototype => Some(1),
        RightTab::Inspect => Some(usize::from(app.ui.dev.plugins_tab)),
    };
    let tab_area = Rect::from_min_size(pos2(tabs.min.x + 6.0, tabs.min.y), vec2((tabs.width() - 90.0).max(1.0), 32.0));
    let mut tab_ui = ui.new_child(egui::UiBuilder::new().max_rect(tab_area));
    if let Some(index) = crate::primitives::Tabs::new(ui.id().with("right_panel_tabs"), &tab_items, selected)
        .width(crate::primitives::ControlWidth::Fill)
        .allow_empty(true)
        .show(&mut tab_ui)
    {
        crate::proto_panel::open_editor(ui.ctx(), None);
        if inspecting {
            app.ui.dev.plugins_tab = index == 1;
        } else {
            app.ui.right_tab = if index == 0 { RightTab::Design } else { RightTab::Prototype };
        }
    }
    // Zoom menu.
    let zoom = app.session.ds().map(|d| d.view.zoom).unwrap_or(1.0);
    let zoom_label = format!("{}%", (zoom * 100.0).round());
    let zoom_galley = ui.painter().layout_no_wrap(zoom_label.clone(), regular(12.0), t.text);
    let zoom_width = (zoom_galley.size().x + 40.0).max(72.0);
    let zr = Rect::from_min_size(pos2(tabs.max.x - 9.0 - zoom_width, tabs.min.y + 3.0), vec2(zoom_width, 24.0));
    let zresp = Button::new(&zoom_label)
        .variant(ButtonVariant::Ghost)
        .behavior(ButtonBehavior::Select { expanded: false })
        .align(ButtonAlign::Start)
        .show_at(ui, ui.id().with("zoommenu"), zr);
    crate::a11y::combo(&zresp, tr("Zoom"), &zoom_label, crate::a11y::menu_open(&zresp));
    menu_popup(&zresp).width(200.0).show(|ui| {
        for (label, cmd) in [
            (tr("Zoom in"), "view.zoomIn"),
            (tr("Zoom out"), "view.zoomOut"),
            (tr("Zoom to fit"), "view.zoomFit"),
            (tr("Zoom to 50%"), "view.zoom50"),
            (tr("Zoom to 100%"), "view.zoom100"),
            (tr("Zoom to 200%"), "view.zoom200"),
            (tr("Zoom to selection"), "view.zoomSelection"),
        ] {
            if menu_item(ui, label, crate::shortcuts::shortcut(cmd).unwrap_or("")) {
                app.run(cmd, json!({}));
                ui.close();
            }
        }
        crate::primitives::Menu::separator(ui, None);
        let Ok(v) = app.session.ds().map(|d| d.view.clone()) else { return };
        for (label, scale) in [(tr("Pixel preview 1x"), 1u8), (tr("Pixel preview 2x"), 2)] {
            let on = v.pixel_preview && v.pixel_scale == scale;
            if menu_check(ui, on, label, "") {
                app.run("view.pixelPreview", json!({"on": !on, "scale": scale}));
                ui.close();
            }
        }
        let toggles: [(&str, &str, bool); 5] = [
            (tr("Pixel grid"), "view.pixelGrid", v.pixel_grid),
            (tr("Snap to pixel grid"), "view.snapPixel", v.snap_pixel),
            (tr("Layout grids"), "view.layoutGrids", v.layout_grids),
            (tr("Rulers"), "view.rulers", v.rulers),
            (tr("Outlines"), "view.outlines", v.outlines),
        ];
        for (label, cmd, on) in toggles {
            if menu_check(ui, on, label, crate::shortcuts::shortcut(cmd).unwrap_or("")) {
                app.run(cmd, json!({}));
                ui.close();
            }
        }
    });
    ui.painter().line_segment([tabs.left_bottom(), tabs.right_bottom()], Stroke::new(1.0, t.border));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        if app.session.tool.tool == lizard_engine::ToolId::Comment {
            crate::comments::panel(app, ui);
            ui.add_space(80.0);
            return;
        }
        match app.ui.right_tab {
            // The Frame tool shows the frame presets.
            RightTab::Design if app.session.tool.tool == lizard_engine::ToolId::Frame => crate::frame_presets::panel(app, ui),
            RightTab::Design => design(app, ui),
            RightTab::Prototype => crate::proto_panel::prototype(app, ui),
            RightTab::Inspect => inspect(app, ui),
        }
        ui.add_space(80.0);
        a11y::scroll_focus(&mut app.ui.nav, ui);
    });
}

/// Render one production Design-panel section for the standalone component catalog.
/// The feature is absent from the Lizard app build.
#[cfg(feature = "catalog")]
#[derive(Clone, Copy)]
pub enum CatalogSection {
    Header,
    Position,
    Layout,
    Appearance,
    Typography,
    Fills,
    Strokes,
    Effects,
    SelectionColors,
    Grids,
    Export,
    Component,
    VariableModes,
    Scale,
}

#[cfg(feature = "catalog")]
pub fn catalog_section(app: &mut LizardApp, ui: &mut Ui, section: CatalogSection) {
    let Ok(ds) = app.session.ds() else { return };
    let doc = ds.doc.clone();
    let sel = ds.selection.clone();
    if sel.is_empty() {
        return;
    }
    match section {
        CatalogSection::Header => header(app, ui, &doc, &sel),
        CatalogSection::Position => position(app, ui, &doc, &sel),
        CatalogSection::Layout => crate::layout_panel::layout_section(app, ui, &doc, &sel),
        CatalogSection::Appearance => appearance(app, ui, &doc, &sel),
        CatalogSection::Typography => crate::typography::typography(app, ui, &doc, &sel),
        CatalogSection::Fills => paints(app, ui, &doc, &sel, true),
        CatalogSection::Strokes => paints(app, ui, &doc, &sel, false),
        CatalogSection::Effects => effects(app, ui, &doc, &sel),
        CatalogSection::SelectionColors => crate::selection_colors::section(app, ui, &doc, &sel),
        CatalogSection::Grids => grids(app, ui, &doc, &sel),
        CatalogSection::Export => crate::export_panel::section(app, ui, &doc, &sel),
        CatalogSection::Component => crate::component_panel::instance_section(app, ui, &doc, &sel),
        CatalogSection::VariableModes => {
            variable_modes(app, ui, &doc, &sel);
        }
        CatalogSection::Scale => crate::scale_panel::section(app, ui, &doc, &sel),
    }
}

/// The menu a dropdown or button opens: as wide as its widest row and at least as wide as
/// `anchor`. egui measures a popup in a first, invisible pass as wide as the width it is given
/// (else its 600 px default area width) and keeps the size measured; rows ([`menu_row`]) take the
/// larger of that width and their content, so the menu fits its content. Chain `.width(w)` for a
/// menu at least `w` wide.
/// The blend modes as menu items in groups (pass-through only for layers); the one picked.
pub fn blend_menu(ui: &mut Ui, with_pass_through: bool) -> Option<BlendMode> {
    let mut picked = None;
    for b in BlendMode::ALL.into_iter().filter(|b| with_pass_through || *b != BlendMode::PassThrough) {
        if menu_item(ui, tr(b.label()), "") {
            picked = Some(b);
            ui.close();
        }
        if matches!(b, BlendMode::Normal | BlendMode::ColorBurn | BlendMode::ColorDodge | BlendMode::HardLight | BlendMode::Exclusion) {
            crate::primitives::Menu::separator(ui, None);
        }
    }
    picked
}

pub fn menu_popup(anchor: &egui::Response) -> crate::primitives::MenuPopup {
    crate::primitives::Menu::popup(anchor)
}

/// A context menu (secondary click on `resp`) sized like [`menu_popup`].
pub fn context_popup(resp: &egui::Response) -> crate::primitives::MenuPopup {
    crate::primitives::Menu::context(resp)
}

pub fn menu_item(ui: &mut Ui, label: &str, shortcut: &str) -> bool {
    crate::primitives::MenuItem::new(label).shortcut(shortcut).show(ui).clicked()
}

/// A menu row with art drawn into it after the label (`art(painter, row rect, text colour)`),
/// taking `art_w` from the row's right edge.
pub fn menu_item_with(ui: &mut Ui, label: &str, shortcut: &str, art_w: f32, art: impl FnOnce(&egui::Painter, Rect, Color32)) -> bool {
    crate::primitives::MenuItem::new(label).shortcut(shortcut).show_with(ui, art_w, art).clicked()
}

/// A menu row (see [`menu_item_with`]); its response tells hovering too. It spans the menu and
/// is at least as wide as its label, shortcut and art.
pub fn menu_row(ui: &mut Ui, label: &str, shortcut: &str, art_w: f32, art: impl FnOnce(&egui::Painter, Rect, Color32)) -> egui::Response {
    crate::primitives::MenuItem::new(label).shortcut(shortcut).show_with(ui, art_w, art)
}

/// A menu item with a check mark when `on` (the unchecked ones line up with it).
pub fn menu_check(ui: &mut Ui, on: bool, label: &str, shortcut: &str) -> bool {
    marked_row(ui, Some(on), label, shortcut)
}

/// A plain menu item lined up with the check items above or below it.
pub fn menu_item_beside_checks(ui: &mut Ui, label: &str, shortcut: &str) -> bool {
    marked_row(ui, None, label, shortcut)
}

fn marked_row(ui: &mut Ui, on: Option<bool>, label: &str, shortcut: &str) -> bool {
    let item = crate::primitives::MenuItem::new(label).shortcut(shortcut);
    let item = match on {
        Some(on) => item.checked(on),
        None => item.check_column(),
    };
    item.show(ui).clicked()
}

pub(crate) fn row(ui: &mut Ui, h: f32, add: impl FnOnce(&mut Ui)) {
    row_rect(ui, h, add);
}

/// [`row`], returning the area the row takes.
pub(crate) fn row_rect(ui: &mut Ui, h: f32, add: impl FnOnce(&mut Ui)) -> Rect {
    block(ui, h, (h - FIELD_H) / 2.0, add)
}

/// A panel row `h` tall whose content is inset `inset_y` from its top and bottom (for content
/// taller than one field); returns the area the row takes.
pub(crate) fn block(ui: &mut Ui, h: f32, inset_y: f32, add: impl FnOnce(&mut Ui)) -> Rect {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut content = r.shrink2(vec2(PAD, inset_y));
    content.max.x = (content.max.x - widgets::PROPERTY_ACTION_SLOT).max(content.min.x);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.spacing_mut().item_spacing = vec2(8.0, 0.0);
    add(&mut child);
    r
}

fn design(app: &mut LizardApp, ui: &mut Ui) {
    let Ok(ds) = app.session.ds() else { return };
    // The panel shows the document itself, not a live text preview over it.
    let doc = ds.text_preview.clone().unwrap_or_else(|| ds.doc.clone());
    let sel = ds.selection.clone();
    let vector_edit = ds.vector_edit.clone();
    if sel.is_empty() {
        page_section(app, ui, &doc);
        return;
    }
    crate::selection_cache::show(&doc, &sel);
    header(app, ui, &doc, &sel);
    let points_selected = vector_edit.as_ref().is_some_and(|ve| !ve.vertices.is_empty() || !ve.segments.is_empty());
    if !points_selected {
        widgets::separator(ui);
    }
    if crate::widget_ui::design_section(app, ui, &doc, &sel) {
        widgets::separator(ui);
    }
    // Editing a vector with points selected, the points' fields stand in for the layer's position
    // and dimensions.
    match vector_edit {
        Some(ve) => crate::vector_panel::points(app, ui, &doc, &ve),
        None => {
            crate::section_panel::section(app, ui, &doc, &sel);
            if app.session.tool.tool == lizard_engine::ToolId::Scale {
                crate::scale_panel::section(app, ui, &doc, &sel);
                widgets::separator(ui);
            }
            position(app, ui, &doc, &sel);
        }
    }
    widgets::separator(ui);
    if !points_selected {
        crate::layout_panel::layout_section(app, ui, &doc, &sel);
        widgets::separator(ui);
    }
    if !points_selected {
        appearance(app, ui, &doc, &sel);
        widgets::separator(ui);
    }
    if variable_modes(app, ui, &doc, &sel) {
        widgets::separator(ui);
    }
    if sel.iter().any(|i| matches!(doc.node(*i).map(|n| &n.kind), Some(NodeKind::Text(_)))) {
        crate::typography::typography(app, ui, &doc, &sel);
        widgets::separator(ui);
    }
    paints(app, ui, &doc, &sel, true);
    widgets::separator(ui);
    paints(app, ui, &doc, &sel, false);
    widgets::separator(ui);
    effects(app, ui, &doc, &sel);
    widgets::separator(ui);
    crate::selection_colors::section(app, ui, &doc, &sel);
    if sel.iter().all(|i| doc.node(*i).is_some_and(|n| n.kind.is_frame_like())) {
        grids(app, ui, &doc, &sel);
        widgets::separator(ui);
    }
    crate::export_panel::section(app, ui, &doc, &sel);
}

fn page_section(app: &mut LizardApp, ui: &mut Ui, doc: &Document) {
    let Ok(page) = app.session.page() else { return };
    let (bg, background_visible) = match doc.node(page).map(|n| &n.kind) {
        Some(NodeKind::Page { background, background_visible, .. }) => (*background, *background_visible),
        _ => (lizard_color::Color::DEFAULT_PAGE, true),
    };
    widgets::section_header(ui, tr("Page"), &[], false);
    let mut color_change = crate::primitives::ColorControlResponse::default();
    let mut toggle_background = false;
    row(ui, 32.0, |ui| {
        let p = Paint::solid(bg);
        let control_width = ui.available_width().max(96.0);
        color_change = crate::primitives::ColorControl::new(
            Id::new("pagebg"),
            &p,
            crate::primitives::ColorValue::EditableHex(&bg.to_hex()),
            f64::from(bg.a) * 100.0,
        )
        .show_sized(ui, crate::primitives::ControlWidth::Fixed(control_width));
        toggle_background = Button::icon(
            if background_visible { Icon::Eye } else { Icon::EyeOff },
            if background_visible { tr("Hide page background") } else { tr("Show page background") },
        )
        .icon_size(14.0)
        .show(ui)
        .clicked();
    });
    if toggle_background {
        app.run("page.setBackgroundVisible", json!({"visible": !background_visible}));
    }
    if color_change.open_picker {
        app.ui.picker = Some(crate::colorpicker::PickerTarget::PageBackground);
    }
    if let Some(h) = color_change.edited_hex
        && let Some(c) = lizard_color::Color::from_hex_or_name(&h)
    {
        app.run("page.setBackground", json!({"color": c.to_css_hex()}));
    }
    app.field_run(
        color_change.opacity_edit,
        "page.setBackground",
        |value| json!({"color": bg.with_alpha((value / 100.0).clamp(0.0, 1.0) as f32).to_css_hex()}),
    );
    ui.add_space(12.0);
    widgets::separator(ui);
    widgets::section_header(ui, tr("Color profile"), &[], false);
    row(ui, 32.0, |ui| {
        let profiles = lizard_color::ColorProfile::ALL;
        let labels = profiles.map(|profile| profile.label());
        let selected = profiles.iter().position(|profile| *profile == doc.color_profile).unwrap_or(0);
        if let Some(index) =
            crate::primitives::Select::new(&labels, selected).label(tr("Color profile")).width(crate::primitives::ControlWidth::Fill).show(ui)
            && let Some(profile) = profiles.get(index).copied()
            && profile != doc.color_profile
        {
            app.ui.color_profile = Some(profile);
        }
    });
    ui.add_space(12.0);
    widgets::separator(ui);
    // Local variables & styles.
    let vars = doc.variables.variables.iter().filter(|v| !lizard_engine::library::is_remote_variable(doc, &v.id)).count();
    widgets::section_header(ui, tr("Local variables"), &[], vars == 0);
    row(ui, 32.0, |ui| {
        let label = trf("Open variables ({count})", &[("count", &vars)]);
        let r = Button::new(&label)
            .variant(ButtonVariant::Input)
            .align(ButtonAlign::Start)
            .width(crate::primitives::ControlWidth::Fill)
            .leading(Icon::Styles)
            .trailing(Icon::Chevron)
            .trailing_at_end(true)
            .show(ui);
        if r.clicked() {
            app.ui.preferences = false;
            app.ui.palette = None;
            app.ui.variables.get_or_insert_with(Default::default);
        }
    });
    ui.add_space(12.0);
    widgets::separator(ui);
    crate::style_picker::local_styles(app, ui, doc);
    ui.add_space(20.0);
    widgets::separator(ui);
    crate::export_panel::page_section(app, ui, doc, page);
}

fn header(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    const ACTION_ICON_SIZE: f32 = 14.0;
    const ACTION_CELL_WIDTH: f32 = 28.0;
    let t = Tokens::get(ui.ctx());
    if let [id] = sel
        && let Some(node) = doc.node(*id)
        && matches!(node.kind, NodeKind::Component { .. } | NodeKind::ComponentSet { .. })
    {
        component_header(app, ui, doc, *id, &node.name);
        crate::component_panel::instance_section(app, ui, doc, sel);
        return;
    }
    let label = match common(doc, sel, |n| n.kind.label()) {
        Some(l) if sel.len() == 1 => tr(l).to_string(),
        // The plural ("Frames") is a key of its own.
        Some(l) => tr(&format!("{l}s")).to_string(),
        None => tr("Mixed").into(),
    };
    let is_frame = sel.len() == 1 && sel.first().and_then(|i| doc.node(*i)).is_some_and(|n| matches!(n.kind, NodeKind::Frame));
    // Frames and groups convert into each other from the type menu.
    let is_group = !sel.is_empty() && sel.iter().all(|i| doc.node(*i).is_some_and(|n| matches!(n.kind, NodeKind::Group)));
    let kind_items = |app: &mut LizardApp, ui: &mut Ui, frame: bool| {
        for (to_frame, label, cmd) in [(true, tr("Frame"), "object.convertToFrame"), (false, tr("Group"), "object.convertToGroup")] {
            if menu_check(ui, to_frame == frame, label, "") {
                if to_frame != frame {
                    app.run(cmd, json!({"ids": sel}));
                }
                ui.close();
            }
        }
    };
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    let lr = Rect::from_min_size(pos2(r.min.x + 8.0, r.center().y - 16.0), vec2(86.0, 32.0));
    let resp = ui.interact(lr, ui.id().with("typedd"), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(lr, CornerRadius::same(5), t.hover);
    }
    let label_font = regular(12.0);
    let label_x = lr.min.x + 8.0;
    let label_width = ui.painter().layout_no_wrap(label.clone(), label_font.clone(), t.text).size().x;
    ui.painter().text(pos2(label_x, lr.center().y), Align2::LEFT_CENTER, &label, label_font, t.text);
    if is_frame || is_group {
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(label_x + label_width + 13.0, lr.center().y), vec2(10.0, 10.0)),
            Icon::Chevron,
            t.icon_secondary,
        );
    }
    if is_group {
        menu_popup(&resp).width(180.0).show(|ui| kind_items(app, ui, false));
    }
    if is_frame {
        // Align this action with the four action cells and the chevron on the right.
        let sr = Rect::from_min_size(pos2(r.max.x - 8.0 - 16.0 - 5.0 * ACTION_CELL_WIDTH, r.center().y - 16.0), vec2(ACTION_CELL_WIDTH, 32.0));
        let sresp = ui.interact(sr, ui.id().with("swap_orient"), Sense::click()).tip(tr("Swap orientation"));
        if sresp.hovered() {
            ui.painter().rect_filled(sr, CornerRadius::same(5), t.hover);
        }
        icons::paint(ui.painter(), Rect::from_center_size(sr.center(), vec2(ACTION_ICON_SIZE, ACTION_ICON_SIZE)), Icon::Rotate, t.icon);
        if sresp.clicked() {
            app.run("frame.swapOrientation", json!({"ids": sel}));
        }
        let ids: Vec<NodeId> = sel.to_vec();
        menu_popup(&resp).width(260.0).show(|ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                kind_items(app, ui, true);
                let mut cat = "";
                for (c, name, pw, ph) in lizard_engine::cmd::create::FRAME_PRESETS {
                    if *c != cat {
                        cat = c;
                        crate::primitives::Menu::separator(ui, Some(tr(c)));
                    }
                    if menu_item(ui, name, &format!("{}×{}", pw, ph)) {
                        app.set_props(&ids, json!({"width": pw, "height": ph}));
                        ui.close();
                    }
                }
            });
        });
    }
    if app.session.ds().ok().and_then(|ds| ds.vector_edit.as_ref()).is_some_and(|edit| !edit.vertices.is_empty() || !edit.segments.is_empty()) {
        return;
    }
    let is_text = !sel.is_empty() && sel.iter().all(|id| doc.node(*id).is_some_and(|n| matches!(n.kind, NodeKind::Text(_))));
    if is_text {
        let actions = [
            (HeaderIcon::SelectMatching, tr("Select matching layers"), "select.matching"),
            (HeaderIcon::TextLink, tr("Create link"), "text.link"),
            (HeaderIcon::CreateComponent, tr("Create component"), "component.create"),
            (HeaderIcon::More, tr("More actions"), "text.more"),
        ];
        for (index, (icon, label, command)) in actions.into_iter().enumerate() {
            let button = Rect::from_min_size(pos2(r.max.x - 8.0 - (4 - index) as f32 * 24.0, r.center().y - 12.0), vec2(24.0, 24.0));
            let size = if icon == HeaderIcon::More { 16.0 } else { 18.0 };
            let response = Button::header_icon(icon, label).icon_size(size).show_at(ui, ui.id().with(("text_header", index)), button);
            if response.clicked() {
                match command {
                    "text.link" => crate::menus::invoke(app, command),
                    "text.more" => app.ui.context_menu = Some((button.left_bottom(), sel.to_vec())),
                    _ => {
                        app.run(command, json!({}));
                    }
                }
            }
        }
        if let [id] = sel
            && let Some(node) = doc.node(*id)
        {
            crate::component_panel::text_value_ui(app, ui, doc, node);
        }
        crate::component_panel::instance_section(app, ui, doc, sel);
        return;
    }
    // Compact horizontal cells keep the artwork centred in 32-point-tall hit targets.
    let mut x = r.max.x - 8.0;
    let action_rect = |x: f32, width: f32| Rect::from_min_size(pos2(x - width, r.center().y - 16.0), vec2(width, 32.0));
    let edit_id = (sel.len() == 1).then(|| sel[0]).filter(|id| doc.node(*id).is_some_and(|n| n.kind.is_shape()));
    if let Some(id) = edit_id {
        let br = action_rect(x, ACTION_CELL_WIDTH);
        let resp = ui.interact(br, ui.id().with("edit_object"), Sense::click()).tip(tr("Edit object"));
        if resp.hovered() {
            ui.painter().rect_filled(br, CornerRadius::same(5), t.hover);
        }
        icons::paint_header(
            ui.painter(),
            Rect::from_center_size(br.center(), vec2(ACTION_ICON_SIZE, ACTION_ICON_SIZE)),
            HeaderIcon::EditObject,
            t.icon,
        );
        if resp.clicked() {
            app.run("vector.edit.begin", json!({"id": id}));
        }
        x -= ACTION_CELL_WIDTH;
    }
    // Layers in boolean groups, or one boolean group: the operation they use (checked in the menu).
    let current = lizard_engine::booleans::shown_op(doc, sel);
    let bool_menu_rect = action_rect(x, 16.0);
    let bool_menu = ui.interact(bool_menu_rect, ui.id().with("bool_menu"), Sense::click()).tip(tr("Boolean groups"));
    if bool_menu.hovered() {
        ui.painter().rect_filled(bool_menu_rect, CornerRadius::same(5), t.hover);
    }
    icons::paint_compact_chevron(
        ui.painter(),
        Rect::from_center_size(bool_menu_rect.center(), vec2(crate::primitives::CONTROL_CHEVRON_SIZE, crate::primitives::CONTROL_CHEVRON_SIZE)),
        t.icon,
    );
    menu_popup(&bool_menu).width(180.0).show(|ui| {
        for (op, c) in [
            (BoolOp::Union, "bool.union"),
            (BoolOp::Subtract, "bool.subtract"),
            (BoolOp::Intersect, "bool.intersect"),
            (BoolOp::Exclude, "bool.exclude"),
        ] {
            let key = crate::shortcuts::shortcut(c).unwrap_or("");
            let picked = match current {
                Some(cur) => menu_check(ui, cur == op, tr(op.label()), key),
                None => menu_item(ui, tr(op.label()), key),
            };
            if picked {
                app.run(c, json!({}));
                ui.close();
            }
        }
        let key = crate::shortcuts::shortcut("vector.flatten").unwrap_or("");
        let flatten = if current.is_some() { menu_item_beside_checks(ui, tr("Flatten"), key) } else { menu_item(ui, tr("Flatten"), key) };
        if flatten {
            app.run("vector.flatten", json!({}));
            ui.close();
        }
    });
    x -= 16.0;
    let bool_rect = action_rect(x, ACTION_CELL_WIDTH);
    let bool_button = ui.interact(bool_rect, ui.id().with("bool_union"), Sense::click()).tip(tr("Union"));
    if bool_button.hovered() {
        ui.painter().rect_filled(bool_rect, CornerRadius::same(5), t.hover);
    }
    let icon_rect = Rect::from_center_size(bool_rect.center(), vec2(ACTION_ICON_SIZE, ACTION_ICON_SIZE));
    if let Some(op) = current {
        icons::paint(ui.painter(), icon_rect, crate::layers::op_icon(op), t.icon);
    } else {
        icons::paint_header(ui.painter(), icon_rect, HeaderIcon::UnionTertiary, t.text_tertiary);
        icons::paint_header(ui.painter(), icon_rect, HeaderIcon::Union, t.icon);
    }
    if bool_button.clicked() && sel.len() >= 2 {
        app.run("bool.union", json!({}));
    }
    x -= ACTION_CELL_WIDTH;
    for (icon, label, cmd) in [
        (HeaderIcon::Mask, tr("Use as mask"), "object.mask"),
        (HeaderIcon::CreateComponent, tr("Create component"), "component.create"),
        (HeaderIcon::SelectMatching, tr("Select matching layers"), "select.matching"),
    ] {
        let br = action_rect(x, ACTION_CELL_WIDTH);
        let resp = ui.interact(br, ui.id().with(("qa", cmd)), Sense::click()).tip(crate::shortcuts::tip(label, cmd));
        if resp.hovered() {
            ui.painter().rect_filled(br, CornerRadius::same(5), t.hover);
        }
        let icon_rect = Rect::from_center_size(br.center(), vec2(ACTION_ICON_SIZE, ACTION_ICON_SIZE));
        icons::paint_header(ui.painter(), icon_rect, icon, t.icon);
        if icon == HeaderIcon::Mask {
            icons::paint_header(ui.painter(), icon_rect, HeaderIcon::MaskTertiary, t.text_tertiary);
        }
        if resp.clicked() {
            app.run(cmd, json!({}));
        }
        x -= ACTION_CELL_WIDTH;
    }
    crate::component_panel::instance_section(app, ui, doc, sel);
}

fn component_header(app: &mut LizardApp, ui: &mut Ui, doc: &Document, id: NodeId, name: &str) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let label = widgets::elide_font(ui, name, (rect.width() - 120.0).max(40.0), regular(12.0));
    ui.painter().text(pos2(rect.min.x + 16.0, rect.center().y), Align2::LEFT_CENTER, label, regular(12.0), t.text);

    let action = |ui: &mut Ui, index: usize, icon: Icon, size: f32, tip: &str, enabled: bool| {
        let cell = Rect::from_center_size(pos2(rect.max.x - 20.0 - index as f32 * 28.0, rect.center().y), vec2(24.0, 24.0));
        Button::icon(icon, tip).icon_size(size).enabled(enabled).show_at(ui, ui.id().with(("component_header", id, index)), cell)
    };
    let owner = lizard_engine::components::variant_set(doc, id).unwrap_or(id);
    let more = action(ui, 0, Icon::Dots, 14.0, tr("More actions"), true);
    let settings = action(ui, 1, Icon::TypeSettings, 20.0, tr("Component properties"), true);
    let create = action(ui, 2, Icon::AddVariant, 16.0, tr("Add variant"), lizard_engine::cmd::component::can_add_variant(doc, id));
    if create.clicked() {
        app.run("component.addVariant", json!({"id": id}));
    }
    menu_popup(&settings).width(220.0).show(|ui| crate::component_panel::add_property_menu(app, ui, doc, owner));
    menu_popup(&more).width(180.0).show(|ui| {
        if lizard_engine::cmd::component::can_add_variant(doc, owner) && menu_item(ui, tr("Add variant"), "") {
            app.run("component.addVariant", json!({"id": owner}));
            ui.close();
        }
    });
}

fn position(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    widgets::section_header(ui, tr("Alignment"), &[], false);
    // Layers inside an instance take their place and constraints from the main component.
    let nested = sel.iter().any(|i| lizard_engine::components::inside_instance(doc, *i));
    ui.add_enabled_ui(!nested, |ui| position_fields(app, ui, doc, sel))
        .response
        .lizard_disabled_tooltip(tr("Layers in an instance take their position from the main component"));
    ui.add_space(12.0);
}

fn position_segments(ui: &mut Ui, items: &[(PositionIcon, &str)], width: f32) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(5), if t.dark { Color32::from_rgb(0x31, 0x31, 0x31) } else { t.field });
    let mut picked = None;
    for (index, (icon, label)) in items.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(rect.min.x + width * index as f32 / 3.0, rect.min.y), vec2(width / 3.0, 24.0));
        let resp = ui.interact(cell, ui.id().with(("position_segment", *label)), Sense::click()).tip(*label);
        a11y::toggle(&resp, label, false);
        if resp.hovered() && ui.is_enabled() {
            ui.painter().rect_filled(cell, CornerRadius::same(5), t.hover);
        }
        icons::paint_position(
            ui.painter(),
            Rect::from_center_size(cell.center(), vec2(16.0, 16.0)),
            *icon,
            if ui.is_enabled() { t.icon } else { t.icon_secondary },
            t.text_tertiary,
        );
        if resp.clicked() && ui.is_enabled() {
            picked = Some(index);
        }
    }
    picked
}

fn position_fields(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let ids = sel.to_vec();
    let col = widgets::col_width(ui.available_width());
    block(ui, 24.0, 0.0, |ui| {
        let cmds = ["align.left", "align.hcenter", "align.right", "align.top", "align.vcenter", "align.bottom"];
        let tip = crate::shortcuts::tip;
        let items = [
            (PositionIcon::AlignLeft, tip(tr("Align left"), cmds[0])),
            (PositionIcon::AlignHCenter, tip(tr("Align horizontal centers"), cmds[1])),
            (PositionIcon::AlignRight, tip(tr("Align right"), cmds[2])),
            (PositionIcon::AlignTop, tip(tr("Align top"), cmds[3])),
            (PositionIcon::AlignVCenter, tip(tr("Align vertical centers"), cmds[4])),
            (PositionIcon::AlignBottom, tip(tr("Align bottom"), cmds[5])),
        ];
        let picked = ui.add_enabled_ui(lizard_engine::cmd::arrange::can_align(doc, sel), |ui| {
            let h = position_segments(ui, &items[..3], col);
            let v = position_segments(ui, &items[3..], col).map(|i| i + 3);
            h.or(v)
        });
        picked.response.lizard_disabled_tooltip(tr("A layer on its own aligns to its frame: select more layers, or one in a frame"));
        if let Some(i) = picked.inner {
            app.run(cmds[i], json!({"ids": ids}));
        }
    });
    ui.add_space(8.0);
    // In the frame's space: groups have none of their own.
    // What each field shows of a layer (a relative edit of mixed values starts from it).
    let x_of = |n: &Node| (doc.frame_relative(n.id).as_coeffs()[4] * 100.0).round() / 100.0;
    let y_of = |n: &Node| (doc.frame_relative(n.id).as_coeffs()[5] * 100.0).round() / 100.0;
    let rot_of = |n: &Node| (lizard_geom::rotation_degrees(n.transform) * 100.0).round() / 100.0;
    let (x, y, rot) = (common(doc, sel, x_of), common(doc, sel, y_of), common(doc, sel, rot_of));
    block(ui, 24.0, 0.0, |ui| {
        let e = widgets::number_field_mixed(ui, Id::new("pos_x"), Lead::Text("X"), x, "", col, 1.0);
        app.field_of(&ids, "x", e, x_of, |v| json!(v));
        let e = widgets::number_field_mixed(ui, Id::new("pos_y"), Lead::Text("Y"), y, "", col, 1.0);
        app.field_of(&ids, "y", e, y_of, |v| json!(v));
    });
    ui.add_space(8.0);
    // Constraints (children of frames without auto layout, or ignoring it).
    let in_frame = sel.iter().all(|i| {
        doc.node(*i).is_some_and(|n| {
            n.parent.and_then(|p| doc.node(p)).is_some_and(|p| p.kind.is_frame_like() && (p.layout.is_none() || n.child_layout.absolute))
        })
    });
    if in_frame {
        let c = common(doc, sel, |n| n.constraints);
        block(ui, 56.0, 0.0, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let h = c.map(|c| format!("{:?}", c.horizontal)).unwrap_or_else(|| "Mixed".into());
                let resp = crate::primitives::Select::trigger_with_variant(
                    ui,
                    tr(&pretty(&h)),
                    col,
                    Some(crate::primitives::SelectLead::Icon(Icon::MoveHorizontal)),
                    crate::primitives::SelectVariant::Outlined,
                );
                menu_popup(&resp).show(|ui| {
                    for (l, v) in [
                        (tr("Left"), "left"),
                        (tr("Right"), "right"),
                        (tr("Left & right"), "leftRight"),
                        (tr("Center"), "center"),
                        (tr("Scale"), "scale"),
                    ] {
                        if menu_item(ui, l, "") {
                            app.set_props(&ids, json!({"constraints": {"horizontal": v}}));
                            ui.close();
                        }
                    }
                });
                ui.add_space(8.0);
                let v = c.map(|c| format!("{:?}", c.vertical)).unwrap_or_else(|| "Mixed".into());
                let resp = crate::primitives::Select::trigger_with_variant(
                    ui,
                    tr(&pretty(&v)),
                    col,
                    Some(crate::primitives::SelectLead::Icon(Icon::MoveVertical)),
                    crate::primitives::SelectVariant::Outlined,
                );
                menu_popup(&resp).show(|ui| {
                    for (l, val) in [
                        (tr("Top"), "top"),
                        (tr("Bottom"), "bottom"),
                        (tr("Top & bottom"), "topBottom"),
                        (tr("Center"), "center"),
                        (tr("Scale"), "scale"),
                    ] {
                        if menu_item(ui, l, "") {
                            app.set_props(&ids, json!({"constraints": {"vertical": val}}));
                            ui.close();
                        }
                    }
                });
            });
            let (horizontal, vertical) = constraint_preview(ui, c, col);
            if let Some(value) = horizontal {
                app.set_props(&ids, json!({"constraints": {"horizontal": value}}));
            }
            if let Some(value) = vertical {
                app.set_props(&ids, json!({"constraints": {"vertical": value}}));
            }
        });
        ui.add_space(8.0);
    }
    block(ui, 24.0, 0.0, |ui| {
        let e = widgets::number_field_mixed(ui, Id::new("pos_rot"), Lead::PositionIcon(PositionIcon::Rotation, tr("Rotation")), rot, "°", col, 1.0);
        app.field_of(&ids, "rotation", e, rot_of, |v| json!(v));
        let items = [
            (PositionIcon::RotateRight, tr("Rotate 90° right")),
            (PositionIcon::FlipH, crate::shortcuts::tip(tr("Flip horizontal"), "object.flipH")),
            (PositionIcon::FlipV, crate::shortcuts::tip(tr("Flip vertical"), "object.flipV")),
        ];
        if let Some(i) = position_segments(ui, &items, col) {
            app.run(["object.rotateRight", "object.flipH", "object.flipV"][i], json!({"ids": ids}));
        }
    });
}

/// The compact parent/child diagram sits beside the two constraint selectors.
/// Its edge bars set the four pins; the centre bars set centred positioning.
fn constraint_preview(ui: &mut Ui, value: Option<Constraints>, width: f32) -> (Option<&'static str>, Option<&'static str>) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 56.0), Sense::hover());
    let tokens = Tokens::get(ui.ctx());
    ui.painter().rect_filled(rect, CornerRadius::same(5), tokens.field);
    let child = rect.shrink2(vec2(16.0, 16.0));
    ui.painter().rect_stroke(child, CornerRadius::same(5), Stroke::new(1.0, tokens.icon_secondary.gamma_multiply(0.4)), egui::StrokeKind::Inside);

    let h = value.map(|c| c.horizontal);
    let v = value.map(|c| c.vertical);
    let pins = [
        ("left", pos2(rect.left() + 8.0, rect.center().y), true, matches!(h, Some(HConstraint::Left | HConstraint::LeftRight))),
        ("right", pos2(rect.right() - 8.0, rect.center().y), true, matches!(h, Some(HConstraint::Right | HConstraint::LeftRight))),
        ("top", pos2(rect.center().x, rect.top() + 8.0), false, matches!(v, Some(VConstraint::Top | VConstraint::TopBottom))),
        ("bottom", pos2(rect.center().x, rect.bottom() - 8.0), false, matches!(v, Some(VConstraint::Bottom | VConstraint::TopBottom))),
    ];
    let mut horizontal = None;
    let mut vertical = None;
    for (name, centre, is_horizontal, active) in pins {
        let hit = Rect::from_center_size(centre, vec2(22.0, 22.0));
        let response = ui.interact(hit, ui.id().with(("constraint_pin", name)), Sense::click()).tip(tr(name));
        let color = if active { tokens.accent } else { tokens.icon_secondary };
        let half = if is_horizontal { vec2(5.0, 0.0) } else { vec2(0.0, 5.0) };
        ui.painter().line_segment([centre - half, centre + half], Stroke::new(if active { 3.0 } else { 1.5 }, color));
        if response.clicked() {
            if is_horizontal {
                horizontal = Some(name);
            } else {
                vertical = Some(name);
            }
        }
    }
    let center = child.center();
    let center_hit = Rect::from_center_size(center, vec2(22.0, 22.0));
    let response = ui.interact(center_hit, ui.id().with("constraint_center"), Sense::click()).tip(tr("Center"));
    let cross = Stroke::new(
        1.0,
        if matches!(h, Some(HConstraint::Center)) || matches!(v, Some(VConstraint::Center)) { tokens.accent } else { tokens.icon_secondary },
    );
    ui.painter().line_segment([center - vec2(7.0, 0.0), center + vec2(7.0, 0.0)], cross);
    ui.painter().line_segment([center - vec2(0.0, 7.0), center + vec2(0.0, 7.0)], cross);
    if response.clicked() {
        horizontal = Some("center");
        vertical = Some("center");
    }
    (horizontal, vertical)
}

/// The selected ellipses' arc: Start (degrees), Sweep (a share of a full turn, negative running
/// the other way) and Ratio (the inner radius), the same values the canvas arc handles drag
/// (`node.set`'s `arcStart` / `arcSweep` / `arcInner`; bindable to number variables, sweep and
/// ratio ones holding percentages).
fn arc_fields(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let arcs: Vec<(f64, f64, f64)> = sel
        .iter()
        .filter_map(|i| match doc.node(*i)?.kind {
            NodeKind::Ellipse { start, sweep, inner } => Some((start, sweep, inner)),
            _ => None,
        })
        .collect();
    if arcs.is_empty() || arcs.len() != sel.len() {
        return;
    }
    let ids = sel.to_vec();
    let same = |f: &dyn Fn(&(f64, f64, f64)) -> f64| -> Option<f64> {
        let v = (f(arcs.first()?) * 100.0).round() / 100.0;
        arcs.iter().all(|a| ((f(a) * 100.0).round() / 100.0 - v).abs() < 1e-9).then_some(v)
    };
    let start = same(&|a| a.0.to_degrees().rem_euclid(360.0));
    let sweep = same(&|a| a.1 / std::f64::consts::TAU * 100.0);
    let ratio = same(&|a| a.2 * 100.0);
    let field = |app: &mut LizardApp, ui: &mut Ui, key: &str, icon: Icon, name: &str, value: Option<f64>, suffix: &str, width: f32| {
        crate::variable_ui::number_field(app, ui, &ids, key, Id::new(("arc", key)), Lead::Icon(icon, name), value, suffix, width)
    };
    ui.add_space(8.0);
    row(ui, 24.0, |ui| {
        let width = (ui.available_width() - 16.0) / 3.0;
        let e = field(app, ui, "arcStart", Icon::ArcStart, tr("Arc start"), start, "°", width);
        app.field(&ids, "arcStart", e, |v| json!(v));
        let e = field(app, ui, "arcSweep", Icon::ArcSweep, tr("Arc sweep"), sweep, "%", width);
        app.field(&ids, "arcSweep", e, |v| json!(v.clamp(-100.0, 100.0) * 3.6));
        let e = field(app, ui, "arcInner", Icon::ArcRatio, tr("Arc ratio"), ratio, "%", width);
        app.field(&ids, "arcInner", e, |v| json!((v / 100.0).clamp(0.0, 0.99)));
    });
}

/// The selected polygons' and stars' point count (3 to 60 on stars), and the stars' ratio (the
/// inner radius as a share of the outer one), as `node.set`'s `pointCount` / `innerRadius`.
fn shape_points_fields(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let shapes: Vec<(u32, Option<f64>)> = sel
        .iter()
        .filter_map(|i| match doc.node(*i)?.kind {
            NodeKind::Polygon { count } => Some((count, None)),
            NodeKind::Star { count, inner_ratio } => Some((count, Some(inner_ratio))),
            _ => None,
        })
        .collect();
    if shapes.is_empty() || shapes.len() != sel.len() {
        return;
    }
    let ids = sel.to_vec();
    let count = shapes.first().map(|s| s.0).filter(|c| shapes.iter().all(|s| s.0 == *c)).map(f64::from);
    let stars: Vec<f64> = shapes.iter().filter_map(|s| s.1).collect();
    let ratio = stars.first().map(|r| (r * 1000.0).round() / 10.0).filter(|r| stars.iter().all(|x| ((x * 1000.0).round() / 10.0 - r).abs() < 1e-9));
    let all_stars = stars.len() == shapes.len();
    ui.add_space(8.0);
    row(ui, 24.0, |ui| {
        let col = ((ui.available_width() - 8.0) / 2.0).max(0.0);
        let width = if all_stars { col } else { ui.available_width() };
        let e = widgets::number_field(ui, Id::new("pts_count"), Lead::Icon(Icon::Polygon, tr("Count")), count, "", width, 1.0);
        app.field(&ids, "pointCount", e, |v| json!(v.round().clamp(3.0, 120.0)));
        if all_stars {
            let e = widgets::number_field(ui, Id::new("pts_ratio"), Lead::Icon(Icon::Star, tr("Ratio")), ratio, "%", col, 1.0);
            app.field(&ids, "innerRadius", e, |v| json!((v / 100.0).clamp(0.001, 1.0)));
        }
    });
}

/// Constraint names as shown (English keys, translated at display).
fn pretty(s: &str) -> String {
    match s {
        "LeftRight" => "Left & right".into(),
        "TopBottom" => "Top & bottom".into(),
        other => other.to_string(),
    }
}

fn appearance_header(ui: &mut Ui, visible: bool) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(rect.min.x + PAD, rect.center().y), Align2::LEFT_CENTER, tr("Appearance"), crate::theme::property_section_title(), t.text);
    let buttons = [(0, 1, crate::shortcuts::tip(tr("Show/Hide"), "object.toggleVisible")), (1, 0, tr("Blend mode"))];
    let mut clicked = None;
    for (index, column, tip) in buttons {
        let button = widgets::section_action_rect(rect, column);
        let response = ui.interact(button, ui.id().with(("appearance_header", index)), Sense::click()).tip(tip);
        if response.hovered() {
            ui.painter().rect_filled(button, CornerRadius::same(5), t.hover);
        }
        match index {
            0 if visible => icons::paint_appearance(ui.painter(), button.shrink(4.0), AppearanceIcon::Visible, t.icon),
            0 => icons::paint(ui.painter(), button.shrink(4.0), Icon::EyeOff, t.icon),
            _ => icons::paint_appearance(ui.painter(), button.shrink(4.0), AppearanceIcon::Blend, t.icon),
        }
        if response.clicked() {
            clicked = Some(index);
        }
    }
    clicked
}

fn appearance(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let ids = sel.to_vec();
    let visible = common(doc, sel, |n| n.visible).unwrap_or(true);
    let hdr = appearance_header(ui, visible);
    if hdr == Some(0) {
        app.run("object.toggleVisible", json!({"ids": ids}));
    }
    if hdr == Some(1) {
        ui.data_mut(|d| d.insert_temp(Id::new("blend_open"), !d.get_temp::<bool>(Id::new("blend_open")).unwrap_or(false)));
    }
    let col = widgets::col_width(ui.available_width());
    crate::variable_ui::bound_row(app, ui, &ids, "visible", VariableType::Boolean, Lead::Icon(Icon::Eye, tr("Visible")), f32::INFINITY);
    crate::component_panel::visibility_value_ui(app, ui, doc, sel);
    // Masks: how they mask.
    if sel.iter().all(|i| doc.node(*i).is_some_and(|n| n.is_mask)) {
        let mt = common(doc, sel, |n| n.mask_type);
        widgets::sublabels(ui, tr("Mask"), None);
        row(ui, 32.0, |ui| {
            let width = ui.available_width();
            let resp = crate::primitives::Select::trigger(ui, mt.map(|m| tr(m.label())).unwrap_or(tr("Mixed")), width, Some(Icon::Mask));
            menu_popup(&resp).show(|ui| {
                for m in MaskType::ALL {
                    if menu_item(ui, tr(m.label()), "") {
                        app.set_props(&ids, json!({"maskType": m.id()}));
                        ui.close();
                    }
                }
            });
        });
    }
    let op_of = |n: &Node| (f64::from(n.opacity) * 100.0).round();
    let op = common(doc, sel, op_of);
    let point_radius = app.session.ds().ok().and_then(|ds| ds.vector_edit.as_ref().and_then(|edit| selected_point_radius(doc, edit)));
    let radius_ok = point_radius.is_some() || sel.iter().all(|i| doc.node(*i).is_some_and(|n| n.kind.has_corner_radius()));
    // Only rectangles and frames round each corner on its own.
    let indep_ok = radius_ok && sel.iter().all(|i| doc.node(*i).is_some_and(|n| n.kind.has_independent_corners()));
    let r = match point_radius {
        Some(radius) => Some(radius),
        None => common(doc, sel, layer_corner_radius),
    };
    ui.add_space(3.0);
    row(ui, 24.0, |ui| {
        let lead = Lead::AppearanceIcon(AppearanceIcon::Opacity, tr("Opacity"));
        let width = if radius_ok { col } else { ui.available_width() };
        let e = crate::variable_ui::number_field_mixed(app, ui, &ids, "opacity", Id::new("ap_op"), lead, op, "%", width);
        app.field_of(&ids, "opacity", e, op_of, |v| json!((v / 100.0).clamp(0.0, 1.0)));
        if radius_ok {
            let lead = Lead::AppearanceIcon(AppearanceIcon::Corners, tr("Corner radius"));
            if point_radius.is_some() {
                let edit = widgets::number_field_mixed(ui, Id::new("ap_rad"), lead, r.flatten(), "", col, 1.0);
                match edit {
                    Some(Edit::All(field)) => app.field_run(Some(field), "vector.edit.cornerRadius", |v| json!({"radius": v.max(0.0)})),
                    Some(Edit::Each(widgets::Rel::Add(v))) => {
                        app.run("vector.edit.cornerRadius", json!({"add": v}));
                    }
                    Some(Edit::Each(widgets::Rel::Mul(v))) => {
                        app.run("vector.edit.cornerRadius", json!({"multiply": v}));
                    }
                    Some(Edit::Each(widgets::Rel::Div(v))) => {
                        app.run("vector.edit.cornerRadius", json!({"divide": v}));
                    }
                    None => {}
                }
            } else {
                let e = crate::variable_ui::number_field_mixed(app, ui, &ids, "cornerRadius", Id::new("ap_rad"), lead, r.flatten(), "", col);
                // A layer with different corner values starts relative edits from its largest.
                app.field_of(&ids, "cornerRadius", e, layer_corner_radius_base, |v| json!(v.max(0.0)));
            }
            let open = ui.data(|d| d.get_temp::<bool>(Id::new("indep_corners"))).unwrap_or(false);
            if indep_ok {
                ui.add_space(8.0);
                let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), Sense::click());
                let response = response.tip(tr("Individual corners"));
                crate::a11y::toggle(&response, tr("Individual corners"), open);
                let t = Tokens::get(ui.ctx());
                if open || response.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(5), if open { t.selected_row } else { t.hover });
                }
                icons::paint_appearance(ui.painter(), rect.shrink(4.0), AppearanceIcon::Corners, if open { t.accent_text } else { t.icon });
                if response.clicked() {
                    ui.data_mut(|d| d.insert_temp(Id::new("indep_corners"), !open));
                }
            }
        }
    });
    arc_fields(app, ui, doc, sel);
    shape_points_fields(app, ui, doc, sel);
    if indep_ok && ui.data(|d| d.get_temp::<bool>(Id::new("indep_corners"))).unwrap_or(false) {
        let radii = common(doc, sel, |n| n.corners.radii);
        let keys = [
            ("topLeftRadius", tr("Top left radius"), AppearanceIcon::CornerTopLeft, 0usize),
            ("topRightRadius", tr("Top right radius"), AppearanceIcon::CornerTopRight, 1),
            ("bottomLeftRadius", tr("Bottom left radius"), AppearanceIcon::CornerBottomLeft, 3),
            ("bottomRightRadius", tr("Bottom right radius"), AppearanceIcon::CornerBottomRight, 2),
        ];
        ui.add_space(8.0);
        row(ui, 24.0, |ui| {
            for (k, (key, name, icon, radius_index)) in keys.iter().enumerate().take(2) {
                let e = crate::variable_ui::number_field(
                    app,
                    ui,
                    &ids,
                    key,
                    Id::new(("corner", k)),
                    Lead::AppearanceIcon(*icon, name),
                    radii.map(|r| r[*radius_index]),
                    "",
                    col,
                );
                app.field(&ids, key, e, |v| json!(v.max(0.0)));
            }
        });
        ui.add_space(8.0);
        row(ui, 24.0, |ui| {
            for (k, (key, name, icon, radius_index)) in keys.iter().enumerate().skip(2) {
                let e = crate::variable_ui::number_field(
                    app,
                    ui,
                    &ids,
                    key,
                    Id::new(("corner", k)),
                    Lead::AppearanceIcon(*icon, name),
                    radii.map(|r| r[*radius_index]),
                    "",
                    col,
                );
                app.field(&ids, key, e, |v| json!(v.max(0.0)));
            }
            let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
            let response = response.tip(tr("Corner smoothing"));
            let t = Tokens::get(ui.ctx());
            if response.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(5), t.hover);
            }
            icons::paint_appearance(ui.painter(), rect.shrink(4.0), AppearanceIcon::CornerSettings, t.icon);
            let open_id = Id::new("corner_smoothing_open");
            let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
            if response.clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(open_id, open));
            }
            if open {
                let initial = pos2((rect.left() - 248.0).max(ui.ctx().content_rect().left() + 8.0), rect.top());
                let mut keep = true;
                let mut close_requested = false;
                crate::primitives::FloatingPanel::new(Id::new("corner_smoothing_panel"), initial)
                    .height(crate::primitives::FloatingPanelHeight::Fixed(92.0))
                    .open(&mut keep)
                    .show_with_header(ui.ctx(), |ui, header| {
                        let modal = Rect::from_min_size(header.min, vec2(240.0, 92.0));
                        ui.allocate_exact_size(vec2(240.0, 52.0), Sense::hover());
                        ui.painter().text(
                            pos2(modal.min.x + 16.0, modal.min.y + 20.0),
                            Align2::LEFT_CENTER,
                            tr("Corner smoothing"),
                            semibold(11.0),
                            Color32::WHITE,
                        );
                        ui.painter().line_segment(
                            [pos2(modal.min.x, modal.min.y + 40.0), pos2(modal.max.x, modal.min.y + 40.0)],
                            Stroke::new(1.0, Color32::from_rgb(0x38, 0x38, 0x38)),
                        );
                        let close = Rect::from_center_size(pos2(modal.max.x - 20.0, modal.min.y + 20.0), vec2(24.0, 24.0));
                        if crate::primitives::FloatingPanel::close_button(ui, ui.id().with("corner_smoothing_close"), close, |painter, rect| {
                            let cross = rect.center();
                            painter.line_segment([cross + vec2(-4.5, -4.5), cross + vec2(4.5, 4.5)], Stroke::new(1.2, Color32::WHITE));
                            painter.line_segment([cross + vec2(4.5, -4.5), cross + vec2(-4.5, 4.5)], Stroke::new(1.2, Color32::WHITE));
                        }) {
                            close_requested = true;
                        }

                        let painter = ui.painter();
                        let value = common(doc, sel, |n| (n.corners.smoothing * 100.0).round()).unwrap_or(0.0).clamp(0.0, 100.0);
                        let track = Rect::from_min_size(modal.min + vec2(16.0, 53.0), vec2(158.0, 16.0));
                        let knob_x = track.min.x + 8.0 + (track.width() - 16.0) * (value as f32 / 100.0);
                        painter.rect_filled(track, CornerRadius::same(8), Color32::from_rgb(0x30, 0x30, 0x30));
                        painter.rect_filled(
                            Rect::from_min_max(track.min + vec2(1.0, 1.0), pos2(knob_x, track.max.y - 1.0)),
                            CornerRadius::same(7),
                            Tokens::get(ui.ctx()).accent,
                        );
                        painter.rect_stroke(
                            track,
                            CornerRadius::same(8),
                            Stroke::new(1.0, Color32::from_rgb(0x58, 0x58, 0x58)),
                            egui::StrokeKind::Inside,
                        );
                        let marker_x = track.min.x + track.width() * 0.60;
                        painter.circle_filled(pos2(marker_x, track.center().y), 1.7, Color32::from_rgb(0x78, 0x78, 0x78));
                        painter.text(
                            pos2(marker_x, modal.min.y + 81.0),
                            Align2::CENTER_CENTER,
                            "iOS",
                            regular(10.0),
                            Color32::from_rgb(0x78, 0x78, 0x78),
                        );
                        painter.circle_filled(pos2(knob_x, track.center().y + 1.0), 8.0, Color32::from_rgb(0xf8, 0xf8, 0xf8));
                        let slider = ui.interact(track, ui.id().with("corner_smoothing_slider"), Sense::click_and_drag());
                        if (slider.dragged() || slider.drag_stopped() || slider.clicked())
                            && let Some(pointer) = slider.interact_pointer_pos()
                        {
                            let percent = ((pointer.x - track.min.x - 8.0) / (track.width() - 16.0) * 100.0).round().clamp(0.0, 100.0);
                            let edit = if slider.clicked() {
                                FieldEdit::Set(f64::from(percent))
                            } else {
                                FieldEdit::Scrub(f64::from(percent), slider.drag_stopped())
                            };
                            app.field(&ids, "cornerSmoothing", Some(edit), |v| json!((v / 100.0).clamp(0.0, 1.0)));
                        }
                        let field = Rect::from_min_size(modal.min + vec2(180.0, 49.0), vec2(44.0, 24.0));
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field).layout(egui::Layout::left_to_right(egui::Align::Center)));
                        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        let e = widgets::number_field(&mut child, Id::new("smoothing"), Lead::None, Some(value), "%", 44.0, 1.0);
                        app.field(&ids, "cornerSmoothing", e, |v| json!((v / 100.0).clamp(0.0, 1.0)));
                    });
                if !keep || close_requested {
                    ui.data_mut(|d| d.insert_temp(open_id, false));
                }
            }
        });
    }
    let blend = common(doc, sel, |n| n.blend);
    let show_blend = ui.data(|d| d.get_temp::<bool>(Id::new("blend_open"))).unwrap_or(false)
        || blend.is_some_and(|b| !matches!(b, BlendMode::Normal | BlendMode::PassThrough));
    if show_blend {
        widgets::sublabels(ui, tr("Blend mode"), None);
        row(ui, 32.0, |ui| {
            let label = blend.map(|b| tr(b.label())).unwrap_or(tr("Mixed"));
            let width = ui.available_width();
            let resp = crate::primitives::Select::trigger(ui, label, width, Some(Icon::Blend));
            menu_popup(&resp).width(200.0).show(|ui| {
                if let Some(b) = blend_menu(ui, true) {
                    app.set_props(&ids, json!({"blend": b.label()}));
                }
            });
        });
    }
    ui.add_space(12.0);
}

/// The variable modes of the selected frames (sections, components, instances): a row per
/// collection with more than one mode, Auto (the mode the layer inherits from its parent) or a mode
/// of its own (`variable.setMode`). Collections from enabled libraries follow under their
/// library's name (`library.setVariableMode` copies the collection in). Returns whether it showed.
fn variable_modes(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) -> bool {
    if !sel.iter().all(|i| doc.node(*i).is_some_and(|n| n.kind.is_frame_like())) {
        return false;
    }
    let libs = crate::variable_ui::libraries(app, doc);
    let rows = crate::variable_ui::mode_rows(doc, &libs);
    if rows.is_empty() {
        return false;
    }
    let t = Tokens::get(ui.ctx());
    widgets::section_header(ui, tr("Variable modes"), &[], false);
    // The collection's name, then a wider mode picker ("Auto (Light)") across both field columns.
    let col = widgets::col_width(ui.available_width());
    let name_w = (col * 0.7).round();
    // Auto resolves as for the first selected layer: from its parent, or an instance's main.
    let parent_modes = sel.first().map(|i| doc.inherited_modes(*i)).unwrap_or_default();
    let mut group = None;
    for r in &rows {
        let c = r.coll;
        // Library collections under their library's name.
        if r.library != group {
            group = r.library;
            if let Some((_, lib)) = r.library {
                widgets::sublabels(ui, lib, None);
            }
        }
        let name_of = |m: &str| c.modes.iter().find(|x| x.id == m).map_or("", |x| x.name.as_str());
        // Each layer's own mode (None: Auto); None overall when they differ.
        let own = match r.local {
            Some(l) => common(doc, sel, |n| n.var_modes.get(l).cloned()),
            None => Some(None),
        };
        let inherited = r.local.and_then(|l| parent_modes.get(l)).or(c.default_mode()).map_or("", |m| name_of(m));
        let auto = trf("Auto ({mode})", &[("mode", &inherited)]);
        let label = match &own {
            Some(Some(m)) => name_of(m).to_string(),
            Some(None) => auto.clone(),
            None => tr("Mixed").into(),
        };
        let hover = match r.library {
            Some((_, lib)) => trf("{name} (from {library})", &[("name", &c.name), ("library", &lib)]),
            None => c.name.clone(),
        };
        row(ui, 32.0, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(name_w, FIELD_H), Sense::hover());
            let name = widgets::elide(ui, &c.name, name_w);
            ui.painter().text(pos2(rect.min.x, rect.center().y), Align2::LEFT_CENTER, &name, regular(11.0), t.text);
            if name != c.name || r.library.is_some() {
                ui.interact(rect, Id::new(("var_mode_name", &c.id)), Sense::hover()).lizard_tooltip(&hover);
            }
            let width = ui.available_width();
            let resp = crate::primitives::Select::trigger(ui, &label, width, None);
            menu_popup(&resp).width(180.0).show(|ui| {
                let mut pick = None;
                if menu_check(ui, own == Some(None), &auto, "") {
                    pick = Some(Value::Null);
                }
                crate::primitives::Menu::separator(ui, None);
                for m in &c.modes {
                    if menu_check(ui, own.as_ref().is_some_and(|o| o.as_ref() == Some(&m.id)), &m.name, "") {
                        pick = Some(json!(m.id));
                    }
                }
                if let Some(mode) = pick {
                    set_variable_mode(app, sel, r, mode);
                    ui.close();
                }
            });
        });
    }
    ui.add_space(12.0);
    true
}

/// Set `sel`'s mode of the collection of `row` (`mode` null: Auto). A library collection goes
/// through `library.setVariableMode` (copying it in); Auto on one never used here is already so.
fn set_variable_mode(app: &mut LizardApp, sel: &[NodeId], row: &crate::variable_ui::ModeRow, mode: Value) {
    match (row.library, row.key, row.local) {
        (Some((lib, _)), Some(key), _) if !mode.is_null() => {
            app.run("library.setVariableMode", json!({"library": lib, "collection": key, "ids": sel, "mode": mode}));
        }
        (_, _, Some(local)) => {
            app.run("variable.setMode", json!({"ids": sel, "collection": local, "mode": mode}));
        }
        _ => {}
    }
}

/// While a range of text is selected, the Fill section edits that range's colour.
fn range_fill(app: &mut LizardApp, ui: &mut Ui, doc: &Document, id: NodeId, start: usize, end: usize) {
    let t = Tokens::get(ui.ctx());
    widgets::section_header(ui, tr("Fill"), &[], false);
    // The fills over the range, if every character shares them.
    let fills: Option<Vec<Paint>> = doc.node(id).and_then(|n| n.kind.text().map(|tx| (n, tx))).and_then(|(n, tx)| {
        let mut it =
            tx.spans().into_iter().filter(|(s, e, _)| *s < end && *e > start).map(|(_, _, st)| st.fills.clone().unwrap_or_else(|| n.fills.clone()));
        let first = it.next()?;
        it.all(|f| f == first).then_some(first)
    });
    row(ui, 32.0, |ui| {
        let w = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(5), t.field);
        let sw = Rect::from_min_size(pos2(r.min.x + 4.0, r.center().y - 8.0), vec2(16.0, 16.0));
        let text_r = Rect::from_min_max(pos2(sw.max.x + 8.0, r.min.y), pos2(r.max.x - 4.0, r.max.y));
        match fills.as_ref().and_then(|f| f.first()) {
            Some(p) => {
                widgets::paint_swatch(ui.painter(), sw, p);
                if ui.interact(sw.expand(2.0), Id::new("range_swatch"), Sense::click()).clicked() {
                    app.ui.picker = Some(crate::colorpicker::PickerTarget::TextRange { id, start, end });
                }
                if let Some(c) = p.solid_color() {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_r));
                    if let Some(h) =
                        widgets::text_field_inline(&mut child, Id::new("range_hex"), &c.to_hex(), text_r.shrink2(vec2(0.0, 4.0)), regular(11.0))
                        && let Some(nc) = lizard_color::Color::from_hex_or_name(&h)
                    {
                        app.run("text.style", json!({"id": id, "start": start, "end": end, "textFills": [nc.to_css_hex()]}));
                    }
                } else {
                    ui.painter().text(pos2(text_r.min.x, r.center().y), Align2::LEFT_CENTER, tr("Gradient"), regular(11.0), t.text);
                }
            }
            None => {
                ui.painter().text(pos2(text_r.min.x, r.center().y), Align2::LEFT_CENTER, tr("Mixed"), regular(11.0), t.text_secondary);
                if ui.interact(r, Id::new("range_mixed"), Sense::click()).clicked() {
                    app.ui.picker = Some(crate::colorpicker::PickerTarget::TextRange { id, start, end });
                }
            }
        }
        // Back to the layer's fill.
        if icons::button(ui, Icon::Detach, 24.0, false, tr("Use the layer's fill")).clicked() {
            app.run("text.style", json!({"id": id, "start": start, "end": end, "textFills": null}));
        }
    });
    ui.add_space(12.0);
}

fn paints(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId], fills: bool) {
    if fills
        && let Some(ve) = app.session.ds().ok().and_then(|ds| ds.vector_edit.as_ref())
        && !ve.regions.is_empty()
        && sel.contains(&ve.id)
        && let Some(node) = doc.node(ve.id)
        && let NodeKind::Vector { network } = &node.kind
        && let Some(region) = ve.regions.first().and_then(|index| network.regions.get(*index))
    {
        region_paints(app, ui, ve.id, ve.regions.clone(), region.fills.as_ref().unwrap_or(&node.fills).clone());
        return;
    }
    if fills
        && let Some((id, a, b)) = lizard_engine::textedit::selected_range(&app.session)
        && a < b
        && sel.contains(&id)
    {
        range_fill(app, ui, doc, id, a, b);
        return;
    }
    let t = Tokens::get(ui.ctx());
    let ids = sel.to_vec();
    let list = common(doc, sel, |n| if fills { n.fills.clone() } else { n.strokes.clone() });
    let title = if fills { tr("Fill") } else { tr("Stroke") };
    let empty = list.as_ref().is_none_or(|l| l.is_empty());
    let slot = if fills { StyleSlot::Fill } else { StyleSlot::Stroke };
    if let Some(i) = paint_section_header(ui, title, empty, fills) {
        if i == 1 {
            // The new paint goes on top; open its picker. A mixed selection has no one index to edit.
            if app.run(if fills { "fill.add" } else { "stroke.add" }, json!({"ids": ids})).is_some()
                && let Some(l) = &list
            {
                app.ui.picker = Some(crate::colorpicker::PickerTarget::Paint { fills, index: l.len(), ids: ids.clone() });
            }
        } else {
            style_picker::open(app, ui, &ids, slot);
        }
    }
    let styled = common(doc, sel, |node| if fills { node.styles.fill.clone() } else { node.styles.stroke.clone() })
        .flatten()
        .and_then(|id| doc.style(&id))
        .and_then(|style| {
            let StyleKind::Paint { paints } = &style.kind else { return None };
            let preview = paints
                .iter()
                .rev()
                .find(|paint| paint.visible)
                .or_else(|| paints.last())
                .cloned()
                .unwrap_or_else(|| Paint::solid(lizard_color::Color::TRANSPARENT));
            Some((style.name.as_str(), preview))
        });
    let has_content = styled.is_some() || list.as_ref().is_none_or(|l| !l.is_empty());
    if let Some((name, preview)) = styled {
        styled_paint_row(app, ui, &ids, fills, name, &preview);
    } else {
        match &list {
            None => widgets::note(ui, tr("Click + to replace mixed content"), t.text_secondary),
            Some(l) => {
                // The last paint is on top: shown first. Rows drag to reorder.
                let mut drag = ListDrag::new(ui, if fills { "fills" } else { "strokes" });
                for (i, p) in l.iter().enumerate().rev() {
                    let r = paint_row(app, ui, &ids, i, p, fills, None);
                    drag.row(ui, i, r, r, || paint_label(p));
                }
                if let Some((from, to)) = drag.finish(ui) {
                    app.run(if fills { "fill.move" } else { "stroke.move" }, json!({"ids": ids, "from": from, "to": to}));
                    // An open colour picker keeps editing the paint it was on.
                    if let Some(crate::colorpicker::PickerTarget::Paint { fills: f, index, .. }) = &mut app.ui.picker
                        && *f == fills
                    {
                        *index = moved_index(*index, from, to);
                    }
                }
            }
        }
    }
    if !fills && !empty {
        let weight = common(doc, sel, |n| n.stroke.weight);
        let align = common(doc, sel, |n| n.stroke.align);
        let sided = sel.iter().all(|i| doc.node(*i).is_some_and(|n| n.kind.is_frame_like() || matches!(n.kind, NodeKind::Rectangle)));
        let t = Tokens::get(ui.ctx());
        let (controls, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let field_width = ((controls.width() - 92.0) / 2.0).max(48.0);
        let align_rect = Rect::from_min_size(pos2(controls.left() + 16.0, controls.top() + 4.0), vec2(field_width, 24.0));
        let weight_rect = Rect::from_min_size(pos2(align_rect.right() + 8.0, align_rect.top()), vec2(field_width, 24.0));
        let align_label = match align {
            Some(StrokeAlign::Inside) => tr("Inside"),
            Some(StrokeAlign::Center) => tr("Center"),
            Some(StrokeAlign::Outside) => tr("Outside"),
            None => tr("Mixed"),
        };
        let resp = ui.interact(align_rect, Id::new("stroke_align"), Sense::click()).tip(tr("Stroke alignment"));
        ui.painter().rect_filled(align_rect, CornerRadius::same(5), t.panel);
        ui.painter().rect_stroke(
            align_rect,
            CornerRadius::same(5),
            Stroke::new(1.0, if resp.hovered() { t.field_hover_border } else { t.border }),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(pos2(align_rect.left() + 8.0, align_rect.center().y), Align2::LEFT_CENTER, align_label, regular(11.0), t.text);
        icons::paint_colorpicker(
            ui.painter(),
            Rect::from_center_size(pos2(align_rect.right() - 12.0, align_rect.center().y), vec2(5.002, 3.002)),
            7,
            t.icon,
        );
        {
            menu_popup(&resp).show(|ui| {
                for (l, v) in [(tr("Inside"), "inside"), (tr("Center"), "center"), (tr("Outside"), "outside")] {
                    if menu_item(ui, l, "") {
                        app.set_props(&ids, json!({"strokeAlign": v}));
                        ui.close();
                    }
                }
            });
        }
        {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(weight_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
            let e = crate::variable_ui::number_field_mixed(
                app,
                &mut child,
                &ids,
                "strokeWeight",
                Id::new("stroke_w"),
                Lead::PropertyIcon(1, tr("Stroke weight")),
                weight,
                "",
                field_width,
            );
            app.field_of(&ids, "strokeWeight", e, |n| n.stroke.weight, |v| json!(v.max(0.0)));
        }
        let advanced = Rect::from_min_size(pos2(weight_rect.right() + 8.0, align_rect.top()), vec2(24.0, 24.0));
        let advanced_btn = property_icon_response(ui, advanced, 2, tr("Advanced stroke"), false, true);
        let panel_id = Id::new("stroke_settings_panel");
        if crate::primitives::FloatingPanel::toggle(ui.ctx(), panel_id, advanced_btn.clicked()) {
            let ctx = ui.ctx().clone();
            let initial = pos2(advanced.left() - 248.0, advanced.top());
            let mut close = false;
            crate::primitives::FloatingPanel::new(panel_id, initial).height(crate::primitives::FloatingPanelHeight::Fixed(224.0)).show_with_header(
                &ctx,
                |ui, header| {
                    close = stroke_settings_panel(app, ui, doc, sel, header);
                },
            );
            if close {
                crate::primitives::FloatingPanel::dismiss(&ctx, panel_id);
            }
        }
        let sides = Rect::from_min_size(pos2(controls.right() - 32.0, align_rect.top()), vec2(24.0, 24.0));
        let sides_btn = property_icon_response(ui, sides, 3, tr("Individual strokes"), false, sided);
        if sided {
            let current = common(doc, sel, |n| n.stroke.sides);
            let selected = match current {
                Some(None) => Some(0),
                Some(Some([top, 0.0, 0.0, 0.0])) if top > 0.0 => Some(1),
                Some(Some([0.0, 0.0, bottom, 0.0])) if bottom > 0.0 => Some(2),
                Some(Some([0.0, 0.0, 0.0, left])) if left > 0.0 => Some(3),
                Some(Some([0.0, right, 0.0, 0.0])) if right > 0.0 => Some(4),
                Some(Some(_)) => Some(5),
                None => None,
            };
            crate::primitives::Menu::popup(&sides_btn).width(150.0).show(|ui| {
                ui.set_width(150.0);
                let w = weight.unwrap_or(1.0);
                for (index, label, value) in [
                    (0, tr("All"), Value::Null),
                    (1, tr("Top"), json!([w, 0.0, 0.0, 0.0])),
                    (2, tr("Bottom"), json!([0.0, 0.0, w, 0.0])),
                    (3, tr("Left"), json!([0.0, 0.0, 0.0, w])),
                    (4, tr("Right"), json!([0.0, w, 0.0, 0.0])),
                ] {
                    if stroke_side_menu_row(ui, label, index, selected == Some(index)) {
                        app.set_props(&ids, json!({"strokeSides": value}));
                        ui.close();
                    }
                }
                crate::primitives::Menu::separator(ui, None);
                if stroke_side_menu_row(ui, tr("Custom"), 5, selected == Some(5)) {
                    app.set_props(&ids, json!({"strokeSides": [w, w, w, w]}));
                    ui.close();
                }
            });
        }
        // Per-side weights, two to a row (like the corner radii) so a bound variable's name fits:
        // each side binds to a number variable on its own.
        if let Some(sides) = common(doc, sel, |n| n.stroke.sides).flatten() {
            let col = widgets::col_width(ui.available_width());
            let fields = ["T", "R", "B", "L"].into_iter().zip(lizard_engine::cmd::props::STROKE_SIDES);
            for first in [0, 2] {
                self::row(ui, 32.0, |ui| {
                    for (k, (label, field)) in fields.clone().enumerate().skip(first).take(2) {
                        let v = sides.get(k).copied();
                        let e = crate::variable_ui::number_field(app, ui, &ids, field, Id::new(("stroke_side", k)), Lead::Text(label), v, "", col);
                        app.field(&ids, field, e, |x| json!(x.max(0.0)));
                    }
                });
            }
        }
    }
    if has_content {
        ui.add_space(12.0);
    }
}

fn region_paints(app: &mut LizardApp, ui: &mut Ui, id: NodeId, regions: Vec<usize>, list: Vec<Paint>) {
    if widgets::section_header(ui, tr("Fill"), &[(Icon::Plus, tr("Add fill"))], list.is_empty()).is_some()
        && app.run("vector.region.fill.add", json!({"id": id, "regions": regions})).is_some()
    {
        app.ui.picker = Some(crate::colorpicker::PickerTarget::VectorRegion { id, regions: regions.clone(), index: list.len() });
    }
    let mut drag = ListDrag::new(ui, "vector_region_fills");
    for (index, paint) in list.iter().enumerate().rev() {
        let row = paint_row(app, ui, &[id], index, paint, true, Some(&regions));
        drag.row(ui, index, row, row, || paint_label(paint));
    }
    if let Some((from, to)) = drag.finish(ui) {
        app.run("vector.region.fill.move", json!({"id": id, "regions": regions, "from": from, "to": to}));
        if let Some(crate::colorpicker::PickerTarget::VectorRegion { index, .. }) = &mut app.ui.picker {
            *index = moved_index(*index, from, to);
        }
    }
    if !list.is_empty() {
        ui.add_space(12.0);
    }
}

/// A linked paint style occupies one row for its whole paint list. The style control opens the
/// browser; unlink keeps the paints as editable values, while remove clears the paint list.
fn styled_paint_row(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], fills: bool, name: &str, preview: &Paint) {
    let t = Tokens::get(ui.ctx());
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let control = Rect::from_min_size(pos2(row.left() + 16.0, row.top() + 4.0), vec2((row.width() - 84.0).max(64.0), 24.0));
    let selected =
        crate::primitives::ColorControl::new(Id::new(("paint_style", fills)), preview, crate::primitives::ColorValue::StyleName(name), 100.0)
            .show_at(ui, control);
    if selected.open_picker {
        style_picker::open(app, ui, ids, if fills { StyleSlot::Fill } else { StyleSlot::Stroke });
    }
    let detach = Rect::from_min_size(pos2(control.right() + 8.0, control.top()), vec2(24.0, 24.0));
    let unlink = ui.interact(detach, Id::new(("paint_style_detach", fills)), Sense::click()).tip(tr("Detach style"));
    if unlink.hovered() {
        ui.painter().rect_filled(detach, CornerRadius::same(5), t.hover);
    }
    icons::paint_color_detach(ui.painter(), Rect::from_center_size(detach.center(), vec2(14.0, 14.0)), t.icon);
    if unlink.clicked() {
        app.run("style.detach", json!({"ids": ids, "which": if fills { "fill" } else { "stroke" }}));
    }
    let remove = Rect::from_min_size(pos2(row.right() - 32.0, control.top()), vec2(24.0, 24.0));
    if Button::icon(Icon::Minus, tr("Remove")).icon_size(14.0).show_at(ui, Id::new(("paint_style_remove", fills)), remove).clicked() {
        let props = if fills { json!({"fills": []}) } else { json!({"strokes": []}) };
        app.run(
            "command.batch",
            json!({
                "label": if fills { tr("Remove fill") } else { tr("Remove stroke") },
                "commands": [
                    {"command": "style.detach", "params": {"ids": ids, "which": if fills { "fill" } else { "stroke" }}},
                    {"command": "node.set", "params": {"ids": ids, "props": props}},
                ],
            }),
        );
    }
}

fn stroke_side_menu_row(ui: &mut Ui, label: &str, index: usize, selected: bool) -> bool {
    crate::primitives::MenuItem::new(label)
        .checked(selected)
        .show_with_parts(
            ui,
            20.0,
            |painter, rect, color| {
                let icon = Rect::from_center_size(rect.center(), vec2(12.0, 12.0));
                if index == 0 {
                    icons::paint_property(painter, icon, 3, color);
                } else if index < 5 {
                    icons::paint_stroke_side(painter, icon, index, color);
                } else {
                    icons::paint_property(painter, icon, 2, color);
                }
            },
            0.0,
            |_, _, _| {},
        )
        .clicked()
}

/// Compact floating editor opened from the advanced stroke button.
fn stroke_settings_panel(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId], header: Rect) -> bool {
    let ids = sel.to_vec();
    let t = Tokens::get(ui.ctx());
    let dark = t.dark;
    let bg = if dark { Color32::from_rgb(0x2c, 0x2c, 0x2c) } else { t.panel };
    let raised = if dark { Color32::from_rgb(0x38, 0x38, 0x38) } else { t.field };
    let border = if dark { Color32::from_rgb(0x44, 0x44, 0x44) } else { t.border };
    let secondary = if dark { Color32::from_white_alpha(178) } else { t.text_secondary };
    let panel = Rect::from_min_size(header.min, vec2(240.0, 224.0));
    ui.allocate_exact_size(vec2(240.0, 184.0), Sense::hover());
    let p = ui.painter().clone();
    p.line_segment([pos2(panel.left(), panel.top() + 39.5), pos2(panel.right(), panel.top() + 39.5)], Stroke::new(1.0, raised));
    p.text(pos2(panel.left() + 16.0, panel.top() + 20.0), Align2::LEFT_CENTER, tr("Stroke settings"), semibold(11.0), t.text);
    let close = Rect::from_min_size(pos2(panel.right() - 32.0, panel.top() + 8.0), vec2(24.0, 24.0));
    let close_requested = crate::primitives::FloatingPanel::close_button(ui, ui.id().with("stroke_settings_close"), close, |painter, rect| {
        icons::paint_colorpicker(painter, Rect::from_center_size(rect.center(), vec2(10.01, 10.01)), 9, t.icon);
    });

    let tab_id = Id::new("stroke_settings_tab");
    let mut tab = ui.data(|d| d.get_temp::<usize>(tab_id)).unwrap_or(0);
    let tabs = Rect::from_min_size(pos2(panel.left() + 16.0, panel.top() + 52.0), vec2(208.0, 24.0));
    p.rect_filled(tabs, CornerRadius::same(5), raised);
    for (i, label) in [tr("Basic"), tr("Dynamic"), tr("Brush")].iter().enumerate() {
        let x0 = tabs.left() + i as f32 * tabs.width() / 3.0;
        let x1 = tabs.left() + (i + 1) as f32 * tabs.width() / 3.0;
        let rect = Rect::from_min_max(pos2(x0, tabs.top()), pos2(x1, tabs.bottom()));
        let response = ui.interact(rect, ui.id().with(("stroke_settings_tab", i)), Sense::click());
        if response.clicked() {
            tab = i;
            ui.data_mut(|d| d.insert_temp(tab_id, tab));
        }
        if tab == i {
            p.rect_filled(rect.shrink(2.0), CornerRadius::same(4), bg);
        } else if response.hovered() {
            p.rect_filled(rect.shrink(2.0), CornerRadius::same(4), t.hover);
        }
        p.text(rect.center(), Align2::CENTER_CENTER, label, regular(11.0), if tab == i { t.text } else { secondary });
    }

    if tab != 0 {
        let body = Rect::from_min_max(pos2(panel.left(), panel.top() + 76.0), panel.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
        egui::ScrollArea::vertical().max_height(148.0).show(&mut child, |ui| {
            if tab == 1 {
                stroke_settings(app, ui, doc, sel);
            } else {
                let brush = common(doc, sel, |n| n.stroke.brush.clone());
                brush_row(app, ui, &ids, brush, 208.0);
            }
        });
        return close_requested;
    }

    let stroke = common(doc, sel, |n| n.stroke.clone());
    let content_y = panel.top() + 84.0;
    let field_x = panel.left() + 96.0;
    let field = |ui: &mut Ui, row: usize, width: f32, id: &'static str| {
        let rect = Rect::from_min_size(pos2(field_x, content_y + row as f32 * 32.0 + 4.0), vec2(width, 24.0));
        ui.interact(rect, ui.id().with(id), Sense::click())
    };
    for (i, label) in [tr("Style"), tr("Width profile"), tr("Join|stroke"), tr("Miter angle")].iter().enumerate() {
        p.text(pos2(panel.left() + 16.0, content_y + i as f32 * 32.0 + 16.0), Align2::LEFT_CENTER, label, regular(11.0), secondary);
    }

    let dashed = stroke.as_ref().is_some_and(|s| !s.dashes.is_empty());
    let style = field(ui, 0, 128.0, "stroke_settings_style");
    p.rect_filled(style.rect, CornerRadius::same(5), bg);
    p.rect_stroke(style.rect, CornerRadius::same(5), Stroke::new(1.0, border), egui::StrokeKind::Inside);
    let sy = style.rect.center().y;
    if dashed {
        for dx in [0.0, 6.0, 12.0] {
            p.line_segment([pos2(style.rect.left() + 10.0 + dx, sy), pos2(style.rect.left() + 13.0 + dx, sy)], Stroke::new(1.0, t.text));
        }
    } else {
        p.line_segment([pos2(style.rect.left() + 10.0, sy), pos2(style.rect.left() + 24.0, sy)], Stroke::new(1.0, t.text));
    }
    p.text(pos2(style.rect.left() + 32.0, sy), Align2::LEFT_CENTER, if dashed { tr("Dash") } else { tr("Solid") }, regular(11.0), t.text);
    icons::paint_colorpicker(&p, Rect::from_center_size(pos2(style.rect.right() - 13.0, sy), vec2(5.0, 3.0)), 7, t.icon);
    menu_popup(&style).show(|ui| {
        for (label, value) in [(tr("Solid"), false), (tr("Dash"), true)] {
            if menu_item(ui, label, "") {
                let weight = stroke.as_ref().map_or(1.0, |s| s.weight.max(1.0));
                app.set_props(&ids, json!({"dashes": if value { vec![weight * 4.0, weight * 4.0] } else { vec![] }}));
                ui.close();
            }
        }
    });

    let profile = stroke.as_ref().and_then(|s| s.profile().cloned());
    let profile_btn = field(ui, 1, 96.0, "stroke_settings_profile");
    p.rect_filled(profile_btn.rect, CornerRadius::same(5), bg);
    p.rect_stroke(profile_btn.rect, CornerRadius::same(5), Stroke::new(1.0, border), egui::StrokeKind::Inside);
    profile_art(
        &p,
        Rect::from_center_size(pos2(profile_btn.rect.left() + 39.0, profile_btn.rect.center().y), vec2(62.0, 4.0)),
        profile.as_ref(),
        t.text,
    );
    icons::paint_colorpicker(
        &p,
        Rect::from_center_size(pos2(profile_btn.rect.right() - 13.0, profile_btn.rect.center().y), vec2(5.0, 3.0)),
        7,
        t.icon,
    );
    menu_popup(&profile_btn).show(|ui| {
        for preset in WidthPreset::ALL {
            let prof = preset.profile();
            let art = |pt: &egui::Painter, r: Rect, col: Color32| {
                profile_art(pt, Rect::from_center_size(pos2(r.right() - 40.0, r.center().y), vec2(62.0, 8.0)), prof.as_ref(), col);
            };
            if menu_item_with(ui, tr(preset.label()), "", 72.0, art) {
                app.run("stroke.widthProfile", json!({"ids": ids, "preset": preset.id()}));
                ui.close();
            }
        }
    });
    let flip_rect = Rect::from_min_size(pos2(field_x + 104.0, profile_btn.rect.top()), vec2(24.0, 24.0));
    let flip = ui.interact(flip_rect, ui.id().with("stroke_settings_flip"), Sense::click()).tip(tr("Flip width profile"));
    let flip_col = if profile.is_some() { t.icon } else { secondary };
    // Two opposing tapered ends with a central axis.
    p.line_segment(
        [pos2(flip_rect.center().x, flip_rect.top() + 6.0), pos2(flip_rect.center().x, flip_rect.bottom() - 6.0)],
        Stroke::new(1.0, flip_col),
    );
    p.add(egui::Shape::convex_polygon(
        vec![
            pos2(flip_rect.left() + 6.0, flip_rect.center().y - 4.0),
            pos2(flip_rect.left() + 6.0, flip_rect.center().y + 4.0),
            pos2(flip_rect.center().x - 3.0, flip_rect.center().y),
        ],
        flip_col,
        Stroke::NONE,
    ));
    p.add(egui::Shape::convex_polygon(
        vec![
            pos2(flip_rect.right() - 6.0, flip_rect.center().y - 4.0),
            pos2(flip_rect.right() - 6.0, flip_rect.center().y + 4.0),
            pos2(flip_rect.center().x + 3.0, flip_rect.center().y),
        ],
        flip_col,
        Stroke::NONE,
    ));
    if flip.clicked()
        && let Some(profile) = profile
    {
        let points: Vec<Value> =
            profile.points.iter().rev().map(|q| lizard_engine::cmd::props::width_point_json(&WidthPoint { t: 1.0 - q.t, ..*q })).collect();
        app.run("stroke.widthProfile", json!({"ids": ids, "points": points}));
    }

    let join = stroke.as_ref().map(|s| s.join);
    let join_group = Rect::from_min_size(pos2(field_x, content_y + 68.0), vec2(128.0, 24.0));
    p.rect_filled(join_group, CornerRadius::same(5), raised);
    for (i, value) in StrokeJoin::ALL.iter().enumerate() {
        let x0 = join_group.left() + i as f32 * join_group.width() / 3.0;
        let x1 = join_group.left() + (i + 1) as f32 * join_group.width() / 3.0;
        let rect = Rect::from_min_max(pos2(x0, join_group.top()), pos2(x1, join_group.bottom()));
        let response = ui.interact(rect, ui.id().with(("stroke_settings_join", i)), Sense::click()).tip(join_label(*value));
        if join == Some(*value) {
            p.rect_filled(rect.shrink(2.0), CornerRadius::same(4), bg);
        } else if response.hovered() {
            p.rect_filled(rect.shrink(2.0), CornerRadius::same(4), t.hover);
        }
        let col = if join == Some(*value) { t.text } else { secondary };
        let a = rect.center() - vec2(5.0, 5.0);
        let b = rect.center() + vec2(5.0, 5.0);
        let corner = match value {
            StrokeJoin::Miter => pos2(b.x, a.y),
            StrokeJoin::Bevel => pos2(b.x - 4.0, a.y),
            StrokeJoin::Round => pos2(b.x - 2.0, a.y + 1.0),
        };
        p.line_segment([pos2(a.x, a.y), corner], Stroke::new(1.0, col));
        p.line_segment([corner, pos2(b.x, b.y)], Stroke::new(1.0, col));
        if response.clicked() {
            app.set_props(&ids, json!({"strokeJoin": value}));
        }
    }

    let angle_rect = Rect::from_min_size(pos2(field_x, content_y + 100.0), vec2(128.0, 24.0));
    let mut angle_ui = ui.new_child(egui::UiBuilder::new().max_rect(angle_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let angle = stroke.as_ref().map(|s| (s.miter_angle() * 100.0).round() / 100.0);
    let edit = widgets::number_field(&mut angle_ui, Id::new("stroke_settings_miter"), Lead::Text("∟"), angle, "°", 128.0, 1.0);
    app.field(&ids, "miterAngle", edit, |v| json!(v));
    close_requested
}

fn paint_section_header(ui: &mut Ui, title: &str, dim: bool, fills: bool) -> Option<usize> {
    let add_tip = if fills { tr("Add fill") } else { tr("Add stroke") };
    widgets::section_header(ui, title, &[(Icon::Styles, tr("Styles")), (Icon::Plus, add_tip)], dim)
}

fn property_icon_response(ui: &mut Ui, rect: Rect, icon: usize, tip: &str, active: bool, enabled: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let response = ui.interact(rect, ui.id().with(("property_icon", icon, tip)), if enabled { Sense::click() } else { Sense::hover() }).tip(tip);
    if active || (enabled && response.hovered()) {
        ui.painter().rect_filled(rect, CornerRadius::same(5), if active { t.selected_row } else { t.hover });
    }
    icons::paint_property(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(12.0, 12.0)),
        icon,
        if !enabled {
            t.icon_secondary
        } else if active {
            t.accent_text
        } else {
            t.icon
        },
    );
    response
}

/// A width profile drawn as a filled band across `rect` (thickest point = full height); `None`
/// draws a uniform stroke.
pub(crate) fn profile_art(painter: &egui::Painter, rect: Rect, profile: Option<&WidthProfile>, col: Color32) {
    const STEPS: u32 = 40;
    let max = profile.map_or(1.0, WidthProfile::max_scale).max(1e-6);
    let (cy, half) = (rect.center().y, rect.height() / 2.0);
    let mut mesh = egui::Mesh::default();
    let (mut top, mut bottom) = (Vec::new(), Vec::new());
    for i in 0..=STEPS {
        let t = f64::from(i) / f64::from(STEPS);
        // The path runs left to right: its left side is drawn on top.
        let (l, r) = profile.map_or((0.5, 0.5), |p| {
            let (l, r) = p.sides_at(t);
            (l / max, r / max)
        });
        let x = rect.min.x + rect.width() * t as f32;
        let (hl, hr) = (((l as f32) * half).max(0.5), ((r as f32) * half).max(0.5));
        top.push(pos2(x, cy - hl));
        bottom.push(pos2(x, cy + hr));
        mesh.colored_vertex(pos2(x, cy - hl), col);
        mesh.colored_vertex(pos2(x, cy + hr), col);
    }
    for i in 0..STEPS {
        let k = 2 * i;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
    // The mesh isn't anti-aliased: smooth its edges with hairlines.
    for edge in [top, bottom] {
        painter.add(egui::Shape::line(edge, Stroke::new(0.75, col)));
    }
}

/// Stroke settings ▸ Width profile: the presets (each with its shape) and a flip.
fn width_profile_row(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], profile: Option<Option<WidthProfile>>) {
    let t = Tokens::get(ui.ctx());
    widgets::sublabels(ui, tr("Width profile"), None);
    row(ui, 32.0, |ui| {
        let label = match &profile {
            None => tr("Mixed"),
            Some(None) => tr(WidthPreset::Uniform.label()),
            Some(Some(p)) => p.preset().map_or(tr("Custom"), |w| tr(w.label())),
        };
        let width = ui.available_width().max(72.0);
        let resp = crate::primitives::Select::trigger(ui, label, width, None);
        if let Some(p) = &profile {
            let r = resp.rect;
            let art = Rect::from_min_max(pos2(r.max.x - 22.0 - 56.0, r.center().y - 5.0), pos2(r.max.x - 22.0, r.center().y + 5.0));
            profile_art(ui.painter(), art, p.as_ref(), t.icon_secondary);
        }
        menu_popup(&resp).show(|ui| {
            for preset in WidthPreset::ALL {
                let prof = preset.profile();
                let art = |pt: &egui::Painter, r: Rect, col: Color32| {
                    let a = Rect::from_min_max(pos2(r.max.x - 8.0 - 64.0, r.center().y - 5.0), pos2(r.max.x - 8.0, r.center().y + 5.0));
                    profile_art(pt, a, prof.as_ref(), col);
                };
                if menu_item_with(ui, tr(preset.label()), "", 8.0 + 64.0, art) {
                    app.run("stroke.widthProfile", json!({"ids": ids, "preset": preset.id()}));
                    ui.close();
                }
            }
        });
        // Flip: the profile runs from the other end of the path.
        let custom = profile.clone().flatten();
        let flip = icons::button(ui, Icon::FlipH, 24.0, false, tr("Flip width profile"));
        if flip.clicked()
            && let Some(p) = custom
        {
            let pts: Vec<Value> =
                p.points.iter().rev().map(|q| lizard_engine::cmd::props::width_point_json(&WidthPoint { t: 1.0 - q.t, ..*q })).collect();
            app.run("stroke.widthProfile", json!({"ids": ids, "points": pts}));
        }
    });
}

/// A filled preview rasterized from the same outline used by the canvas renderer.
pub(crate) fn brush_art(painter: &egui::Painter, rect: Rect, preset: &str, col: Color32) {
    let width = rect.width().ceil().max(1.0) as u32;
    let height = rect.height().ceil().max(1.0) as u32;
    let cache_id = egui::Id::new(("brush_preview", preset, width, height));
    let cached = painter.ctx().data_mut(|data| data.get_temp::<egui::TextureHandle>(cache_id));
    let texture = if let Some(texture) = cached {
        texture
    } else {
        let Some(texture) = brush_preview_texture(painter.ctx(), preset, width, height) else { return };
        painter.ctx().data_mut(|data| data.insert_temp(cache_id, texture.clone()));
        texture
    };
    painter.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), col);
}

fn brush_preview_texture(ctx: &egui::Context, preset: &str, width: u32, height: u32) -> Option<egui::TextureHandle> {
    use tiny_skia::{FillRule, Paint as SkPaint, Pixmap, Transform};
    let mut line = lizard_geom::BezPath::new();
    line.move_to((4.0, f64::from(height) * 0.5));
    line.curve_to(
        (f64::from(width) * 0.28, f64::from(height) * 0.07),
        (f64::from(width) * 0.72, f64::from(height) * 0.93),
        (f64::from(width) - 4.0, f64::from(height) * 0.5),
    );
    let brush = Brush::new(preset);
    let weight = f64::from(height) * 0.46;
    let props = StrokeProps { weight, align: StrokeAlign::Center, brush: Some(brush.clone()), ..Default::default() };
    let out = lizard_pathops::brush::outline(&line, weight, &props, &brush)?;
    let path = lizard_render::ts_path(&out)?;
    let mut pixmap = Pixmap::new(width, height)?;
    let mut paint = SkPaint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &lizard_render::to_rgba8(&pixmap));
    Some(ctx.load_texture(format!("brush-preview-{preset}-{width}-{height}"), image, egui::TextureOptions::LINEAR))
}

/// Stroke settings ▸ Brush: a plain stroke, a stretch brush or a scatter brush, each with its look.
fn brush_row(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], brush: Option<Option<Brush>>, width: f32) {
    use lizard_pathops::brush::{PRESETS, preset};
    let t = Tokens::get(ui.ctx());
    widgets::sublabels(ui, tr("Brush"), None);
    row(ui, 32.0, |ui| {
        let label = match &brush {
            None => tr("Mixed"),
            Some(b) => b.as_ref().and_then(|b| preset(&b.preset)).map_or(tr("None"), |p| tr(p.label)),
        };
        let width = ui.available_width();
        let resp = crate::primitives::Select::trigger(ui, label, width, None);
        a11y::combo(&resp, tr("Brush"), label, a11y::menu_open(&resp));
        if let Some(Some(b)) = &brush {
            let r = resp.rect;
            brush_art(
                ui.painter(),
                Rect::from_min_max(pos2(r.max.x - 22.0 - 56.0, r.center().y - 5.0), pos2(r.max.x - 22.0, r.center().y + 5.0)),
                &b.preset,
                t.icon_secondary,
            );
        }
        menu_popup(&resp).show(|ui| {
            if menu_item(ui, tr("None"), "") {
                app.run("stroke.brush", json!({"ids": ids, "preset": null}));
                ui.close();
            }
            for p in PRESETS {
                let art = |pt: &egui::Painter, r: Rect, col: Color32| {
                    brush_art(
                        pt,
                        Rect::from_min_max(pos2(r.max.x - 8.0 - 64.0, r.center().y - 5.0), pos2(r.max.x - 8.0, r.center().y + 5.0)),
                        p.id,
                        col,
                    );
                };
                if menu_item_with(ui, tr(p.label), "", 8.0 + 64.0, art) {
                    app.run("stroke.brush", json!({"ids": ids, "preset": p.id}));
                    ui.close();
                }
            }
        });
    });
    // A scatter brush's spacing and jitter.
    let Some(Some(b)) =
        brush.filter(|b| b.as_ref().and_then(|b| preset(&b.preset)).is_some_and(|p| p.kind == lizard_pathops::brush::BrushKind::Scatter))
    else {
        return;
    };
    let half = (width - 8.0) / 2.0;
    let with = |f: &dyn Fn(&mut Brush)| {
        let mut nb = b.clone();
        f(&mut nb);
        serde_json::to_value(nb.sanitized()).unwrap_or_default()
    };
    widgets::sublabels(ui, tr("Spacing"), Some(tr("Scatter")));
    row(ui, 32.0, |ui| {
        let e = widgets::number_field(ui, Id::new("brush_spacing"), Lead::Icon(Icon::Gap, tr("Brush spacing")), Some(b.spacing), "×", half, 0.1);
        app.field(ids, "brush", e, |v| with(&|nb| nb.spacing = v));
        let e = widgets::number_field(ui, Id::new("brush_scatter"), Lead::Icon(Icon::Dots, tr("Brush scatter")), Some(b.scatter), "×", half, 0.1);
        app.field(ids, "brush", e, |v| with(&|nb| nb.scatter = v));
    });
    widgets::sublabels(ui, tr("Size jitter"), Some(tr("Rotation jitter")));
    row(ui, 32.0, |ui| {
        let e = widgets::number_field(
            ui,
            Id::new("brush_size_jitter"),
            Lead::Icon(Icon::Scale, tr("Brush size jitter")),
            Some(b.size_jitter * 100.0),
            "%",
            half,
            1.0,
        );
        app.field(ids, "brush", e, |v| with(&|nb| nb.size_jitter = v / 100.0));
        let e = widgets::number_field(
            ui,
            Id::new("brush_rotation_jitter"),
            Lead::Icon(Icon::Rotate, tr("Brush rotation jitter")),
            Some(b.rotation_jitter * 100.0),
            "%",
            half,
            1.0,
        );
        app.field(ids, "brush", e, |v| with(&|nb| nb.rotation_jitter = v / 100.0));
    });
}

/// Advanced stroke settings: dash pattern, width profile, join and the end points of open paths.
fn stroke_settings(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let ids = sel.to_vec();
    let half = widgets::col_width(ui.available_width());
    let st = common(doc, sel, |n| n.stroke.clone());
    let dashes = st.as_ref().map(|s| s.dashes.clone());
    let weight = st.as_ref().map(|s| s.weight).unwrap_or(1.0).max(1.0);
    widgets::sublabels(ui, tr("Stroke style"), None);
    row(ui, 32.0, |ui| {
        let dashed = dashes.as_ref().is_some_and(|d| !d.is_empty());
        let width = ui.available_width();
        let resp = crate::primitives::Select::trigger(ui, if dashed { tr("Dash") } else { tr("Solid") }, width, None);
        menu_popup(&resp).show(|ui| {
            if menu_item(ui, tr("Solid"), "") {
                app.set_props(&ids, json!({"dashes": []}));
                ui.close();
            }
            if menu_item(ui, tr("Dash"), "") {
                app.set_props(&ids, json!({"dashes": [weight * 4.0, weight * 4.0]}));
                ui.close();
            }
        });
    });
    if let Some(d) = dashes.filter(|d| !d.is_empty()) {
        let (dash, gap) = (d.first().copied().unwrap_or(4.0), d.get(1).copied().unwrap_or_else(|| d.first().copied().unwrap_or(4.0)));
        widgets::sublabels(ui, tr("Dash"), Some(tr("Gap")));
        row(ui, 32.0, |ui| {
            let e = widgets::number_field(ui, Id::new("stroke_dash"), Lead::None, Some(dash), "", half, 1.0);
            app.field(&ids, "dashes", e, |v| json!([v.max(0.0), gap]));
            let e = widgets::number_field(ui, Id::new("stroke_gap"), Lead::None, Some(gap), "", half, 1.0);
            app.field(&ids, "dashes", e, |v| json!([dash, v.max(0.0)]));
        });
    }
    width_profile_row(app, ui, &ids, st.as_ref().map(|s| s.profile().cloned()));
    brush_row(app, ui, &ids, st.as_ref().map(|s| s.brush.clone()), half * 2.0 + 8.0);
    let join = st.as_ref().map(|s| s.join);
    widgets::sublabels(ui, tr("Join|stroke"), (join == Some(StrokeJoin::Miter)).then_some(tr("Miter angle")));
    row(ui, 32.0, |ui| {
        let resp = crate::primitives::Select::trigger(ui, join.map(join_label).unwrap_or(tr("Mixed")), half, None);
        menu_popup(&resp).show(|ui| {
            for j in StrokeJoin::ALL {
                if menu_item(ui, join_label(j), "") {
                    app.set_props(&ids, json!({"strokeJoin": j}));
                    ui.close();
                }
            }
        });
        if join == Some(StrokeJoin::Miter) {
            let angle = st.as_ref().map(|s| (s.miter_angle() * 100.0).round() / 100.0);
            let e = widgets::number_field(ui, Id::new("stroke_miter"), Lead::None, angle, "°", half, 1.0);
            app.field(&ids, "miterAngle", e, |v| json!(v));
        }
    });
    // End points only mean something on open paths.
    let open_paths = sel.iter().all(|i| doc.node(*i).is_some_and(|n| matches!(n.kind, NodeKind::Line | NodeKind::Vector { .. })));
    if open_paths {
        widgets::sublabels(ui, tr("Start point"), Some(tr("End point")));
        row(ui, 32.0, |ui| {
            for (key, cap) in [("startCap", st.as_ref().map(|s| s.cap_start)), ("endCap", st.as_ref().map(|s| s.cap_end))] {
                let resp = crate::primitives::Select::trigger(ui, cap.map(|c| tr(c.label())).unwrap_or(tr("Mixed")), half, None);
                menu_popup(&resp).show(|ui| {
                    for c in StrokeCap::ALL {
                        if menu_item_with(ui, tr(c.label()), "", CAP_ART_W, |p, r, col| cap_art(p, r, col, c, key == "startCap")) {
                            app.set_props(&ids, json!({key: format!("{c:?}")}));
                            ui.close();
                        }
                    }
                });
            }
        });
    }
    ui.add_space(4.0);
}

/// A stroke join's name in the interface.
pub(crate) fn join_label(j: StrokeJoin) -> &'static str {
    match j {
        StrokeJoin::Miter => tr("Miter"),
        StrokeJoin::Bevel => tr("Bevel"),
        StrokeJoin::Round => tr("Round"),
    }
}

/// The width [`cap_art`] takes at the right of a menu row (its arrows reach past the line's ends).
pub(crate) const CAP_ART_W: f32 = 64.0;

/// A short stroke ending in `cap` (on its left end for a start point), drawn at the right of a menu
/// row.
pub(crate) fn cap_art(p: &egui::Painter, r: Rect, col: Color32, cap: StrokeCap, start: bool) {
    let y = r.center().y;
    let (left, right) = (r.max.x - 58.0, r.max.x - 14.0);
    // The capped end, the plain end, and the direction from the stroke into its cap.
    let (tip, tail, d) = if start { (left, right, -1.0) } else { (right, left, 1.0) };
    let at = |back: f32, dy: f32| pos2(tip - d * back, y + dy);
    let line = |to: Pos2, w: f32| {
        p.line_segment([pos2(tail, y), to], Stroke::new(w, col));
    };
    let fill = |pts: Vec<Pos2>| {
        p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
    };
    match cap {
        StrokeCap::None => line(at(0.0, 0.0), 4.0),
        StrokeCap::Round => {
            line(at(2.0, 0.0), 4.0);
            p.circle_filled(at(2.0, 0.0), 2.0, col);
        }
        StrokeCap::Square => line(at(-2.0, 0.0), 4.0),
        StrokeCap::LineArrow => {
            line(at(0.0, 0.0), 1.5);
            p.add(egui::Shape::line(vec![at(5.5, -5.5), at(0.0, 0.0), at(5.5, 5.5)], Stroke::new(1.5, col)));
        }
        StrokeCap::TriangleArrow => {
            line(at(7.0, 0.0), 1.5);
            fill(vec![at(0.0, 0.0), at(8.0, -4.5), at(8.0, 4.5)]);
        }
        StrokeCap::ReversedTriangle => {
            line(at(8.0, 0.0), 1.5);
            fill(vec![at(8.0, 0.0), at(0.0, -4.5), at(0.0, 4.5)]);
        }
        StrokeCap::CircleFilled => {
            line(at(4.0, 0.0), 1.5);
            p.circle_filled(at(4.0, 0.0), 4.0, col);
        }
        StrokeCap::DiamondFilled => {
            line(at(5.0, 0.0), 1.5);
            fill(vec![at(0.0, 0.0), at(5.0, -5.0), at(10.0, 0.0), at(5.0, 5.0)]);
        }
    }
}

/// Paint `i`'s row (colour control, visibility, remove); returns its area.
fn paint_row(app: &mut LizardApp, ui: &mut Ui, ids: &[NodeId], i: usize, p: &Paint, fills: bool, regions: Option<&[usize]>) -> Rect {
    let t = Tokens::get(ui.ctx());
    let ids = ids.to_vec();
    let cmd = if regions.is_some() {
        "vector.region.fill.set"
    } else if fills {
        "fill.set"
    } else {
        "stroke.set"
    };
    let args = |field: &str, value: Value| -> Value {
        let mut params =
            if let Some(regions) = regions { json!({"id": ids.first(), "regions": regions, "index": i}) } else { json!({"ids": ids, "index": i}) };
        params[field] = value;
        params
    };
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let r = Rect::from_min_size(pos2(row.left() + 16.0, row.top() + 4.0), vec2((row.width() - 84.0).max(64.0), 24.0));
    // The variable bound to this paint's colour (its key on the layers: `fill` or `fills/<i>/color`).
    let bound = regions
        .is_none()
        .then(|| {
            app.session.doc().ok().and_then(|d| {
                let key = ids.first().and_then(|id| d.node(*id)).and_then(|n| lizard_engine::cmd::lists::paint_binding_key(n, fills, i))?;
                Some((key.to_string(), crate::variable_ui::bound_name(d, &ids, key)?))
            })
        })
        .flatten();
    let hex = p.solid_color().map(|color| color.to_hex()).unwrap_or_default();
    let label = match &p.kind {
        PaintKind::Shader(fill) => app
            .session
            .doc()
            .ok()
            .and_then(|doc| doc.shaders.get(&fill.shader_id))
            .map(|definition| definition.name.clone())
            .unwrap_or_else(|| tr("Shader").into()),
        _ => paint_label(p),
    };
    let value = match (&p.kind, &bound) {
        (PaintKind::Solid { .. }, Some((_, name))) => crate::primitives::ColorValue::BoundVariable(name),
        (PaintKind::Solid { .. }, None) => crate::primitives::ColorValue::EditableHex(&hex),
        _ => crate::primitives::ColorValue::Label(&label),
    };
    let change = crate::primitives::ColorControl::new(Id::new(("paint_color", fills, i)), p, value, f64::from(p.opacity) * 100.0)
        .swatch_id(Id::new(("swatch", fills, i)))
        .show_at(ui, r);
    if change.open_picker {
        app.ui.picker = if let (Some(regions), Some(id)) = (regions, ids.first()) {
            Some(crate::colorpicker::PickerTarget::VectorRegion { id: *id, regions: regions.to_vec(), index: i })
        } else {
            Some(crate::colorpicker::PickerTarget::Paint { fills, index: i, ids: ids.clone() })
        };
    }
    if change.detach_variable
        && let Some((key, _)) = &bound
    {
        app.run("variable.bind", json!({"ids": ids, "field": key, "variable": null}));
    }
    if let Some(h) = change.edited_hex
        && let Some(c) = lizard_color::Color::from_hex_or_name(&h)
    {
        app.run(cmd, args("color", json!(c.to_css_hex())));
    }
    match change.opacity_edit {
        Some(FieldEdit::Set(v)) => {
            app.run(cmd, args("opacity", json!((v / 100.0).clamp(0.0, 1.0))));
        }
        Some(FieldEdit::Scrub(v, done)) => {
            let _ = app.session.begin_interaction("Opacity");
            let _ = app.session.execute(cmd, &args("opacity", json!((v / 100.0).clamp(0.0, 1.0))));
            if done {
                let _ = app.session.commit_interaction();
            }
        }
        None => {}
    }
    let eye = Rect::from_min_size(pos2(r.right() + 8.0, r.top()), vec2(24.0, 24.0));
    let eye_response = ui.interact(eye, Id::new(("paint_visibility", fills, i)), Sense::click()).tip(tr("Toggle visibility"));
    if eye_response.hovered() {
        ui.painter().rect_filled(eye, CornerRadius::same(5), t.hover);
    }
    if p.visible {
        icons::paint_appearance(ui.painter(), eye.shrink(4.0), AppearanceIcon::Visible, t.icon);
    } else {
        icons::paint(ui.painter(), Rect::from_center_size(eye.center(), vec2(14.0, 14.0)), Icon::EyeOff, t.icon);
    }
    if eye_response.clicked() {
        app.run(cmd, args("visible", json!(!p.visible)));
    }
    let remove = Rect::from_min_size(pos2(row.right() - 32.0, r.top()), vec2(24.0, 24.0));
    let remove_response = ui.interact(remove, Id::new(("paint_remove", fills, i)), Sense::click()).tip(tr("Remove"));
    if remove_response.hovered() {
        ui.painter().rect_filled(remove, CornerRadius::same(5), t.hover);
    }
    icons::paint_colorpicker(ui.painter(), Rect::from_center_size(remove.center(), vec2(12.0, 1.0)), 13, t.icon);
    if remove_response.clicked() {
        let payload = args("index", json!(i));
        if app
            .run(
                if regions.is_some() {
                    "vector.region.fill.remove"
                } else if fills {
                    "fill.remove"
                } else {
                    "stroke.remove"
                },
                payload,
            )
            .is_some()
        {
            let index = match &mut app.ui.picker {
                Some(crate::colorpicker::PickerTarget::VectorRegion { index, .. }) if regions.is_some() => Some(index),
                Some(crate::colorpicker::PickerTarget::Paint { fills: f, index, .. }) if regions.is_none() && *f == fills => Some(index),
                _ => None,
            };
            if let Some(index) = index {
                match removed_index(*index, i) {
                    Some(next) => *index = next,
                    None => app.ui.picker = None,
                }
            }
        }
    }
    row
}

/// A paint row's label (also what a drag of it carries): its colour, gradient kind or "Image".
fn paint_label(p: &Paint) -> String {
    match &p.kind {
        PaintKind::Solid { color } => color.to_hex(),
        // The gradient kind's name ("Linear") is a key of its own.
        PaintKind::Gradient(g) => tr(&format!("{:?}", g.kind)).to_string(),
        PaintKind::Image(_) => tr("Image").into(),
        PaintKind::Pattern(_) => tr("Pattern").into(),
        PaintKind::Shader(_) => tr("Shader").into(),
    }
}

fn effects(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let ids = sel.to_vec();
    let list = common(doc, sel, |n| n.effects.clone());
    let empty = list.as_ref().is_none_or(|l| l.is_empty());
    let (clicked, actions) =
        widgets::section_header_actions(ui, tr("Effects"), &[(Icon::Styles, tr("Styles")), (Icon::Plus, tr("Add effect"))], empty);
    if clicked == Some(0) {
        style_picker::open(app, ui, &ids, StyleSlot::Effect);
    }
    if let Some((_, add)) = actions.iter().find(|(index, _)| *index == 1) {
        crate::primitives::Menu::popup(add).width(216.0).show(|ui| {
            for (label, effect_type) in Effect::TYPES {
                let Some(effect) = Effect::of_type(effect_type) else { continue };
                if crate::primitives::MenuItem::new(tr(label))
                    .show_with_parts(
                        ui,
                        28.0,
                        |painter, rect, color| paint_effect_kind_icon(painter, rect.center(), &effect.kind, color),
                        0.0,
                        |_, _, _| {},
                    )
                    .clicked()
                {
                    app.run("effect.add", json!({"ids": ids, "type": effect_type}));
                    ui.close();
                }
            }
        });
    }
    let before_chip = ui.cursor().min.y;
    style_picker::chip(app, ui, doc, &ids, StyleSlot::Effect);
    let has_chip = ui.cursor().min.y > before_chip;
    let Some(list) = list else {
        if has_chip {
            ui.add_space(12.0);
        }
        return;
    };
    ui.data_mut(|data| data.insert_temp(Id::new("fx_open_count"), list.len()));
    if list.is_empty() {
        if has_chip {
            ui.add_space(12.0);
        }
        return;
    }
    let mut drag = ListDrag::new(ui, "effects");
    let mut removed = None;
    for (i, e) in list.iter().enumerate().rev() {
        let mut open = effect_open(ui.ctx(), i);
        let row = effect_row(ui, Id::new(("effect_row", i)), e, open);
        if row.settings {
            open = !open;
            for j in 0..list.len() {
                ui.data_mut(|data| data.insert_temp(Id::new((FX_OPEN, j)), j == i && open));
            }
            ui.data_mut(|data| data.insert_temp(Id::new((FX_OPEN, "anchor")), pos2(row.rect.min.x - 248.0, row.rect.min.y)));
        }
        if row.visibility {
            let mut changed = e.clone();
            changed.visible = !changed.visible;
            app.run("effect.set", json!({"ids": ids, "index": i, "effect": changed}));
        }
        if row.remove && app.run("effect.remove", json!({"ids": ids, "index": i})).is_some() {
            removed = Some(i);
        }
        if open {
            effect_popup(app, ui.ctx(), &ids, i, e);
        }
        drag.row(ui, i, row.rect, row.rect.with_max_y(ui.cursor().min.y.max(row.rect.max.y)), || tr(e.label()).to_string());
    }
    if let Some((from, to)) = drag.finish(ui) {
        app.run("effect.move", json!({"ids": ids, "from": from, "to": to}));
        move_flags(ui, FX_OPEN, list.len(), from, to);
        if let Some(crate::colorpicker::PickerTarget::Effect { index, .. }) = &mut app.ui.picker {
            *index = moved_index(*index, from, to);
        }
    }
    // Open settings and an open colour picker stay with their effects, or go with theirs.
    if let Some(i) = removed {
        remove_flags(ui, FX_OPEN, list.len(), i);
        if let Some(crate::colorpicker::PickerTarget::Effect { index, .. }) = &mut app.ui.picker {
            match removed_index(*index, i) {
                Some(k) => *index = k,
                None => app.ui.picker = None,
            }
        }
    }
    ui.add_space(12.0);
}

/// Salt of the flags that keep effect rows' settings open.
const FX_OPEN: &str = "fx_open";

/// Clear settings tied to the previous layer selection.
pub(crate) fn dismiss_selection_panels(ctx: &egui::Context) {
    let effect_count = ctx.data(|data| data.get_temp::<usize>(Id::new("fx_open_count"))).unwrap_or(0);
    ctx.data_mut(|data| {
        for key in ["corner_smoothing_open", "blend_open", "indep_corners"] {
            data.insert_temp(Id::new(key), false);
        }
        for index in 0..effect_count {
            data.insert_temp(Id::new((FX_OPEN, index)), false);
        }
        data.insert_temp(Id::new("fx_open_count"), 0usize);
    });
}

/// Whether effect `i`'s settings are open in the Design panel.
pub(crate) fn effect_open(ctx: &egui::Context, i: usize) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new((FX_OPEN, i)))).unwrap_or(false)
}

fn effect_panel_id(i: usize) -> Id {
    Id::new(("effect_settings_panel", i))
}

pub(crate) struct EffectRow {
    pub rect: Rect,
    pub settings: bool,
    pub visibility: bool,
    pub remove: bool,
}

fn paint_effect_kind_icon(painter: &egui::Painter, center: egui::Pos2, kind: &EffectKind, color: Color32) {
    let (index, size) = match kind {
        EffectKind::Glass(_) => (3, 14.0),
        EffectKind::Texture(_) => (7, 11.0),
        EffectKind::Noise(_) => (11, 12.0),
        EffectKind::BackgroundBlur(_) => (15, 12.0),
        EffectKind::LayerBlur(_) => (19, 12.0),
        EffectKind::DropShadow(_) => (23, 12.0),
        EffectKind::InnerShadow(_) => (28, 12.0),
    };
    icons::paint_panel_reference(painter, Rect::from_center_size(center, vec2(size, size)), index, color);
    match kind {
        EffectKind::DropShadow(_) => icons::paint_panel_reference(
            painter,
            Rect::from_center_size(center + vec2(0.0, 6.5), vec2(12.0, 5.0)),
            24,
            Color32::from_white_alpha(102),
        ),
        EffectKind::InnerShadow(_) => icons::paint_panel_reference(
            painter,
            Rect::from_center_size(center + vec2(0.0, -3.0), vec2(10.0, 4.0)),
            29,
            Color32::from_white_alpha(102),
        ),
        _ => {}
    }
}

/// One effect-list row, shared by the properties panel and effect-style editor.
pub(crate) fn effect_row(ui: &mut Ui, id: Id, effect: &Effect, open: bool) -> EffectRow {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let drag_rect = Rect::from_min_size(row.min, vec2(16.0, 32.0));
    let button_rect = Rect::from_min_size(row.min + vec2(16.0, 3.0), vec2((row.width() - 84.0).max(70.0), 26.0));
    let eye_rect = Rect::from_min_size(pos2(row.max.x - 60.0, row.min.y + 4.0), vec2(24.0, 24.0));
    let remove_rect = Rect::from_min_size(pos2(row.max.x - 32.0, row.min.y + 4.0), vec2(24.0, 24.0));
    let kind_icon = match effect.kind {
        EffectKind::Glass(_) => 3,
        EffectKind::Texture(_) => 7,
        EffectKind::Noise(_) => 11,
        EffectKind::BackgroundBlur(_) => 15,
        EffectKind::LayerBlur(_) => 19,
        EffectKind::DropShadow(_) => 23,
        EffectKind::InnerShadow(_) => 28,
    };
    let grip_icon = match kind_icon {
        3 => 2,
        7 => 6,
        11 => 10,
        15 => 14,
        19 => 18,
        23 => 22,
        _ => 27,
    };
    icons::paint_panel_reference(ui.painter(), Rect::from_center_size(drag_rect.center(), vec2(6.0, 7.0)), grip_icon, Color32::from_white_alpha(100));
    let settings = ui.interact(button_rect, id.with("settings"), Sense::click()).tip(tr("Effect settings"));
    if settings.hovered() || open {
        ui.painter().rect_filled(button_rect, CornerRadius::same(5), Tokens::get(ui.ctx()).hover);
    }
    ui.painter().rect_stroke(button_rect, CornerRadius::same(5), Stroke::new(1.0, Color32::from_white_alpha(26)), egui::StrokeKind::Inside);
    let icon_center = pos2(button_rect.min.x + 20.0, button_rect.center().y);
    paint_effect_kind_icon(ui.painter(), icon_center, &effect.kind, Tokens::get(ui.ctx()).text_secondary);
    ui.painter().text(
        pos2(button_rect.min.x + 32.0, button_rect.center().y),
        Align2::LEFT_CENTER,
        tr(effect.label()),
        regular(11.0),
        Tokens::get(ui.ctx()).text,
    );
    let eye = ui.interact(eye_rect, id.with("visibility"), Sense::click()).tip(tr("Toggle"));
    icons::paint_panel_reference(
        ui.painter(),
        Rect::from_center_size(eye_rect.center(), vec2(14.0, 10.0)),
        4,
        if effect.visible { Tokens::get(ui.ctx()).text_secondary } else { Color32::from_white_alpha(70) },
    );
    let remove = ui.interact(remove_rect, id.with("remove"), Sense::click()).tip(tr("Remove"));
    icons::paint_panel_reference(
        ui.painter(),
        Rect::from_center_size(remove_rect.center(), vec2(12.0, 1.0)),
        5,
        Tokens::get(ui.ctx()).text_secondary,
    );
    EffectRow { rect: row, settings: settings.clicked(), visibility: eye.clicked(), remove: remove.clicked() }
}

enum EffectTarget<'a> {
    Layers(&'a [NodeId]),
    Style { id: &'a str, effects: &'a [Effect] },
}

impl EffectTarget<'_> {
    fn set(&self, app: &mut LizardApp, index: usize, effect: Effect, scrub: Option<bool>) {
        let (command, params) = match self {
            Self::Layers(ids) => ("effect.set", json!({"ids": ids, "index": index, "effect": effect})),
            Self::Style { id, effects } => {
                let mut updated = effects.to_vec();
                if let Some(slot) = updated.get_mut(index) {
                    *slot = effect;
                }
                ("style.update", json!({"style": id, "value": updated}))
            }
        };
        match scrub {
            None => {
                app.run(command, params);
            }
            Some(done) => {
                let _ = app.session.begin_interaction("Effect");
                let _ = app.session.execute(command, &params);
                if done {
                    let _ = app.session.commit_interaction();
                }
            }
        }
    }

    fn picker(&self, index: usize, secondary: bool) -> crate::colorpicker::PickerTarget {
        match self {
            Self::Layers(ids) => crate::colorpicker::PickerTarget::Effect { ids: ids.to_vec(), index, secondary },
            Self::Style { id, .. } => crate::colorpicker::PickerTarget::StyleEffect { id: (*id).to_string(), index, secondary },
        }
    }
}

fn effect_popup(app: &mut LizardApp, ctx: &egui::Context, ids: &[NodeId], i: usize, e: &Effect) {
    let anchor = ctx.data(|d| d.get_temp::<Pos2>(Id::new((FX_OPEN, "anchor")))).unwrap_or(pos2(100.0, 140.0));
    effect_popup_target(app, ctx, EffectTarget::Layers(ids), i, e, anchor, effect_panel_id(i), Id::new((FX_OPEN, i)));
}

pub(crate) fn effect_style_popup(app: &mut LizardApp, ctx: &egui::Context, id: &str, effects: &[Effect], index: usize, effect: &Effect, at: Pos2) {
    effect_popup_target(
        app,
        ctx,
        EffectTarget::Style { id, effects },
        index,
        effect,
        at,
        Id::new(("effect_style_settings", id)),
        Id::new(("effect_style_open", id)),
    );
}

fn effect_popup_target(
    app: &mut LizardApp,
    ctx: &egui::Context,
    target: EffectTarget<'_>,
    i: usize,
    e: &Effect,
    anchor: Pos2,
    panel_id: Id,
    open_id: Id,
) {
    let height = match e.kind {
        EffectKind::Glass(_) => 313.0,
        EffectKind::Texture(_) | EffectKind::InnerShadow(_) => 224.0,
        EffectKind::Noise(ref n) => {
            if n.mode == NoiseMode::Duo {
                256.0
            } else {
                224.0
            }
        }
        EffectKind::DropShadow(_) => 285.0,
        EffectKind::LayerBlur(ref b) | EffectKind::BackgroundBlur(ref b) => {
            if b.is_progressive() {
                156.0
            } else {
                124.0
            }
        }
    };
    let screen = ctx.content_rect();
    let at = pos2(
        anchor.x.clamp(screen.min.x + 8.0, (screen.max.x - 248.0).max(screen.min.x + 8.0)),
        anchor.y.clamp(screen.min.y + 8.0, (screen.max.y - height - 8.0).max(screen.min.y + 8.0)),
    );
    let mut keep = true;
    crate::primitives::FloatingPanel::new(panel_id, at)
        .height(crate::primitives::FloatingPanelHeight::Fixed(height))
        .open(&mut keep)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.set_width(240.0);
            ui.painter().hline(header.min.x..=header.max.x, header.max.y - 0.5, Stroke::new(1.0, Color32::from_rgb(68, 68, 68)));
            let icon = match e.kind {
                EffectKind::Glass(_) => 70,
                EffectKind::Texture(_) => 50,
                EffectKind::Noise(_) => 54,
                EffectKind::BackgroundBlur(_) | EffectKind::LayerBlur(_) => 66,
                EffectKind::DropShadow(_) => 32,
                EffectKind::InnerShadow(_) => 59,
            };
            let icon_center = pos2(header.min.x + 20.0, header.center().y);
            icons::paint_panel_reference(
                ui.painter(),
                Rect::from_center_size(
                    icon_center,
                    if matches!(e.kind, EffectKind::DropShadow(_) | EffectKind::InnerShadow(_)) { vec2(12.0, 12.0) } else { vec2(14.0, 14.0) },
                ),
                icon,
                Color32::WHITE,
            );
            match e.kind {
                EffectKind::DropShadow(_) => icons::paint_panel_reference(
                    ui.painter(),
                    Rect::from_center_size(icon_center + vec2(0.0, 6.5), vec2(12.0, 5.0)),
                    33,
                    Color32::from_white_alpha(102),
                ),
                EffectKind::InnerShadow(_) => icons::paint_panel_reference(
                    ui.painter(),
                    Rect::from_center_size(icon_center + vec2(0.0, -3.0), vec2(10.0, 4.0)),
                    60,
                    Color32::from_white_alpha(102),
                ),
                _ => {}
            }
            let title = tr(e.label());
            let type_area = Rect::from_min_size(header.min + vec2(8.0, 8.0), vec2(176.0, 24.0));
            let type_response = ui.interact(type_area, Id::new(("effect_popup_type", i)), Sense::click()).tip(tr("Effect type"));
            ui.painter().text(pos2(header.min.x + 36.0, header.center().y), Align2::LEFT_CENTER, title, semibold(11.0), Color32::WHITE);
            let label_width = ui.painter().layout_no_wrap(title.to_string(), semibold(11.0), Color32::WHITE).size().x;
            icons::paint_panel_reference(
                ui.painter(),
                Rect::from_center_size(pos2((header.min.x + 42.0 + label_width).min(header.max.x - 68.0), header.center().y), vec2(5.0, 3.0)),
                34,
                Color32::from_white_alpha(178),
            );
            menu_popup(&type_response).show(|ui| {
                for (label, ty) in Effect::TYPES {
                    if menu_item(ui, tr(label), "")
                        && let Some(next) = Effect::of_type(ty)
                    {
                        target.set(app, i, next, None);
                        ui.close();
                    }
                }
            });
            if e.color(false).is_some() {
                let color = Rect::from_min_size(pos2(header.max.x - 56.0, header.min.y + 8.0), vec2(24.0, 24.0));
                if ui.interact(color, Id::new(("effect_popup_color", i)), Sense::click()).tip(tr("Effect color")).clicked() {
                    app.ui.picker = Some(target.picker(i, false));
                }
                icons::paint_panel_reference(ui.painter(), Rect::from_center_size(color.center(), vec2(14.0, 14.0)), 35, Color32::WHITE);
            }
            let close = Rect::from_min_size(pos2(header.max.x - 32.0, header.min.y + 8.0), vec2(24.0, 24.0));
            if crate::primitives::FloatingPanel::close_button(ui, Id::new(("effect_popup_close", i)), close, |painter, rect| {
                icons::paint_panel_reference(painter, Rect::from_center_size(rect.center(), vec2(10.0, 10.0)), 36, Color32::WHITE);
            }) {
                ui.data_mut(|d| d.remove::<usize>(open_id));
                ui.data_mut(|d| d.insert_temp(open_id, false));
            }
            effect_settings(app, ui, &target, i, e);
        });
    if !keep {
        ctx.data_mut(|d| d.remove::<usize>(open_id));
        ctx.data_mut(|d| d.insert_temp(open_id, false));
    }
}

/// A number field of effect `i` that can be bound to a variable (`effects/<i>/<prop>`).
fn fx_field(
    app: &mut LizardApp,
    ui: &mut Ui,
    target: &EffectTarget<'_>,
    i: usize,
    prop: &str,
    lead: Lead,
    value: f64,
    width: f32,
) -> Option<FieldEdit> {
    match target {
        EffectTarget::Layers(ids) => {
            crate::variable_ui::number_field(app, ui, ids, &format!("effects/{i}/{prop}"), Id::new(("fx", prop, i)), lead, Some(value), "", width)
        }
        EffectTarget::Style { .. } => widgets::number_field(ui, Id::new(("fx_style", prop, i)), lead, Some(value), "", width, 1.0),
    }
}

fn fx_icon_field(app: &mut LizardApp, ui: &mut Ui, target: &EffectTarget<'_>, i: usize, prop: &str, value: f64, icon: usize) -> Option<FieldEdit> {
    let origin = ui.cursor().min;
    let result = fx_field(app, ui, target, i, prop, Lead::Text(""), value, 136.0);
    let size = match icon {
        37 | 38 | 64 | 65 | 69 => vec2(11.0, 11.0),
        _ => vec2(12.0, 12.0),
    };
    // The field is vertically centred in a 32-point row. `ui.cursor().min` is the row's
    // top edge, so centre the artwork on the row rather than 12 points below that edge.
    let icon_center = pos2(origin.x + 12.0, ui.max_rect().center().y);
    icons::paint_panel_reference(ui.painter(), Rect::from_center_size(icon_center, size), icon, Color32::from_white_alpha(178));
    result
}

fn fx_popup_label(ui: &mut Ui, text: &str) {
    ui.add_space(16.0);
    let (r, _) = ui.allocate_exact_size(vec2(72.0, 24.0), Sense::hover());
    ui.painter().text(r.left_center(), Align2::LEFT_CENTER, text, regular(11.0), Color32::from_white_alpha(178));
}

fn fx_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let (r, _) = ui.allocate_exact_size(vec2(240.0, 32.0), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    add(&mut child);
}

/// The full-width segmented control used for blur and noise modes in the reference popup.
fn fx_mode_selector(ui: &mut Ui, labels: &[&str], selected: usize, id: Id) -> Option<usize> {
    if labels.is_empty() {
        return None;
    }
    let (row, _) = ui.allocate_exact_size(vec2(240.0, 32.0), Sense::hover());
    let bar = Rect::from_min_size(row.min + vec2(16.0, 4.0), vec2(208.0, 24.0));
    ui.painter().rect_filled(bar, CornerRadius::same(5), Color32::from_rgb(56, 56, 56));
    let mut clicked = None;
    let segment_width = bar.width() / labels.len() as f32;
    for (index, label) in labels.iter().enumerate() {
        let segment = Rect::from_min_size(bar.min + vec2(segment_width * index as f32, 0.0), vec2(segment_width, 24.0));
        if selected == index {
            ui.painter().rect_filled(segment, CornerRadius::same(5), Color32::from_rgb(44, 44, 44));
            ui.painter().rect_stroke(segment, CornerRadius::same(5), Stroke::new(2.0, Color32::from_rgb(56, 56, 56)), egui::StrokeKind::Inside);
        }
        ui.painter().text(
            segment.center(),
            Align2::CENTER_CENTER,
            *label,
            regular(11.0),
            if selected == index { Color32::WHITE } else { Color32::from_white_alpha(178) },
        );
        if ui.interact(segment, id.with(index), Sense::click()).clicked() {
            clicked = Some(index);
        }
    }
    clicked
}

fn fx_glass_slider(
    app: &mut LizardApp,
    ui: &mut Ui,
    target: &EffectTarget<'_>,
    i: usize,
    label: &str,
    key: &str,
    value: f64,
    max: f64,
    effect: impl Fn(f64) -> Effect,
) {
    let (row, _) = ui.allocate_exact_size(vec2(240.0, 32.0), Sense::hover());
    ui.painter().text(row.min + vec2(16.0, 16.0), Align2::LEFT_CENTER, label, regular(11.0), Tokens::get(ui.ctx()).text_secondary);
    let controls = Rect::from_min_size(row.min + vec2(88.0, 4.0), vec2(136.0, 24.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(controls).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 0.0;
    let slider = Slider::new(Id::new(("fx_glass_slider", key, i)), label)
        .range(0.0, max)
        .step(1.0)
        .value(Some(value.clamp(0.0, max)))
        .text(false)
        .width(ControlWidth::Fixed(80.0))
        .show(&mut child);
    if let Some((next, done)) = slider.change {
        if (next - value).abs() > f64::EPSILON {
            target.set(app, i, effect(next), Some(done));
        } else if done {
            let _ = app.session.commit_interaction();
        }
    }
    let edit = NumberField::new(Id::new(("fx_glass", key, i)), Lead::None, Some(value.round()), "")
        .show(&mut child, ControlWidth::Fixed(56.0))
        .edit
        .and_then(Edit::settled);
    effect_target_field(app, target, i, edit, |v| effect(v.max(0.0)));
}

fn effect_target_field(app: &mut LizardApp, target: &EffectTarget<'_>, index: usize, edit: Option<FieldEdit>, effect: impl Fn(f64) -> Effect) {
    match edit {
        Some(FieldEdit::Set(value)) => target.set(app, index, effect(value), None),
        Some(FieldEdit::Scrub(value, done)) => target.set(app, index, effect(value), Some(done)),
        None => {}
    }
}

fn effect_settings(app: &mut LizardApp, ui: &mut Ui, target: &EffectTarget<'_>, i: usize, e: &Effect) {
    let set = |app: &mut LizardApp, e2: Effect, scrub: Option<bool>| target.set(app, i, e2, scrub);
    let edit = |app: &mut LizardApp, ed: Option<FieldEdit>, f: &dyn Fn(f64) -> Effect| match ed {
        Some(FieldEdit::Set(v)) => set(app, f(v), None),
        Some(FieldEdit::Scrub(v, d)) => set(app, f(v), Some(d)),
        None => {}
    };
    // A new colour for the effect (`secondary`: a duo noise's second colour).
    let edit_color = |app: &mut LizardApp, secondary: bool, c: lizard_color::Color, scrub: Option<bool>| {
        let mut e2 = e.clone();
        if let Some(slot) = e2.color_mut(secondary) {
            *slot = c;
        }
        set(app, e2, scrub);
    };
    match e.kind {
        EffectKind::Glass(_) => {}
        EffectKind::LayerBlur(_) | EffectKind::BackgroundBlur(_) | EffectKind::Noise(_) => ui.add_space(8.0),
        _ => ui.add_space(12.0),
    }
    match &e.kind {
        EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => {
            let inner = matches!(e.kind, EffectKind::InnerShadow(_));
            let mk = |s2: Shadow| Effect { kind: if inner { EffectKind::InnerShadow(s2) } else { EffectKind::DropShadow(s2) }, visible: e.visible };
            fx_row(ui, |ui| {
                fx_popup_label(ui, tr("Alignment"));
                let ed = fx_field(app, ui, target, i, "offsetX", Lead::Text("X"), s.offset.0, 136.0);
                edit(app, ed, &|v| mk(Shadow { offset: (v, s.offset.1), ..s.clone() }));
            });
            fx_row(ui, |ui| {
                fx_popup_label(ui, "");
                let ed = fx_field(app, ui, target, i, "offsetY", Lead::Text("Y"), s.offset.1, 136.0);
                edit(app, ed, &|v| mk(Shadow { offset: (s.offset.0, v), ..s.clone() }));
            });
            fx_row(ui, |ui| {
                fx_popup_label(ui, tr("Blur"));
                let ed = fx_icon_field(app, ui, target, i, "radius", s.radius, 37);
                edit(app, ed, &|v| mk(Shadow { radius: v.max(0.0), ..s.clone() }));
            });
            fx_row(ui, |ui| {
                fx_popup_label(ui, tr("Spread"));
                let ed = fx_icon_field(app, ui, target, i, "spread", s.spread, 38);
                edit(app, ed, &|v| mk(Shadow { spread: v, ..s.clone() }));
            });
            // The colour: a bound variable's chip, else swatch, hex and alpha with a variable button.
            fx_row(ui, |ui| {
                fx_popup_label(ui, tr("Color"));
                match target {
                    EffectTarget::Layers(ids) => {
                        let color_field = format!("effects/{i}/color");
                        crate::variable_ui::bindable(app, ui, ids, &color_field, VariableType::Color, 136.0, |app, ui| {
                            fx_color(app, ui, target, i, e, false, &edit_color)
                        });
                    }
                    EffectTarget::Style { .. } => fx_color(app, ui, target, i, e, false, &edit_color),
                }
            });
            if !inner {
                ui.add_space(12.0);
                ui.separator();
                block(ui, 60.0, 12.0, |ui| {
                    if widgets::checkbox(ui, s.show_behind, tr("Show behind transparent areas")) {
                        set(app, mk(Shadow { show_behind: !s.show_behind, ..s.clone() }), None);
                    }
                });
            }
        }
        EffectKind::LayerBlur(b) | EffectKind::BackgroundBlur(b) => {
            let with = |f: &dyn Fn(&mut Blur)| {
                let mut e2 = e.clone();
                if let Some(b2) = e2.blur_mut() {
                    f(b2);
                }
                e2
            };
            let modes = BlurType::ALL;
            let labels: Vec<&str> = modes.iter().map(|t| tr(t.label())).collect();
            let current = modes.iter().position(|t| *t == b.blur_type).unwrap_or(0);
            if let Some(index) = fx_mode_selector(ui, &labels, current, Id::new(("fx_blur_mode", i)))
                && let Some(mode) = modes.get(index)
            {
                set(app, with(&|b2| b2.blur_type = *mode), None);
            }
            if b.is_progressive() {
                fx_row(ui, |ui| {
                    fx_popup_label(ui, tr("Start"));
                    let ed = fx_field(app, ui, target, i, "startRadius", Lead::None, b.start_radius, 136.0);
                    edit(app, ed, &|v| with(&|b2| b2.start_radius = v.clamp(0.0, MAX_BLUR_RADIUS)));
                });
                fx_row(ui, |ui| {
                    fx_popup_label(ui, tr("End"));
                    let ed = fx_field(app, ui, target, i, "radius", Lead::None, b.radius, 136.0);
                    edit(app, ed, &|v| with(&|b2| b2.radius = v.clamp(0.0, MAX_BLUR_RADIUS)));
                });
            } else {
                fx_row(ui, |ui| {
                    fx_popup_label(ui, tr("Blur"));
                    let ed = fx_icon_field(app, ui, target, i, "radius", b.radius, 69);
                    edit(app, ed, &|v| with(&|b2| b2.radius = v.clamp(0.0, MAX_BLUR_RADIUS)));
                });
            }
        }
        EffectKind::Noise(nz) => {
            let with = |f: &dyn Fn(&mut Noise)| {
                tweaked(e, |k| {
                    if let EffectKind::Noise(n) = k {
                        f(n);
                    }
                })
            };
            let modes = NoiseMode::ALL;
            let labels: Vec<&str> = modes.iter().map(|m| tr(m.label())).collect();
            let current = modes.iter().position(|m| *m == nz.mode).unwrap_or(0);
            if let Some(index) = fx_mode_selector(ui, &labels, current, Id::new(("fx_noise_mode", i)))
                && let Some(mode) = modes.get(index)
            {
                set(app, with(&|n| n.mode = *mode), None);
            }
            ui.add_space(4.0);
            fx_row(ui, |ui| {
                fx_popup_label(ui, tr("Size"));
                let ed = widgets::number_field(ui, Id::new(("fx_ns", i)), Lead::Text("X"), Some(nz.size), "", 136.0, 0.1);
                edit(app, ed, &|v| with(&|n| n.size = v.clamp(0.01, 1000.0)));
            });
            fx_row(ui, |ui| {
                fx_popup_label(ui, "");
                let ed = widgets::number_field(ui, Id::new(("fx_nsy", i)), Lead::Text("Y"), Some(nz.size_y.unwrap_or(nz.size)), "", 136.0, 0.1);
                edit(app, ed, &|v| with(&|n| n.size_y = Some(v.clamp(0.01, 1000.0))));
            });
            fx_row(ui, |ui| {
                fx_popup_label(ui, tr("Density"));
                let ed = widgets::number_field(ui, Id::new(("fx_nd", i)), Lead::Text(""), Some((nz.density * 100.0).round()), "%", 136.0, 1.0);
                let icon_center = ui.min_rect().min + vec2(100.0, 16.0);
                icons::paint_panel_reference(ui.painter(), Rect::from_center_size(icon_center, vec2(12.0, 12.0)), 58, Color32::from_white_alpha(178));
                edit(app, ed, &|v| with(&|n| n.density = (v / 100.0).clamp(0.0, 1.0)));
            });
            match nz.mode {
                NoiseMode::Mono => fx_row(ui, |ui| {
                    fx_popup_label(ui, tr("Color"));
                    fx_color(app, ui, target, i, e, false, &edit_color);
                }),
                NoiseMode::Duo => {
                    fx_row(ui, |ui| {
                        fx_popup_label(ui, tr("Color"));
                        fx_color(app, ui, target, i, e, false, &edit_color);
                    });
                    fx_row(ui, |ui| {
                        fx_popup_label(ui, tr("Color 2"));
                        fx_color(app, ui, target, i, e, true, &edit_color);
                    });
                }
                NoiseMode::Multi => fx_row(ui, |ui| {
                    fx_popup_label(ui, tr("Opacity"));
                    let op = Some((f64::from(nz.opacity) * 100.0).round());
                    let ed = widgets::number_field(ui, Id::new(("fx_no", i)), Lead::None, op, "%", 136.0, 1.0);
                    edit(app, ed, &|v| with(&|n| n.opacity = (v / 100.0).clamp(0.0, 1.0) as f32));
                }),
            }
        }
        EffectKind::Texture(tx) => {
            let with = |f: &dyn Fn(&mut Texture)| {
                tweaked(e, |k| {
                    if let EffectKind::Texture(t) = k {
                        f(t);
                    }
                })
            };
            fx_row(ui, |ui| {
                fx_popup_label(ui, tr("Size"));
                let ed = widgets::number_field(ui, Id::new(("fx_ts", i)), Lead::Text("X"), Some(tx.size), "", 136.0, 0.1);
                edit(app, ed, &|v| with(&|t| t.size = v.clamp(0.01, 1000.0)));
            });
            fx_row(ui, |ui| {
                fx_popup_label(ui, "");
                let ed = widgets::number_field(ui, Id::new(("fx_tsy", i)), Lead::Text("Y"), Some(tx.size_y.unwrap_or(tx.size)), "", 136.0, 0.1);
                edit(app, ed, &|v| with(&|t| t.size_y = Some(v.clamp(0.01, 1000.0))));
            });
            fx_row(ui, |ui| {
                fx_popup_label(ui, tr("Radius"));
                let ed = widgets::number_field(ui, Id::new(("fx_tr", i)), Lead::Text(""), Some(tx.radius), "", 136.0, 1.0);
                let icon_center = ui.min_rect().min + vec2(100.0, 16.0);
                icons::paint_panel_reference(ui.painter(), Rect::from_center_size(icon_center, vec2(14.0, 14.0)), 53, Color32::from_white_alpha(178));
                edit(app, ed, &|v| with(&|t| t.radius = v.max(0.0)));
            });
            ui.add_space(24.0);
            let (footer, _) = ui.allocate_exact_size(vec2(240.0, 48.0), Sense::hover());
            ui.painter().hline(footer.min.x..=footer.max.x, footer.min.y + 0.5, Stroke::new(1.0, Color32::from_rgb(68, 68, 68)));
            let mut child = ui
                .new_child(egui::UiBuilder::new().max_rect(footer.shrink2(vec2(16.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            if widgets::checkbox(&mut child, tx.clip_to_shape, tr("Clip to shape")) {
                set(app, with(&|t| t.clip_to_shape = !tx.clip_to_shape), None);
            }
        }
        EffectKind::Glass(g) => {
            let with = |f: &dyn Fn(&mut Glass)| {
                tweaked(e, |k| {
                    if let EffectKind::Glass(gl) = k {
                        f(gl);
                    }
                })
            };
            let (light, _) = ui.allocate_exact_size(vec2(240.0, 88.0), Sense::hover());
            ui.painter().text(
                pos2(light.min.x + 16.0, light.min.y + 44.0),
                Align2::LEFT_CENTER,
                tr("Light|glass"),
                regular(11.0),
                Color32::from_white_alpha(178),
            );
            let preview = Rect::from_min_size(light.min + vec2(88.0, 16.0), vec2(56.0, 56.0));
            let light_control = GlassLightControl::new(Id::new(("fx_light_preview", i)), g.light_angle, g.light_intensity).show(ui, preview);
            if let Some(angle) = light_control.angle
                && (angle - g.light_angle).abs() > 0.5
            {
                target.set(app, i, with(&|gl| gl.light_angle = angle), if light_control.response.clicked() { None } else { Some(false) });
            }
            if light_control.response.drag_stopped() {
                let _ = app.session.commit_interaction();
            }
            let angle = Rect::from_min_size(light.min + vec2(152.0, 16.0), vec2(72.0, 24.0));
            let intensity = Rect::from_min_size(light.min + vec2(152.0, 47.0), vec2(72.0, 24.0));
            let angle_edit = ui
                .scope_builder(egui::UiBuilder::new().max_rect(angle), |ui| {
                    NumberField::new(Id::new(("fx_gl", i)), Lead::None, Some(g.light_angle.round()), "°")
                        .show(ui, ControlWidth::Fixed(72.0))
                        .edit
                        .and_then(Edit::settled)
                })
                .inner;
            effect_target_field(app, target, i, angle_edit, |v| with(&|gl| gl.light_angle = v.clamp(-180.0, 180.0)));
            let intensity_edit = ui
                .scope_builder(egui::UiBuilder::new().max_rect(intensity), |ui| {
                    NumberField::new(Id::new(("fx_gi", i)), Lead::None, Some((g.light_intensity * 100.0).round()), "%")
                        .show(ui, ControlWidth::Fixed(72.0))
                        .edit
                        .and_then(Edit::settled)
                })
                .inner;
            effect_target_field(app, target, i, intensity_edit, |v| with(&|gl| gl.light_intensity = (v / 100.0).clamp(0.0, 1.0)));
            ui.add_space(12.0);
            let (divider, _) = ui.allocate_exact_size(vec2(240.0, 1.0), Sense::hover());
            ui.painter().hline(divider.min.x..=divider.max.x, divider.center().y, Stroke::new(1.0, Color32::from_rgb(68, 68, 68)));
            ui.add_space(12.0);
            fx_glass_slider(app, ui, target, i, tr("Refraction"), "refraction", g.refraction * 100.0, 100.0, |v| {
                with(&|gl| gl.refraction = (v / 100.0).clamp(0.0, 1.0))
            });
            fx_glass_slider(app, ui, target, i, tr("Depth"), "depth", g.depth, 80.0, |v| with(&|gl| gl.depth = v.clamp(1.0, 1000.0)));
            fx_glass_slider(app, ui, target, i, tr("Dispersion"), "dispersion", g.dispersion * 100.0, 100.0, |v| {
                with(&|gl| gl.dispersion = (v / 100.0).clamp(0.0, 1.0))
            });
            fx_glass_slider(app, ui, target, i, tr("Frost"), "frost", g.frost, 30.0, |v| with(&|gl| gl.frost = v.clamp(0.0, 1000.0)));
            fx_glass_slider(app, ui, target, i, tr("Splay"), "splay", g.splay * 100.0, 100.0, |v| with(&|gl| gl.splay = (v / 100.0).clamp(0.0, 1.0)));
            ui.add_space(16.0);
        }
    }
}

/// Applies a new colour to an effect: (app, secondary colour, colour, scrubbing: `Some(done)`).
type ColorEdit<'a> = dyn Fn(&mut LizardApp, bool, lizard_color::Color, Option<bool>) + 'a;

/// `e` with `f` applied to its kind.
fn tweaked(e: &Effect, f: impl FnOnce(&mut EffectKind)) -> Effect {
    let mut e2 = e.clone();
    f(&mut e2.kind);
    e2
}

/// The swatch (opens the colour picker), hex and opacity of effect `i`'s colour (`secondary`: a
/// duo noise's second colour), in the current row; `edit` applies a new one.
fn fx_color(app: &mut LizardApp, ui: &mut Ui, target: &EffectTarget<'_>, i: usize, e: &Effect, secondary: bool, edit: &ColorEdit<'_>) {
    let Some(c) = e.color(secondary) else { return };
    let paint = Paint::solid(c);
    let hex = c.to_hex();
    let change = crate::primitives::ColorControl::new(
        Id::new(("effect_color", i, secondary)),
        &paint,
        crate::primitives::ColorValue::EditableHex(&hex),
        f64::from(c.a) * 100.0,
    )
    .swatch_id(Id::new(("fx_swatch", i, secondary)))
    .show(ui, 136.0);
    if change.open_picker {
        app.ui.picker = Some(target.picker(i, secondary));
    }
    if let Some(h) = change.edited_hex
        && let Some(nc) = lizard_color::Color::from_hex_or_name(&h)
    {
        edit(app, secondary, nc.with_alpha(c.a), None);
    }
    let alpha = |v: f64| c.with_alpha((v / 100.0).clamp(0.0, 1.0) as f32);
    match change.opacity_edit {
        Some(FieldEdit::Set(v)) => edit(app, secondary, alpha(v), None),
        Some(FieldEdit::Scrub(v, done)) => edit(app, secondary, alpha(v), Some(done)),
        None => {}
    }
}

fn grids(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let ids = sel.to_vec();
    let list = common(doc, sel, |n| n.grids.clone());
    let empty = list.as_ref().is_none_or(|l| l.is_empty());
    if let Some(i) = widgets::section_header(ui, tr("Layout grid"), &[(Icon::Styles, tr("Styles")), (Icon::Plus, tr("Add layout grid"))], empty) {
        if i == 1 {
            app.run("grid.add", json!({"ids": ids}));
        } else {
            style_picker::open(app, ui, &ids, StyleSlot::Grid);
        }
    }
    let before_chip = ui.cursor().min.y;
    style_picker::chip(app, ui, doc, &ids, StyleSlot::Grid);
    let has_chip = ui.cursor().min.y > before_chip;
    let Some(list) = list else {
        if has_chip {
            ui.add_space(12.0);
        }
        return;
    };
    if list.is_empty() {
        if has_chip {
            ui.add_space(12.0);
        }
        return;
    }
    // The last grid first, like paints and effects; rows drag to reorder.
    let mut drag = ListDrag::new(ui, "grids");
    let mut removed = None;
    for (i, g) in list.iter().enumerate().rev() {
        let open_id = Id::new(("grid_open", i));
        let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
        let row = grid_row(ui, g, open);
        if row.settings {
            open = !open;
            ui.data_mut(|d| d.insert_temp(open_id, open));
        }
        if let Some(kind) = row.kind {
            let fresh = grid_of_kind(kind);
            app.set_props(&ids, json!({"grids": grids_with(&list, i, |x| *x = fresh)}));
        }
        if row.visibility {
            app.set_props(&ids, json!({"grids": grids_with(&list, i, |x| x.visible = !x.visible)}));
        }
        if row.remove && app.run("grid.remove", json!({"ids": ids, "index": i})).is_some() {
            removed = Some(i);
        }
        if open {
            grid_settings(app, ui, &GridTarget::Layers { ids: &ids, grids: &list }, i, g);
        }
        drag.row(ui, i, row.rect, row.rect.with_max_y(ui.cursor().min.y.max(row.rect.max.y)), || grid_label(g));
    }
    if let Some((from, to)) = drag.finish(ui) {
        app.run("grid.move", json!({"ids": ids, "from": from, "to": to}));
        move_flags(ui, "grid_open", list.len(), from, to);
    }
    // Open settings stay with their grids, or go with theirs.
    if let Some(i) = removed {
        remove_flags(ui, "grid_open", list.len(), i);
    }
    ui.add_space(12.0);
}

pub(crate) struct GridRow {
    pub rect: Rect,
    pub settings: bool,
    pub kind: Option<GridKind>,
    pub visibility: bool,
    pub remove: bool,
}

/// The same layout-grid row in the properties panel and grid-style editor.
pub(crate) fn grid_row(ui: &mut Ui, grid: &LayoutGrid, open: bool) -> GridRow {
    let settings;
    let mut kind = None;
    let visibility;
    let remove;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let content = rect.shrink2(vec2(PAD, (32.0 - FIELD_H) / 2.0));
    let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::left_to_right(egui::Align::Center)));
    content_ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
    {
        let ui = &mut content_ui;
        let width = (ui.available_width() - 3.0 * 24.0 - 3.0 * 8.0).max(60.0);
        let icon = match grid.kind {
            GridKind::Grid => Icon::Grid,
            GridKind::Columns => Icon::LayoutH,
            GridKind::Rows => Icon::LayoutV,
        };
        settings = icons::button(ui, icon, 24.0, open, tr("Layout grid settings")).clicked();
        let trigger = crate::primitives::Select::trigger(ui, &grid_label(grid), width, None);
        menu_popup(&trigger).width(220.0).show(|ui| {
            for (label, value) in [(tr("Grid"), GridKind::Grid), (tr("Columns"), GridKind::Columns), (tr("Rows"), GridKind::Rows)] {
                if menu_item(ui, label, "") {
                    kind = Some(value);
                    ui.close();
                }
            }
        });
        visibility = icons::button(ui, if grid.visible { Icon::Eye } else { Icon::EyeOff }, 24.0, false, tr("Toggle")).clicked();
        remove = icons::button(ui, Icon::Minus, 24.0, false, tr("Remove")).clicked();
    }
    GridRow { rect, settings, kind, visibility, remove }
}

fn grid_of_kind(kind: GridKind) -> LayoutGrid {
    match kind {
        GridKind::Grid => LayoutGrid::default(),
        GridKind::Columns => LayoutGrid::columns(),
        GridKind::Rows => LayoutGrid::rows(),
    }
}

/// A layout grid's name in its row ("Grid (10)", "5 columns").
pub(crate) fn grid_label(g: &LayoutGrid) -> String {
    match g.kind {
        GridKind::Grid => trf("Grid ({size})", &[("size", &g.size)]),
        GridKind::Columns if auto_count(g) => tr("Auto columns").to_string(),
        GridKind::Rows if auto_count(g) => tr("Auto rows").to_string(),
        GridKind::Columns => trn("{n} columns", g.count.max(1) as usize, &[]),
        GridKind::Rows => trn("{n} rows", g.count.max(1) as usize, &[]),
    }
}

/// Fixed-size columns or rows counted Auto: as many as fit.
fn auto_count(g: &LayoutGrid) -> bool {
    g.count == 0 && g.align != GridAlign::Stretch
}

/// `list` with grid `i` changed by `f`.
fn grids_with(list: &[LayoutGrid], i: usize, f: impl FnOnce(&mut LayoutGrid)) -> Vec<LayoutGrid> {
    let mut gs = list.to_vec();
    if let Some(g) = gs.get_mut(i) {
        f(g);
    }
    gs
}

enum GridTarget<'a> {
    Layers { ids: &'a [NodeId], grids: &'a [LayoutGrid] },
    Style { id: &'a str, grids: &'a [LayoutGrid] },
}

impl GridTarget<'_> {
    fn set(&self, app: &mut LizardApp, index: usize, change: impl FnOnce(&mut LayoutGrid), scrub: Option<bool>) {
        let (command, params) = match self {
            Self::Layers { ids, grids } => ("node.set", json!({"ids": ids, "props": {"grids": grids_with(grids, index, change)}})),
            Self::Style { id, grids } => ("style.update", json!({"style": id, "value": grids_with(grids, index, change)})),
        };
        match scrub {
            None => {
                app.run(command, params);
            }
            Some(done) => {
                let _ = app.session.begin_interaction("Layout grid");
                let _ = app.session.execute(command, &params);
                if done {
                    let _ = app.session.commit_interaction();
                }
            }
        }
    }
}

fn grid_target_field(app: &mut LizardApp, target: &GridTarget<'_>, index: usize, edit: Option<FieldEdit>, change: impl Fn(&mut LayoutGrid, f64)) {
    match edit {
        Some(FieldEdit::Set(value)) => target.set(app, index, |grid| change(grid, value), None),
        Some(FieldEdit::Scrub(value, done)) => target.set(app, index, |grid| change(grid, value), Some(done)),
        None => {}
    }
}

/// Layout grid `i`'s settings: size (grids); count, type, width or height, margin or offset and
/// gutter (columns and rows); colour. The numbers can be bound to number variables
/// (`layoutGrids/<i>/<size|count|gutter|offset>`).
fn grid_settings(app: &mut LizardApp, ui: &mut Ui, target: &GridTarget<'_>, i: usize, g: &LayoutGrid) {
    let col = widgets::col_width(ui.available_width());
    // One bindable number field, applied with `apply` (a scrub is one undo step).
    let field = |app: &mut LizardApp, ui: &mut Ui, name: &str, value: f64, width: f32, apply: &dyn Fn(&mut LayoutGrid, f64)| {
        let edit = match target {
            GridTarget::Layers { ids, .. } => {
                let key = format!("layoutGrids/{i}/{name}");
                crate::variable_ui::number_field(app, ui, ids, &key, Id::new(("grid_field", name, i)), Lead::None, Some(value), "", width)
            }
            GridTarget::Style { .. } => widgets::number_field(ui, Id::new(("grid_style_field", name, i)), Lead::None, Some(value), "", width, 1.0),
        };
        grid_target_field(app, target, i, edit, apply);
    };
    let size = |x: &mut LayoutGrid, v: f64| x.size = v.max(1.0);
    let offset = |x: &mut LayoutGrid, v: f64| x.offset = v.max(0.0);
    let gutter = |x: &mut LayoutGrid, v: f64| x.gutter = v.max(0.0);
    if g.kind == GridKind::Grid {
        widgets::sublabels(ui, tr("Size"), None);
        row(ui, 32.0, |ui| field(app, ui, "size", g.size, ui.available_width(), &size));
    } else {
        let cols = g.kind == GridKind::Columns;
        widgets::sublabels(ui, tr("Count"), Some(tr("Type")));
        row(ui, 32.0, |ui| {
            // 0 is Auto for fixed-size columns (rows); a stretched grid has at least one.
            let count = (!auto_count(g)).then(|| f64::from(g.count.max(1)));
            // A stretched grid has at least one band, so 0 (Auto) isn't a count it can take.
            let e = match target {
                GridTarget::Layers { ids, .. } => {
                    let key = format!("layoutGrids/{i}/count");
                    let id = Id::new(("grid_field", "count", i));
                    if g.align == GridAlign::Stretch {
                        crate::variable_ui::number_field_or(app, ui, ids, &key, id, count, col, "", Lead::None, "")
                    } else {
                        crate::variable_ui::number_field_auto(app, ui, ids, &key, id, count, col, Lead::None, "")
                    }
                }
                GridTarget::Style { .. } => widgets::number_field_bindable(
                    ui,
                    Id::new(("grid_style_field", "count", i)),
                    Lead::None,
                    count,
                    "",
                    col,
                    1.0,
                    None,
                    if g.align == GridAlign::Stretch { widgets::Empty::Text("") } else { widgets::Empty::Auto },
                )
                .0
                .and_then(Edit::settled),
            };
            grid_target_field(app, target, i, e, |grid, value| {
                grid.count =
                    if grid.align == GridAlign::Stretch { value.round().clamp(1.0, 1000.0) } else { value.round().clamp(0.0, 1000.0) } as u32;
            });
            let names =
                if cols { [tr("Stretch"), tr("Left"), tr("Center"), tr("Right")] } else { [tr("Stretch"), tr("Top"), tr("Center"), tr("Bottom")] };
            let aligns = [GridAlign::Stretch, GridAlign::Min, GridAlign::Center, GridAlign::Max];
            let cur = aligns.iter().position(|a| *a == g.align).unwrap_or(0);
            let resp = crate::primitives::Select::trigger(ui, names.get(cur).copied().unwrap_or_default(), col, None);
            menu_popup(&resp).show(|ui| {
                for (k, (name, a)) in names.iter().zip(aligns).enumerate() {
                    if menu_check(ui, k == cur, name, "") {
                        // Fixed-size columns need a size of their own.
                        target.set(
                            app,
                            i,
                            |grid| {
                                grid.align = a;
                                if a != GridAlign::Stretch && !(grid.size.is_finite() && grid.size > 0.0) {
                                    grid.size = 60.0;
                                }
                                if a == GridAlign::Stretch {
                                    grid.count = grid.count.max(1);
                                }
                            },
                            None,
                        );
                        ui.close();
                    }
                }
            });
        });
        let span = if cols { tr("Width") } else { tr("Height") };
        match g.align {
            GridAlign::Stretch => {
                widgets::sublabels(ui, span, Some(tr("Margin")));
                row(ui, 32.0, |ui| {
                    // The frame, divided by the count: shown as Auto and not editable.
                    ui.add_enabled_ui(false, |ui| {
                        let auto = widgets::Empty::Text(tr("Auto"));
                        widgets::number_field_bindable(ui, Id::new(("grid_field", "size", i)), Lead::Named(span), None, "", col, 1.0, None, auto)
                    });
                    field(app, ui, "offset", g.offset, col, &offset);
                });
                widgets::sublabels(ui, tr("Gutter"), None);
                row(ui, 32.0, |ui| field(app, ui, "gutter", g.gutter, ui.available_width(), &gutter));
            }
            GridAlign::Center => {
                widgets::sublabels(ui, span, Some(tr("Gutter")));
                row(ui, 32.0, |ui| {
                    field(app, ui, "size", g.size, col, &size);
                    field(app, ui, "gutter", g.gutter, col, &gutter);
                });
            }
            GridAlign::Min | GridAlign::Max => {
                widgets::sublabels(ui, span, Some(tr("Offset")));
                row(ui, 32.0, |ui| {
                    field(app, ui, "size", g.size, col, &size);
                    field(app, ui, "offset", g.offset, col, &offset);
                });
                widgets::sublabels(ui, tr("Gutter"), None);
                row(ui, 32.0, |ui| field(app, ui, "gutter", g.gutter, ui.available_width(), &gutter));
            }
        }
    }
    // The colour: swatch, hex and alpha.
    widgets::sublabels(ui, tr("Color"), None);
    row(ui, 32.0, |ui| {
        let (sw, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        widgets::paint_swatch(ui.painter(), sw, &Paint::solid(g.color));
        let hex_width = (ui.available_width() - 8.0 - 56.0).max(40.0);
        if let Some(h) = widgets::text_field(ui, Id::new(("grid_hex", i)), &g.color.to_hex(), hex_width, "")
            && let Some(c) = lizard_color::Color::from_hex_or_name(&h)
        {
            target.set(app, i, |grid| grid.color = c.with_alpha(grid.color.a), None);
        }
        let e = widgets::number_field(ui, Id::new(("grid_alpha", i)), Lead::None, Some((f64::from(g.color.a) * 100.0).round()), "%", 56.0, 1.0);
        grid_target_field(app, target, i, e, |grid, value| grid.color = grid.color.with_alpha((value / 100.0).clamp(0.0, 1.0) as f32));
    });
}

pub(crate) fn grid_style_popup(app: &mut LizardApp, ctx: &egui::Context, id: &str, grids: &[LayoutGrid], index: usize, grid: &LayoutGrid, at: Pos2) {
    let panel_id = Id::new(("grid_style_settings", id));
    let open_id = Id::new(("grid_style_open", id));
    let mut keep = true;
    crate::primitives::FloatingPanel::new(panel_id, at).width(crate::primitives::FloatingPanelWidth::Compact).open(&mut keep).show_with_header(
        ctx,
        |ui, header| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(240.0);
            ui.painter().text(
                pos2(header.left() + 16.0, header.center().y),
                Align2::LEFT_CENTER,
                grid_label(grid),
                semibold(11.0),
                Tokens::get(ui.ctx()).text,
            );
            let close = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
            if Button::icon(Icon::Close, tr("Close")).icon_size(14.0).show_at(ui, panel_id.with("close"), close).clicked() {
                ui.data_mut(|data| data.remove::<usize>(open_id));
            }
            ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, Tokens::get(ui.ctx()).border));
            ui.add_space(8.0);
            grid_settings(app, ui, &GridTarget::Style { id, grids }, index, grid);
            ui.add_space(12.0);
        },
    );
    if !keep {
        ctx.data_mut(|data| data.remove::<usize>(open_id));
    }
}

/// One group of inspected properties: label, value (with a swatch for colours) and the style or
/// variable it comes from. Clicking a row copies its value.
pub(crate) fn inspect_section(app: &mut LizardApp, ui: &mut Ui, s: &lizard_engine::devinspect::Section) {
    let t = Tokens::get(ui.ctx());
    widgets::separator(ui);
    let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    ui.painter().text(pos2(hr.min.x + PAD, hr.center().y), Align2::LEFT_CENTER, tr(s.title), crate::theme::property_section_title(), t.text);
    for p in &s.rows {
        prop_row(app, ui, p);
    }
    ui.add_space(12.0);
}

/// An inspected property row: label, value (with a swatch for colours) and the style or variable
/// it comes from as a chip. Clicking it copies the value.
pub(crate) fn prop_row(app: &mut LizardApp, ui: &mut Ui, p: &lizard_engine::devinspect::Prop) {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let resp = resp.tip(trf("Copy {value}", &[("value", &p.value)]));
    if resp.hovered() {
        ui.painter().rect_filled(r.shrink2(vec2(8.0, 0.0)), CornerRadius::same(5), t.hover);
    }
    let painter = ui.painter();
    // The engine's property labels ("Width") are keys of their own.
    let label = tr(&p.label);
    // Elided before the value column (some languages' labels run longer).
    painter.text(
        pos2(r.min.x + PAD, r.center().y),
        Align2::LEFT_CENTER,
        widgets::elide(ui, label, 96.0 - PAD - 6.0),
        regular(11.0),
        t.text_secondary,
    );
    // The style or variable, right-aligned as a chip.
    let mut right = r.max.x - PAD;
    if let Some(token) = &p.token {
        let galley = painter.layout_no_wrap(token.clone(), regular(10.0), t.text_secondary);
        let chip =
            Rect::from_min_max(pos2((right - galley.size().x - 12.0).max(r.min.x + 196.0), r.center().y - 9.0), pos2(right, r.center().y + 9.0));
        painter.rect_filled(chip, CornerRadius::same(4), t.field);
        painter.with_clip_rect(chip.shrink2(vec2(6.0, 0.0))).galley(
            pos2(chip.min.x + 6.0, chip.center().y - galley.size().y / 2.0),
            galley,
            t.text_secondary,
        );
        right = chip.min.x - 6.0;
    }
    let mut x = r.min.x + 96.0;
    if let Some(c) = p.color.as_deref().and_then(lizard_color::Color::from_hex) {
        let sw = Rect::from_min_size(pos2(x, r.center().y - 6.0), vec2(12.0, 12.0));
        widgets::paint_swatch(painter, sw, &Paint::solid(c));
        x += 18.0;
    }
    let value_r = Rect::from_min_max(pos2(x, r.min.y), pos2(right.max(x + 10.0), r.max.y));
    painter.with_clip_rect(value_r).text(pos2(x, r.center().y), Align2::LEFT_CENTER, &p.value, regular(11.0), t.text);
    if resp.clicked() {
        app.services.clipboard_set_text(&p.value);
        app.toast(trf("Copied {value}", &[("value", &label.to_lowercase())]));
    }
}

/// The selection's assets (layers with export settings, icons, images): click one to export it,
/// or export them all.
fn inspect_assets(app: &mut LizardApp, ui: &mut Ui, assets: &[lizard_engine::devinspect::Asset]) {
    let t = Tokens::get(ui.ctx());
    widgets::separator(ui);
    let export_all = [(Icon::Export, tr("Export all"))];
    let all = widgets::section_header(ui, tr("Assets"), if assets.is_empty() { &[] } else { &export_all }, false).is_some();
    if assets.is_empty() {
        widgets::note(ui, tr("No assets: no export settings, icons or images. Add export settings in the Design tab."), t.text_secondary);
        ui.add_space(6.0);
        return;
    }
    let mut export: Option<Vec<NodeId>> = all.then(|| assets.iter().map(|a| a.id).collect());
    for a in assets {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
        let resp = resp.tip(trf("Export {name}", &[("name", &a.name)]));
        if resp.hovered() {
            ui.painter().rect_filled(r.shrink2(vec2(8.0, 0.0)), CornerRadius::same(5), t.hover);
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.max.x - PAD - 6.0, r.center().y), vec2(12.0, 12.0)), Icon::Export, t.icon);
        }
        let name_r = Rect::from_min_max(pos2(r.min.x + PAD, r.min.y), pos2(r.min.x + 96.0 - 4.0, r.max.y));
        ui.painter().with_clip_rect(name_r).text(pos2(name_r.min.x, r.center().y), Align2::LEFT_CENTER, &a.name, regular(11.0), t.text);
        let fmt_r = Rect::from_min_max(pos2(r.min.x + 96.0, r.min.y), pos2(r.max.x - PAD - 16.0, r.max.y));
        ui.painter().with_clip_rect(fmt_r).text(
            pos2(fmt_r.min.x, r.center().y),
            Align2::LEFT_CENTER,
            a.exports.join(", "),
            regular(11.0),
            t.text_secondary,
        );
        if resp.clicked() {
            export = Some(vec![a.id]);
        }
    }
    if let Some(ids) = export {
        crate::export_panel::export_layers(app, &ids, true);
    }
    ui.add_space(12.0);
}

fn inspect_disclosure(ui: &mut Ui, title: &str, id: Id, default_open: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(default_open);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    if response.clicked() {
        open = !open;
        ui.data_mut(|data| data.insert_temp(id, open));
    }
    let ink = if response.hovered() { t.text } else { t.text_secondary };
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(rect.left() + PAD - 8.0, rect.center().y), vec2(8.0, 8.0)),
        if open { Icon::Chevron } else { Icon::ChevronRight },
        ink,
    );
    ui.painter().text(pos2(rect.left() + PAD, rect.center().y), Align2::LEFT_CENTER, title, crate::theme::property_section_title(), ink);
    crate::a11y::disclosure(&response, title, open);
    open
}

fn inspect_box_model(ui: &mut Ui, node: &Node) {
    let t = Tokens::get(ui.ctx());
    let width = (ui.available_width() - 32.0).max(120.0);
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 188.0), Sense::hover());
    let card = Rect::from_min_size(pos2(outer.left() + 16.0, outer.top() + 6.0), vec2(width, 176.0));
    let card_fill = if t.dark { Color32::from_rgb(48, 48, 48) } else { t.field };
    let border_fill = if t.dark { Color32::from_rgb(39, 39, 39) } else { t.panel };
    let border_ink = if t.dark { Color32::from_rgb(70, 70, 70) } else { t.border };
    ui.painter().rect_filled(card, CornerRadius::same(7), card_fill);
    let border = card.shrink2(vec2(30.0, 31.0));
    ui.painter().rect_filled(border, CornerRadius::same(3), border_fill);
    ui.painter().rect_stroke(border, CornerRadius::same(3), Stroke::new(1.0, border_ink), egui::StrokeKind::Inside);
    ui.painter().text(border.left_top() + vec2(38.0, 12.0), Align2::LEFT_CENTER, tr("Border"), regular(11.0), t.text_secondary);
    let stroke = if node.strokes.is_empty() { "-".to_owned() } else { format!("{}", node.stroke.weight.round()) };
    let border_metrics = [
        pos2(border.left() + 14.0, border.top() + 12.0),
        pos2(border.center().x, border.top() + 12.0),
        pos2(border.right() - 14.0, border.top() + 12.0),
        pos2(border.left() + 14.0, border.center().y),
        pos2(border.right() - 14.0, border.center().y),
        pos2(border.left() + 14.0, border.bottom() - 12.0),
        pos2(border.center().x, border.bottom() - 12.0),
        pos2(border.right() - 14.0, border.bottom() - 12.0),
    ];
    for metric in border_metrics {
        ui.painter().text(metric, Align2::CENTER_CENTER, &stroke, regular(10.0), t.text_secondary);
    }
    let padding = border.shrink2(vec2(30.0, 22.0));
    ui.painter().rect_filled(padding, CornerRadius::same(3), if t.dark { Color32::from_rgb(52, 61, 83) } else { Color32::from_rgb(225, 231, 244) });
    ui.painter().rect_stroke(
        padding,
        CornerRadius::same(3),
        Stroke::new(1.0, if t.dark { Color32::from_rgb(215, 222, 236) } else { t.border }),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(padding.left_top() + vec2(8.0, 11.0), Align2::LEFT_CENTER, tr("Padding|inspect"), regular(11.0), t.text_secondary);
    let pad = node.layout.as_ref().map(|layout| layout.padding).unwrap_or([0.0; 4]);
    let pad_text = |value: f64| if node.layout.is_some() { format!("{}", value.round()) } else { "-".to_owned() };
    ui.painter().text(pos2(padding.center().x, padding.top() + 11.0), Align2::CENTER_CENTER, pad_text(pad[0]), regular(10.0), t.text_secondary);
    ui.painter().text(pos2(padding.right() - 11.0, padding.center().y), Align2::CENTER_CENTER, pad_text(pad[1]), regular(10.0), t.text_secondary);
    ui.painter().text(pos2(padding.center().x, padding.bottom() - 10.0), Align2::CENTER_CENTER, pad_text(pad[2]), regular(10.0), t.text_secondary);
    ui.painter().text(pos2(padding.left() + 11.0, padding.center().y), Align2::CENTER_CENTER, pad_text(pad[3]), regular(10.0), t.text_secondary);
    let content = padding.shrink2(vec2(30.0, 23.0));
    ui.painter().rect_filled(content, CornerRadius::same(3), border_fill);
    inspect_dashed_rect(ui.painter(), content, Stroke::new(1.2, if t.dark { Color32::WHITE } else { t.text }));
    ui.painter().text(content.center(), Align2::CENTER_CENTER, format!("{} × {}", node.width.round(), node.height.round()), regular(11.0), t.text);
    ui.painter().text(card.right_bottom() - vec2(9.0, 12.0), Align2::RIGHT_CENTER, "border-box", regular(10.0), t.text_tertiary);
}

fn inspect_dashed_rect(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
    let dash = 3.0;
    let pitch = 6.0;
    for y in [rect.top(), rect.bottom()] {
        let mut x = rect.left();
        while x < rect.right() {
            painter.line_segment([pos2(x, y), pos2((x + dash).min(rect.right()), y)], stroke);
            x += pitch;
        }
    }
    for x in [rect.left(), rect.right()] {
        let mut y = rect.top();
        while y < rect.bottom() {
            painter.line_segment([pos2(x, y), pos2(x, (y + dash).min(rect.bottom()))], stroke);
            y += pitch;
        }
    }
}

fn inspect_colors(ui: &mut Ui, doc: &Document, selection: &[NodeId], format: lizard_engine::devinspect::ColorFormat) {
    let t = Tokens::get(ui.ctx());
    let colors = lizard_engine::selection_colors::collect(doc, selection);
    if colors.is_empty() {
        return;
    }
    let (heading, _) = ui.allocate_exact_size(vec2(ui.available_width(), 35.0), Sense::hover());
    ui.painter().text(pos2(heading.left() + 16.0, heading.center().y), Align2::LEFT_CENTER, tr("Selection colors"), regular(11.0), t.text_secondary);
    for group in colors {
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let swatch = Rect::from_center_size(pos2(row.left() + 26.0, row.center().y), vec2(17.0, 17.0));
        widgets::paint_swatch(ui.painter(), swatch, &group.paint);
        let label = group.paint.solid_color().map_or_else(|| widgets::paint_name(&group.paint), |color| inspect_color_label(format, color));
        let text_rect = Rect::from_min_max(pos2(row.left() + 44.0, row.top()), pos2(row.right() - 16.0, row.bottom()));
        ui.painter().with_clip_rect(text_rect).text(text_rect.left_center(), Align2::LEFT_CENTER, label, regular(11.0), t.text);
        if let PaintKind::Gradient(gradient) = &group.paint.kind {
            for stop in gradient.sorted_stops() {
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                let paint = Paint::solid(stop.color.with_alpha(stop.color.a * group.paint.opacity));
                let swatch = Rect::from_center_size(pos2(row.left() + 44.0, row.center().y), vec2(16.0, 16.0));
                widgets::paint_swatch(ui.painter(), swatch, &paint);
                let text_rect = Rect::from_min_max(pos2(row.left() + 60.0, row.top()), pos2(row.right() - 16.0, row.bottom()));
                ui.painter().with_clip_rect(text_rect).text(
                    text_rect.left_center(),
                    Align2::LEFT_CENTER,
                    inspect_color_label(format, stop.color.with_alpha(stop.color.a * group.paint.opacity)),
                    regular(11.0),
                    t.text,
                );
            }
        }
    }
    ui.add_space(12.0);
}

fn inspect_color_label(format: lizard_engine::devinspect::ColorFormat, color: lizard_color::Color) -> String {
    if format == lizard_engine::devinspect::ColorFormat::Hsb {
        let hsb = lizard_color::Hsb::from_color(color);
        format!("hsba({}, {}%, {}%, {})", hsb.h.round(), (hsb.s * 100.0).round(), (hsb.b * 100.0).round(), color.a)
    } else {
        format.format(color)
    }
}

/// The Inspect tab (Dev Mode): the handoff status, annotations, properties, assets and code of
/// the selected layer, or the list of what's ready for dev.
fn inspect(app: &mut LizardApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    if app.ui.dev.plugins_tab {
        crate::dev_mode::plugins_section(app, ui);
        return;
    }
    let Ok(ds) = app.session.ds() else { return };
    let doc = ds.doc.clone();
    let sel = ds.selection.clone();
    let Some(first) = sel.first().copied() else {
        ui.add_space(12.0);
        widgets::note(ui, tr("Select a layer to see its properties and code"), t.text_secondary);
        widgets::note(ui, tr("Hover other layers to measure the distance"), t.text_secondary);
        crate::dev_mode::ready_list(app, ui);
        crate::dev_mode::categories_section(app, ui);
        return;
    };
    let Some(node) = doc.node(first) else { return };
    let kind = tr(node.kind.label());
    let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 68.0), Sense::hover());
    ui.painter().text(pos2(header.left() + 16.0, header.top() + 23.0), Align2::LEFT_CENTER, kind, semibold(14.0), t.text);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(header.left() + 24.0, header.top() + 51.0), vec2(13.0, 13.0)),
        crate::layers::node_icon(&doc, first),
        t.text_secondary,
    );
    ui.painter().text(pos2(header.left() + 42.0, header.top() + 51.0), Align2::LEFT_CENTER, &node.name, regular(11.0), t.text_secondary);
    let code_button = Rect::from_center_size(pos2(header.right() - 25.0, header.top() + 23.0), vec2(25.0, 25.0));
    if Button::icon(Icon::Code, tr("Show code")).variant(ButtonVariant::Ghost).show_at(ui, ui.id().with("inspect_code"), code_button).clicked() {
        ui.data_mut(|data| data.insert_temp(Id::new("inspect_view_code"), true));
    }
    widgets::separator(ui);
    if inspect_disclosure(ui, "MCP", Id::new("inspect_mcp_open"), false) {
        widgets::note(ui, tr("Use the Lizard MCP server to inspect this document"), t.text_secondary);
    }
    widgets::separator(ui);
    if !inspect_disclosure(ui, tr("Layer properties"), Id::new("inspect_properties_open"), true) {
        return;
    }
    inspect_box_model(ui, node);
    use lizard_engine::devinspect::ColorFormat;
    let fmt_id = Id::new("inspect_color_format");
    let fmt = ui.data(|d| d.get_temp::<usize>(fmt_id)).and_then(|i| ColorFormat::ALL.get(i).copied()).unwrap_or(ColorFormat::Hsb);
    use lizard_engine::codegen::CodeLang;
    let lang_id = Id::new("inspect_lang");
    let lang = ui.data(|d| d.get_temp::<usize>(lang_id)).and_then(|i| CodeLang::ALL.get(i).copied()).unwrap_or_default();
    let rem_id = Id::new("inspect_rem");
    let rem = lang.is_web() && ui.data(|d| d.get_temp::<bool>(rem_id)).unwrap_or(false);
    let view_id = Id::new("inspect_view_code");
    let mut code_view = ui.data(|data| data.get_temp::<bool>(view_id)).unwrap_or(true);
    ui.add_space(7.0);
    let (toolbar_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let content = toolbar_rect.shrink2(vec2(PAD, 4.0));
    let language_rect = Rect::from_min_size(pos2(content.right() - 82.0, content.top()), vec2(82.0, content.height()));
    let controls_rect = Rect::from_min_max(content.min, pos2(language_rect.left() - 8.0, content.bottom()));
    let mut toolbar = ui.new_child(egui::UiBuilder::new().max_rect(controls_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    toolbar.spacing_mut().item_spacing = vec2(8.0, 0.0);
    {
        let ui = &mut toolbar;
        let segments = [crate::primitives::Segment::text(tr("List")), crate::primitives::Segment::text(tr("Code"))];
        if let Some(selected) = crate::primitives::SegmentedControl::new(&segments, Some(usize::from(code_view))).show(ui) {
            code_view = selected == 1;
            ui.data_mut(|data| data.insert_temp(view_id, code_view));
        }
        if code_view && lang.is_web() {
            let resp = crate::primitives::Select::trigger(ui, if rem { "rem" } else { "px" }, 55.0, None);
            menu_popup(&resp).show(|ui| {
                for (on, label) in [(false, "px"), (true, "rem")] {
                    if menu_item(ui, label, "") {
                        ui.data_mut(|data| data.insert_temp(rem_id, on));
                        ui.close();
                    }
                }
            });
        }
    }
    let mut language_ui = ui.new_child(egui::UiBuilder::new().max_rect(language_rect));
    let resp = crate::primitives::Select::trigger(&mut language_ui, lang.label(), 82.0, None);
    menu_popup(&resp).show(|ui| {
        for (i, language) in CodeLang::ALL.iter().enumerate() {
            if menu_item(ui, language.label(), "") {
                ui.data_mut(|data| data.insert_temp(lang_id, i));
                ui.close();
            }
        }
    });
    let played = crate::dev_mode::playground_section(app, ui, &doc, first, lang);
    if code_view {
        let (heading, _) = ui.allocate_exact_size(vec2(ui.available_width(), 39.0), Sense::hover());
        ui.painter().text(
            pos2(heading.left() + 16.0, heading.center().y),
            Align2::LEFT_CENTER,
            if lang == CodeLang::Css { tr("Layout") } else { tr("Code") },
            regular(12.0),
            t.text,
        );
        let copy_rect = Rect::from_center_size(pos2(heading.right() - 25.0, heading.center().y), vec2(26.0, 26.0));
        let mut code = played.unwrap_or_else(|| crate::dev_mode::code(app, ui, first, lang));
        if rem {
            code = lizard_engine::codegen::px_to_rem(&code, 16.0);
        }
        let copy_response = ui.interact(copy_rect, ui.id().with("inspect_copy_code"), Sense::click()).tip(tr("Copy code"));
        crate::a11y::button(&copy_response, tr("Copy code"));
        if copy_response.hovered() {
            ui.painter().rect_filled(copy_rect, CornerRadius::same(5), t.hover);
        }
        icons::paint_copy(ui.painter(), copy_rect.shrink(4.0), t.icon);
        if copy_response.clicked() {
            app.services.clipboard_set_text(&code);
            app.toast(trf("Copied {value}", &[("value", &lang.label())]));
        }
        let height = ((code.lines().count().min(11) as f32) * 17.0 + 24.0).clamp(80.0, 220.0);
        let block_width = (ui.available_width() - 2.0 * PAD).max(120.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(PAD);
            crate::primitives::CodeBlock::new(ui.id().with(("inspect_code_block", first)))
                .size(block_width, height)
                .language(match lang {
                    CodeLang::Css => crate::primitives::CodeLanguage::Css,
                    CodeLang::Html => crate::primitives::CodeLanguage::Html,
                    CodeLang::React => crate::primitives::CodeLanguage::React,
                    CodeLang::Leptos => crate::primitives::CodeLanguage::Leptos,
                    CodeLang::SwiftUi => crate::primitives::CodeLanguage::SwiftUi,
                    CodeLang::Compose => crate::primitives::CodeLanguage::Compose,
                })
                .editable(false)
                .show(ui, &mut code);
        });
        ui.add_space(16.0);
    } else {
        for section in lizard_engine::devinspect::properties_with(&doc, first, fmt) {
            inspect_section(app, ui, &section);
        }
    }
    widgets::separator(ui);
    let (colors_heading, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    ui.painter().text(pos2(colors_heading.left() + 16.0, colors_heading.center().y), Align2::LEFT_CENTER, tr("Colors"), regular(12.0), t.text);
    let format_rect = Rect::from_center_size(pos2(colors_heading.right() - 52.0, colors_heading.center().y), vec2(76.0, 26.0));
    let mut format_ui = ui.new_child(egui::UiBuilder::new().max_rect(format_rect));
    let resp = crate::primitives::Select::trigger(&mut format_ui, fmt.label(), 76.0, None).tip(tr("Color format"));
    menu_popup(&resp).show(|ui| {
        for (i, format) in ColorFormat::ALL.iter().enumerate() {
            if menu_item(ui, format.label(), "") {
                ui.data_mut(|data| data.insert_temp(fmt_id, i));
                ui.close();
            }
        }
    });
    inspect_colors(ui, &doc, &sel, fmt);
    inspect_assets(app, ui, &lizard_engine::devinspect::assets(&doc, &sel));
    crate::dev_mode::status(app, ui, &doc, &sel);
    crate::dev_mode::annotations_section(app, ui, &doc, first);
    crate::dev_mode::dev_resources_section(app, ui, &doc, first);
    crate::dev_mode::connect_section(app, ui, &doc, first, lang);
}

#[cfg(test)]
mod effect_panel_tests {
    use super::*;

    #[test]
    fn shared_effect_row_opens_settings_from_its_button() {
        let ctx = egui::Context::default();
        let point = pos2(80.0, 16.0);
        let mut clicked = false;
        for events in [
            vec![egui::Event::PointerMoved(point)],
            vec![egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }],
            vec![egui::Event::PointerButton { pos: point, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
        ] {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))), events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                ui.set_width(304.0);
                clicked |= effect_row(ui, Id::new("shared_effect_row_test"), &Effect::drop_shadow(), false).settings;
            });
            output.textures_delta.clear();
        }
        assert!(clicked);
    }

    #[test]
    fn shared_effect_editor_updates_effect_styles_through_the_command_engine() {
        let mut session = lizard_engine::Session::new();
        let created = session.execute(
            "style.create",
            &json!({
                "type": "effect", "name": "Shadow style", "value": [Effect::drop_shadow()]
            }),
        );
        let id = created.ok().and_then(|value| value["id"].as_str().map(str::to_string)).unwrap_or_default();
        assert!(!id.is_empty(), "effect style should be created");
        let mut app = LizardApp::new(session, Box::new(crate::NoServices));
        let effects = app.session.doc().ok().and_then(|doc| match &doc.style(&id)?.kind {
            StyleKind::Effect { effects } => Some(effects.clone()),
            _ => None,
        });
        let effects = effects.unwrap_or_default();
        assert_eq!(effects.len(), 1, "effect style should have one effect");
        let mut changed = effects.first().cloned().unwrap_or_else(Effect::drop_shadow);
        if let EffectKind::DropShadow(shadow) = &mut changed.kind {
            shadow.radius = 18.0;
        }
        EffectTarget::Style { id: &id, effects: &effects }.set(&mut app, 0, changed.clone(), None);
        let saved = app.session.doc().ok().and_then(|doc| match &doc.style(&id)?.kind {
            StyleKind::Effect { effects } => effects.first().cloned(),
            _ => None,
        });
        assert_eq!(saved, Some(changed));
    }

    #[test]
    fn shared_grid_settings_update_grid_styles_through_the_command_engine() {
        let mut session = lizard_engine::Session::new();
        let created = session.execute(
            "style.create",
            &json!({
                "type": "grid", "name": "Grid style", "value": [LayoutGrid::default()]
            }),
        );
        let id = created.ok().and_then(|value| value["id"].as_str().map(str::to_string)).unwrap_or_default();
        assert!(!id.is_empty(), "grid style should be created");
        let mut app = LizardApp::new(session, Box::new(crate::NoServices));
        let grids = app
            .session
            .doc()
            .ok()
            .and_then(|doc| match &doc.style(&id)?.kind {
                StyleKind::Grid { grids } => Some(grids.clone()),
                _ => None,
            })
            .unwrap_or_default();
        GridTarget::Style { id: &id, grids: &grids }.set(&mut app, 0, |grid| grid.size = 18.0, None);
        let saved = app.session.doc().ok().and_then(|doc| match &doc.style(&id)?.kind {
            StyleKind::Grid { grids } => grids.first().cloned(),
            _ => None,
        });
        assert_eq!(saved.map(|grid| grid.size), Some(18.0));
    }

    #[test]
    fn effect_button_and_floating_panel_have_distinct_layers() {
        let ctx = egui::Context::default();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(24.0, 24.0));
            let _ = ui.interact(rect, Id::new(("effect_settings", 0)), Sense::click());
            let _ = crate::primitives::FloatingPanel::new(effect_panel_id(0), pos2(100.0, 100.0))
                .height(crate::primitives::FloatingPanelHeight::Fixed(120.0))
                .show(ui.ctx(), |ui| {
                    ui.label("Effect settings");
                });
        });
        out.textures_delta.clear();
    }

    #[test]
    fn property_rows_reserve_the_action_column_at_every_panel_width() {
        for width in [200.0, 240.0, 480.0] {
            let ctx = egui::Context::default();
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))), ..Default::default() };
            let mut measured = None;
            let mut out = ctx.run_ui(raw, |ui| {
                let panel_rect = Rect::from_min_size(pos2(100.0, 20.0), vec2(width, 100.0));
                let mut panel = ui.new_child(egui::UiBuilder::new().max_rect(panel_rect));
                row(&mut panel, 32.0, |body| {
                    let main_width = body.available_width();
                    let (control, _) = body.allocate_exact_size(vec2(main_width, FIELD_H), Sense::hover());
                    let (action, _) = body.allocate_exact_size(vec2(24.0, FIELD_H), Sense::hover());
                    measured = Some((main_width, control, action));
                });
            });
            out.textures_delta.clear();
            assert!(measured.is_some(), "property row did not render");
            if let Some((main_width, control, action)) = measured {
                assert_eq!(main_width, width - PAD * 2.0 - widgets::PROPERTY_ACTION_SLOT);
                assert_eq!(control.left(), 100.0 + PAD);
                assert_eq!(action.right(), 100.0 + width - PAD);
            }
        }
    }
}
