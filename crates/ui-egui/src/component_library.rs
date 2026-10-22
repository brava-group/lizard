//! Interactive UI catalog for local development. The module and its state are absent from
//! release builds, so this window and its sample data never enter the production bundle.

use egui::{Color32, CornerRadius, Id, Layout, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::icons::{self, AgentIcon, AppearanceIcon, HeaderIcon, Icon, LayerIcon, LayoutIcon, NavIcon, PositionIcon, SizingIcon};
use crate::theme::{self, Tokens};
use crate::widgets::{self, DropLead, Empty, FieldEdit, Lead};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Buttons,
    Numeric,
    Text,
    Dropdowns,
    Segmented,
    Choices,
    Paint,
    Sliders,
    Structure,
    Menus,
    Typography,
    Icons,
    Tokens,
}

const PAGES: &[(Page, &str, &str)] = &[
    (Page::Buttons, "Buttons", "button · icon_button"),
    (Page::Numeric, "Number fields", "number_field · sizing"),
    (Page::Text, "Text fields", "text_field · combo"),
    (Page::Dropdowns, "Dropdowns", "dropdown · dropdown_lead"),
    (Page::Segmented, "Segmented", "segmented · segmented_text"),
    (Page::Choices, "Choices", "checkbox · radio"),
    (Page::Paint, "Paint", "swatch · checker"),
    (Page::Sliders, "Sliders", "slider · progress_bar"),
    (Page::Structure, "Sections", "section_header · disclosure · bar"),
    (Page::Menus, "Menus", "menu_popup · menu_check"),
    (Page::Typography, "Typography", "label · note · sublabels"),
    (Page::Icons, "Icons", "icon families · toolbar"),
    (Page::Tokens, "Tokens", "theme::Tokens"),
];

pub struct State {
    pub open: bool,
    page: Page,
    clicks: usize,
    number: f64,
    bound: bool,
    text: String,
    draft: String,
    multiline: String,
    combo: String,
    option: usize,
    segment: usize,
    icon_segment: usize,
    checked: bool,
    checked_other: bool,
    radio: usize,
    slider: f64,
    disclosure: bool,
    menu_choice: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            page: Page::Buttons,
            clicks: 0,
            number: 128.0,
            bound: false,
            text: "Component name".into(),
            draft: String::new(),
            multiline: "A note with more than one line.".into(),
            combo: "1x".into(),
            option: 0,
            segment: 0,
            icon_segment: 0,
            checked: true,
            checked_other: false,
            radio: 0,
            slider: 42.0,
            disclosure: true,
            menu_choice: 0,
        }
    }
}

pub fn show(ctx: &egui::Context, state: &mut State) {
    if !state.open {
        return;
    }
    let t = Tokens::get(ctx);
    let mut open = state.open;
    let screen = ctx.content_rect();
    let size = vec2((screen.width() - 64.0).clamp(620.0, 1040.0), (screen.height() - 80.0).clamp(440.0, 700.0));
    egui::Window::new("UI Library · development")
        .id(Id::new("component_library_window"))
        .open(&mut open)
        .collapsible(false)
        .fixed_size(size)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(screen.center())
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.panel).inner_margin(egui::Margin::same(12)))
        .show(ctx, |ui| {
            ui.set_min_size(size - vec2(24.0, 24.0));
            let body_h = ui.available_height();
            let content_w = ui.available_width() - 215.0;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(vec2(190.0, body_h), Layout::top_down(egui::Align::Min), |ui| {
                    ui.label(egui::RichText::new("COMPONENTS").size(10.0).strong().color(t.text_secondary));
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical().id_salt("ui_library_nav").show(ui, |ui| {
                        for &(page, name, _) in PAGES {
                            let selected = state.page == page;
                            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                            if selected || response.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(6), if selected { t.selected_row } else { t.hover });
                            }
                            ui.painter().text(
                                pos2(rect.min.x + 10.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                name,
                                theme::regular(12.0),
                                if selected { t.text } else { t.text_secondary },
                            );
                            if response.clicked() {
                                state.page = page;
                            }
                        }
                    });
                });
                ui.separator();
                ui.allocate_ui_with_layout(vec2(content_w, body_h), Layout::top_down(egui::Align::Min), |ui| {
                    egui::ScrollArea::vertical().id_salt("ui_library_content").show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(content_w - 12.0);
                            let (name, source) = PAGES
                                .iter()
                                .find(|(page, _, _)| *page == state.page)
                                .map_or(("Components", ""), |(_, name, source)| (*name, *source));
                            ui.heading(name);
                            ui.label(egui::RichText::new(source).size(11.0).color(t.text_secondary));
                            ui.add_space(12.0);
                            match state.page {
                                Page::Buttons => buttons(ui, state),
                                Page::Numeric => numeric(ui, state),
                                Page::Text => text_fields(ui, state),
                                Page::Dropdowns => dropdowns(ui, state),
                                Page::Segmented => segmented(ui, state),
                                Page::Choices => choices(ui, state),
                                Page::Paint => paint(ui),
                                Page::Sliders => sliders(ui, state),
                                Page::Structure => structure(ui, state),
                                Page::Menus => menus(ui, state),
                                Page::Typography => typography(ui),
                                Page::Icons => iconography(ui),
                                Page::Tokens => tokens(ui),
                            }
                        });
                    });
                });
            });
        });
    state.open = open;
}

fn example(ui: &mut Ui, title: &str, detail: &str, add: impl FnOnce(&mut Ui)) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE
        .fill(t.panel)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 2.0).max(350.0));
            ui.label(egui::RichText::new(title).size(13.0).strong().color(t.text));
            ui.label(egui::RichText::new(detail).size(10.0).color(t.text_secondary));
            ui.add_space(10.0);
            add(ui);
        });
    ui.add_space(10.0);
}

fn buttons(ui: &mut Ui, state: &mut State) {
    example(ui, "Text buttons", "Primary, secondary, disabled, and hover are drawn by widgets::button.", |ui| {
        ui.horizontal(|ui| {
            if widgets::button(ui, "Primary", true, true).clicked() {
                state.clicks += 1;
            }
            if widgets::button(ui, "Secondary", false, true).clicked() {
                state.clicks += 1;
            }
            widgets::button(ui, "Disabled", true, false);
            widgets::button(ui, "Disabled", false, false);
        });
        ui.label(format!("Clicks: {}", state.clicks));
    });
    example(ui, "Icon buttons", "Hover highlights and tooltips are interactive.", |ui| {
        ui.horizontal(|ui| {
            for (icon, name) in [(Icon::Plus, "Add"), (Icon::Eye, "Show"), (Icon::Lock, "Lock"), (Icon::Chevron, "Expand")] {
                if widgets::icon_button(ui, icon, name) {
                    state.clicks += 1;
                }
            }
        });
    });
}

fn apply_edit(value: &mut f64, edit: Option<FieldEdit>) {
    if let Some(FieldEdit::Set(next) | FieldEdit::Scrub(next, _)) = edit {
        *value = next;
    }
}

fn numeric(ui: &mut Ui, state: &mut State) {
    example(ui, "Numeric value", "Commit with Enter or scrub the label. Suffixes and leading icons are supported.", |ui| {
        ui.horizontal(|ui| {
            let current = state.number;
            let edit = widgets::number_field(ui, Id::new("library_number"), Lead::Text("W"), Some(current), "", 110.0, 1.0);
            apply_edit(&mut state.number, edit);
            let current = state.number;
            let edit = widgets::number_field(ui, Id::new("library_percent"), Lead::Icon(Icon::Opacity, "Opacity"), Some(current), "%", 120.0, 1.0);
            apply_edit(&mut state.number, edit);
        });
    });
    example(ui, "Empty and mixed", "The same field can show Auto, Mixed, and a variable binding.", |ui| {
        ui.horizontal(|ui| {
            let _ = widgets::number_field_or(ui, Id::new("library_auto"), None, "Auto", 110.0, 1.0);
            let _ = widgets::number_field_mixed(ui, Id::new("library_mixed"), Lead::Text("X"), None, "", 110.0, 1.0);
        });
        ui.horizontal(|ui| {
            let binding = if state.bound { Some(Some("spacing.md")) } else { Some(None) };
            let (edit, event) = widgets::number_field_bindable(
                ui,
                Id::new("library_bound"),
                Lead::Text("H"),
                Some(state.number),
                "",
                145.0,
                1.0,
                binding,
                Empty::Auto,
            );
            apply_edit(&mut state.number, edit.and_then(|edit| edit.settled()));
            if event.is_some() {
                state.bound = !state.bound;
            }
            if widgets::button(ui, if state.bound { "Unbind" } else { "Bind" }, false, true).clicked() {
                state.bound = !state.bound;
            }
        });
    });
    example(ui, "Sizing field", "The trailing sizing selector is part of the same control.", |ui| {
        let (edit, _, response) =
            widgets::number_field_with_sizing(ui, Id::new("library_sizing"), Lead::Text("W"), Some(state.number), 170.0, None, "Fixed");
        apply_edit(&mut state.number, edit.and_then(|edit| edit.settled()));
        crate::right::menu_popup(&response).show(|ui| {
            for name in ["Fixed", "Hug contents", "Fill container"] {
                if crate::right::menu_item(ui, name, "") {
                    ui.close();
                }
            }
        });
    });
}

fn text_fields(ui: &mut Ui, state: &mut State) {
    example(ui, "Committed text", "Click to edit; Enter or focus loss commits the value.", |ui| {
        if let Some(next) = widgets::text_field(ui, Id::new("library_text"), &state.text, 220.0, "Name") {
            state.text = next;
        }
        if let Some(next) = widgets::text_field_named(ui, Id::new("library_named"), &state.text, 220.0, "Layer name") {
            state.text = next;
        }
    });
    example(ui, "Live text", "Changes are reported while typing.", |ui| {
        widgets::text_input(ui, Id::new("library_live_text"), &mut state.draft, 250.0, "Search components");
        ui.label(format!("Current value: {}", state.draft));
    });
    example(ui, "Inline rename", "Click the text to edit it in place, as in the layer list.", |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(250.0, 24.0), Sense::hover());
        if let Some(next) = widgets::text_field_inline(ui, Id::new("library_inline_name"), &state.text, rect, theme::regular(12.0)) {
            state.text = next;
        }
    });
    example(ui, "Combo field", "Type a custom value or choose a preset.", |ui| {
        let presets = ["1x".to_owned(), "2x".to_owned(), "3x".to_owned()];
        if let Some(next) = widgets::combo_field(ui, Id::new("library_combo"), &state.combo, 140.0, &presets) {
            state.combo = next;
        }
    });
    example(ui, "Multiline", "The draft commits when the editor loses focus.", |ui| {
        if let Some(next) = widgets::multiline_field(ui, Id::new("library_multiline"), &state.multiline, "Description", 3) {
            state.multiline = next;
        }
    });
}

fn dropdowns(ui: &mut Ui, state: &mut State) {
    const OPTIONS: [&str; 3] = ["Normal", "Multiply", "Screen"];
    example(ui, "Dropdown variants", "Plain, icon lead, and color dot lead. Click each control to inspect its menu.", |ui| {
        ui.horizontal(|ui| {
            for lead in [None, Some(DropLead::Icon(Icon::Eye)), Some(DropLead::Dot(Color32::from_rgb(0x0D, 0x99, 0xFF)))] {
                let response = widgets::dropdown_lead(ui, OPTIONS[state.option], 118.0, lead);
                crate::right::menu_popup(&response).show(|ui| {
                    for (index, label) in OPTIONS.iter().enumerate() {
                        if crate::right::menu_check(ui, state.option == index, label, "") {
                            state.option = index;
                            ui.close();
                        }
                    }
                });
            }
        });
        let response = widgets::dropdown(ui, "No leading art", 160.0, None);
        crate::right::menu_popup(&response).show(|ui| {
            crate::right::menu_item(ui, "Dropdown item", "");
        });
    });
}

fn segmented(ui: &mut Ui, state: &mut State) {
    example(ui, "Text segments", "Selected and unselected choices use the same production control.", |ui| {
        ui.set_max_width(320.0);
        if let Some(index) = widgets::segmented_text(ui, &["Design", "Prototype", "Inspect"], state.segment) {
            state.segment = index;
        }
    });
    example(ui, "Icon segments", "The active cell is raised; hover shows the full label.", |ui| {
        if let Some(index) = widgets::segmented(
            ui,
            &[(Icon::AlignLeft, "Left"), (Icon::AlignHCenter, "Center"), (Icon::AlignRight, "Right")],
            Some(state.icon_segment),
            38.0,
        ) {
            state.icon_segment = index;
        }
    });
}

fn choices(ui: &mut Ui, state: &mut State) {
    example(ui, "Checkbox", "Both checked and unchecked states are interactive.", |ui| {
        if widgets::checkbox(ui, state.checked, "Show rulers") {
            state.checked = !state.checked;
        }
        if widgets::checkbox(ui, state.checked_other, "Unchecked by default") {
            state.checked_other = !state.checked_other;
        }
    });
    example(ui, "Radio", "Only one item in the group is selected.", |ui| {
        for (index, label) in ["Scale", "Crop", "Tile"].iter().enumerate() {
            if widgets::radio(ui, state.radio == index, label) {
                state.radio = index;
            }
        }
    });
}

fn paint(ui: &mut Ui) {
    example(ui, "Paint swatches", "Opaque, translucent, image, and pattern fills use widgets::swatch.", |ui| {
        ui.horizontal(|ui| {
            for (name, sample) in [
                ("Solid", figcraft_color::Paint::solid(figcraft_color::Color::new(0.08, 0.58, 0.96, 1.0))),
                ("Alpha", figcraft_color::Paint::solid(figcraft_color::Color::new(0.95, 0.35, 0.40, 0.45))),
                ("Pattern", figcraft_color::Paint::pattern(1)),
                ("Image", figcraft_color::Paint::image("sample", figcraft_color::ImageScaleMode::Fill)),
            ] {
                ui.vertical(|ui| {
                    widgets::swatch(ui, &sample, 32.0);
                    ui.small(name);
                });
            }
        });
    });
    example(ui, "Transparency checker", "The same checkerboard is drawn under transparent colors.", |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(140.0, 42.0), Sense::hover());
        widgets::checker(ui.painter(), rect, 8.0);
        ui.painter().rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, Tokens::get(ui.ctx()).border), StrokeKind::Inside);
    });
}

fn sliders(ui: &mut Ui, state: &mut State) {
    example(ui, "Slider", "Drag the knob or click the track. The second sample shows a mixed value.", |ui| {
        let (_, change) = widgets::slider(ui, Id::new("library_slider"), "Opacity", (0.0, 100.0), Some(50.0), Some(state.slider), 250.0);
        if let Some((next, _)) = change {
            state.slider = next;
        }
        ui.label(format!("{:.0}%", state.slider));
        widgets::slider(ui, Id::new("library_slider_mixed"), "Mixed opacity", (0.0, 100.0), None, None, 250.0);
    });
    example(ui, "Progress", "Static progress indicators share the accent and track colors.", |ui| {
        for fraction in [0.0, 0.35, 0.75, 1.0] {
            widgets::progress_bar(ui, "Progress", fraction, 250.0);
        }
    });
}

fn structure(ui: &mut Ui, state: &mut State) {
    example(ui, "Section header", "Regular and dimmed headings; right-side actions are live.", |ui| {
        widgets::section_header(ui, "Fill", &[(Icon::Styles, "Styles"), (Icon::Plus, "Add fill")], false);
        widgets::separator(ui);
        widgets::section_header(ui, "Empty section", &[(Icon::Plus, "Add")], true);
    });
    example(ui, "Disclosure", "Click the chevron row to expand or collapse its content.", |ui| {
        if widgets::disclosure(ui, "Advanced options", state.disclosure) {
            state.disclosure = !state.disclosure;
        }
        if state.disclosure {
            widgets::note(ui, "Additional controls appear here.", Tokens::get(ui.ctx()).text_secondary);
        }
    });
    example(ui, "Floating bar", "A panel-colored container with the production border and shadow.", |ui| {
        widgets::bar(ui, |ui| {
            widgets::icon_button(ui, Icon::Hand, "Pan");
            widgets::button(ui, "100%", false, true);
        });
    });
}

fn menus(ui: &mut Ui, state: &mut State) {
    example(ui, "Menu popup", "The same popup, row, checkmark, and shortcut styles as the property panel.", |ui| {
        let response = widgets::dropdown(ui, "Open menu", 170.0, None);
        crate::right::menu_popup(&response).show(|ui| {
            if crate::right::menu_item(ui, "Add item", "⌘N") {
                ui.close();
            }
            ui.separator();
            for (index, label) in ["First choice", "Second choice"].iter().enumerate() {
                if crate::right::menu_check(ui, state.menu_choice == index, label, "") {
                    state.menu_choice = index;
                    ui.close();
                }
            }
            crate::right::menu_item_beside_checks(ui, "Manage choices", "");
        });
    });
}

fn typography(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    example(ui, "Panel text", "Primary, secondary, and tertiary text styles.", |ui| {
        widgets::label(ui, "Primary label · 11 px", t.text);
        widgets::label(ui, "Secondary label · 11 px", t.text_secondary);
        widgets::label(ui, "Tertiary label · 11 px", t.text_tertiary);
    });
    example(ui, "Field labels", "A single heading or a pair over two columns.", |ui| {
        widgets::sublabels(ui, "Position", None);
        widgets::sublabels(ui, "Width", Some("Height"));
    });
    example(ui, "Note", "Long explanatory text wraps inside the panel padding.", |ui| {
        widgets::note(
            ui,
            "This is the same muted note used for empty states and property hints. It wraps as the available width changes.",
            t.text_secondary,
        );
    });
}

fn iconography(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    example(ui, "General icons", "Every Icon::ALL variant, rendered from the production vector paths.", |ui| {
        ui.horizontal_wrapped(|ui| {
            for icon in Icon::ALL {
                let (rect, response) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                if response.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(5), t.hover);
                }
                icons::paint(ui.painter(), rect.shrink(7.0), *icon, t.icon);
                response.on_hover_text(format!("{icon:?}"));
            }
        });
    });
    example(ui, "Toolbar", "All 12 tool and mode icons shown at their normal toolbar scale.", |ui| {
        ui.horizontal_wrapped(|ui| {
            for index in 0..12 {
                let (rect, response) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                icons::paint_toolbar(ui.painter(), rect.shrink(7.0), index, t.icon);
                response.on_hover_text(format!("Tool {}", index + 1));
            }
        });
    });
    specialized_icons(ui);
}

fn specialized_icons(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    example(ui, "Panel and navigation icons", "Dedicated SVG families used in the header, properties, layers, navigation, and Agent UI.", |ui| {
        let mut row = |name: &str, icons: usize, paint: &dyn Fn(&egui::Painter, Rect, usize)| {
            ui.label(egui::RichText::new(name).size(11.0).color(t.text_secondary));
            ui.horizontal_wrapped(|ui| {
                for index in 0..icons {
                    let (rect, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                    paint(ui.painter(), rect.shrink(7.0), index);
                }
            });
        };
        let header = [
            HeaderIcon::Present,
            HeaderIcon::ZoomChevron,
            HeaderIcon::AvatarRing,
            HeaderIcon::CreateComponent,
            HeaderIcon::Mask,
            HeaderIcon::MaskTertiary,
            HeaderIcon::Union,
            HeaderIcon::UnionTertiary,
            HeaderIcon::EditObject,
            HeaderIcon::SelectMatching,
        ];
        row("Header", header.len(), &|p, r, i| icons::paint_header(p, r, header[i], t.icon));
        let position = [
            PositionIcon::AlignLeft,
            PositionIcon::AlignHCenter,
            PositionIcon::AlignRight,
            PositionIcon::AlignTop,
            PositionIcon::AlignVCenter,
            PositionIcon::AlignBottom,
            PositionIcon::Rotation,
            PositionIcon::RotateRight,
            PositionIcon::FlipH,
            PositionIcon::FlipV,
        ];
        row("Position", position.len(), &|p, r, i| icons::paint_position(p, r, position[i], t.icon, t.icon_secondary));
        let layout = [
            LayoutIcon::AspectRatio,
            LayoutIcon::AutoLayout,
            LayoutIcon::AutoLayoutActive,
            LayoutIcon::Free,
            LayoutIcon::Vertical,
            LayoutIcon::Horizontal,
            LayoutIcon::Grid,
            LayoutIcon::Wrap,
            LayoutIcon::GapVertical,
            LayoutIcon::Settings,
            LayoutIcon::PaddingHorizontal,
            LayoutIcon::PaddingVertical,
            LayoutIcon::PaddingIndividual,
            LayoutIcon::PaddingIndividualActive,
            LayoutIcon::PaddingLeft,
            LayoutIcon::PaddingRight,
            LayoutIcon::PaddingTop,
            LayoutIcon::PaddingBottom,
        ];
        row("Layout", layout.len(), &|p, r, i| icons::paint_layout(p, r, layout[i], t.icon));
        let appearance = [
            AppearanceIcon::Visible,
            AppearanceIcon::Blend,
            AppearanceIcon::Opacity,
            AppearanceIcon::Corners,
            AppearanceIcon::CornerTopLeft,
            AppearanceIcon::CornerTopRight,
            AppearanceIcon::CornerBottomLeft,
            AppearanceIcon::CornerBottomRight,
            AppearanceIcon::CornerSettings,
        ];
        row("Appearance", appearance.len(), &|p, r, i| icons::paint_appearance(p, r, appearance[i], t.icon));
        let sizing = [
            SizingIcon::Fixed,
            SizingIcon::Hug,
            SizingIcon::Fill,
            SizingIcon::Check,
            SizingIcon::Minimum,
            SizingIcon::Maximum,
            SizingIcon::Variable,
            SizingIcon::HeightFixed,
            SizingIcon::HeightHug,
            SizingIcon::HeightFill,
            SizingIcon::HeightMinimum,
            SizingIcon::HeightMaximum,
        ];
        row("Sizing", sizing.len(), &|p, r, i| icons::paint_sizing(p, r, sizing[i], t.icon));
        let layer = [
            LayerIcon::Frame,
            LayerIcon::LayoutH,
            LayerIcon::LayoutV,
            LayerIcon::LayoutGrid,
            LayerIcon::Text,
            LayerIcon::Absolute,
            LayerIcon::ChevronDown,
            LayerIcon::ChevronRight,
        ];
        row("Layers", layer.len(), &|p, r, i| icons::paint_layer(p, r, layer[i], t.icon));
        let nav = [NavIcon::Logo, NavIcon::File, NavIcon::Agent, NavIcon::Assets, NavIcon::Variables];
        row("Navigation", nav.len(), &|p, r, i| icons::paint_nav(p, r, nav[i], t.icon));
        let agent = [
            AgentIcon::Back,
            AgentIcon::More,
            AgentIcon::Emblem,
            AgentIcon::Polish,
            AgentIcon::Generate,
            AgentIcon::Plugins,
            AgentIcon::Chevron,
            AgentIcon::Add,
            AgentIcon::Library,
            AgentIcon::Microphone,
            AgentIcon::Send,
            AgentIcon::Paperclip,
            AgentIcon::Lightning,
            AgentIcon::Grid,
            AgentIcon::Down,
            AgentIcon::Up,
            AgentIcon::SendUp,
        ];
        row("Agent", agent.len(), &|p, r, i| icons::paint_agent(p, r, agent[i], t.icon));
        for (name, count, paint) in [
            ("Color picker", 17, icons::paint_colorpicker as fn(&egui::Painter, Rect, usize, Color32)),
            ("Properties", 4, icons::paint_property),
            ("Stroke sides", 5, icons::paint_stroke_side),
            ("Panel reference", 78, icons::paint_panel_reference),
        ] {
            row(name, count, &|p, r, i| paint(p, r, i, t.icon));
        }
    });
}

fn tokens(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    example(ui, "Theme palette", "Current app theme. Colors are read from theme::Tokens at draw time.", |ui| {
        for (name, color) in [
            ("Panel", t.panel),
            ("Canvas", t.canvas),
            ("Field", t.field),
            ("Hover", t.hover),
            ("Border", t.border),
            ("Accent", t.accent),
            ("Text", t.text),
            ("Secondary", t.text_secondary),
            ("Success", t.success),
            ("Warning", t.warning),
            ("Danger", t.danger),
        ] {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(26.0, 20.0), Sense::hover());
                ui.painter().rect_filled(rect, CornerRadius::same(4), color);
                ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
                ui.label(format!("{name}  #{:02X}{:02X}{:02X}", color.r(), color.g(), color.b()));
            });
        }
    });
}
