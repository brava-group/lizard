//! Standalone catalog. Previews use Workbench UI controls and Lizard compositions.

use egui::{Color32, CornerRadius, Id, Layout, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use lizard_doc::NodeId;
use lizard_engine::Session;
use lizard_ui_egui::icons::{self, AgentIcon, AppearanceIcon, HeaderIcon, Icon, LayerIcon, LayoutIcon, NavIcon, PositionIcon, SizingIcon};
use lizard_ui_egui::primitives::TooltipResponseExt as _;
use lizard_ui_egui::primitives::{
    Button, ButtonAlign, ButtonBehavior, ButtonSize, ButtonVariant, CodeBlock, CodeLanguage, ControlWidth, SearchField, Switch, TextFieldWithPresets,
};
use lizard_ui_egui::theme::{self, ALL_COLOR_ROLES, ColorRole, Tokens};
use lizard_ui_egui::widgets::{self, Empty, FieldEdit, Lead};
use lizard_ui_egui::{LeftTab, LizardApp, RightTab};
use serde_json::json;

#[path = "cursor_playground.rs"]
mod cursor_playground;
#[path = "theme_editor.rs"]
mod theme_editor;

use workbench_ui::ThemeRecipe;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Buttons,
    Numeric,
    Text,
    CodeBlock,
    Select,
    FloatingPanel,
    Segmented,
    Choices,
    Switch,
    Paint,
    ColorControl,
    PropertyController,
    Tooltip,
    Shortcut,
    Sliders,
    PrimitiveTabs,
    Shimmer,
    Surface,
    Structure,
    Menus,
    MainMenu,
    ActionsPalette,
    SharePopover,
    Typography,
    Icons,
    Theme,
    ThemeEditor,
    Tokens,
    CanvasFeedback,
    CursorPlayground,
    Tabs,
    Toolbar,
    Layers,
    Assets,
    Agent,
    AgentConversation,
    AgentRunning,
    AgentHistory,
    Versions,
    Home,
    CanvasProperties,
    RectangleProperties,
    FrameProperties,
    AutoLayoutProperties,
    TextProperties,
    VectorProperties,
    ComponentProperties,
    MultiProperties,
    PropHeader,
    PropPosition,
    PropLayout,
    PropAppearance,
    PropTypography,
    PropFills,
    PropStrokes,
    PropEffects,
    PropSelectionColors,
    PropGrids,
    PropExport,
    PropComponent,
    PropVariableModes,
    PropScale,
    Effects,
    EffectDropShadow,
    EffectInnerShadow,
    EffectLayerBlur,
    EffectBackgroundBlur,
    EffectGlass,
    EffectNoise,
    Prototype,
    Inspect,
    FramePresets,
    Comments,
    ColorPicker,
    GradientPicker,
    ImagePicker,
    PatternPicker,
    ColorLibrary,
    Variables,
    Shortcuts,
    Plugins,
    Preferences,
    About,
    ExportDialog,
    FindReplace,
    BatchRename,
    LinkDialog,
    ImportReport,
}

const PAGES: &[(Page, &str, &str, &str)] = &[
    (Page::Buttons, "Primitives", "Buttons", "primitives::Button · widgets::button · icon_button"),
    (Page::Numeric, "Primitives", "Number fields", "primitives::NumberField · sizing · binding · width"),
    (Page::Text, "Primitives", "Text fields", "text_field · TextFieldWithPresets · rename · multiline"),
    (Page::CodeBlock, "Primitives", "Code block", "primitives::CodeBlock · line numbers · WGSL syntax · editable"),
    (Page::Select, "Primitives", "Select", "primitives::Select · overlay / popup · width · ghost"),
    (Page::FloatingPanel, "Primitives", "Floating panel", "primitives::FloatingPanel · draggable · width / height"),
    (Page::Segmented, "Primitives", "Segmented", "segmented · segmented_text"),
    (Page::Choices, "Primitives", "Choices", "checkbox · radio"),
    (Page::Switch, "Primitives", "Switch", "primitives::Switch · standalone / embedded · animated"),
    (Page::Paint, "Primitives", "Paint", "swatch · checker"),
    (Page::ColorControl, "Primitives", "Color control", "primitives::ColorControl · swatch / models / opacity · width"),
    (Page::PropertyController, "Primitives", "Property controller", "primitives::PropertyController · component properties · usage indicator"),
    (Page::Tooltip, "Primitives", "Tooltip", "primitives::Tooltip · title · shortcut · description"),
    (Page::Shortcut, "Primitives", "Shortcut", "primitives::Shortcut · keyboard font · badge"),
    (Page::Sliders, "Primitives", "Sliders", "slider · progress_bar"),
    (Page::PrimitiveTabs, "Primitives", "Tabs", "primitives::Tabs · selection · content · overflow"),
    (Page::Shimmer, "Primitives", "Shimmer text", "primitives::ShimmerText · direction · duration · playing"),
    (Page::Surface, "Primitives", "Surface", "primitives::Surface · menu / palette · dual shadows"),
    (Page::Structure, "Primitives", "Sections", "section_header · disclosure · bar"),
    (Page::Menus, "Primitives", "Menu", "primitives::Menu · MenuItem · property popovers"),
    (Page::Typography, "Primitives", "Typography", "label · note · sublabels"),
    (Page::Icons, "Primitives", "Iconography", "all production icon families"),
    (Page::Theme, "Primitives", "Theme", "theme::Tokens · light and dark previews"),
    (Page::ThemeEditor, "Primitives", "Theme editor", "workbench_ui::ThemeRecipe · live preview · JSON / CSS export"),
    (Page::Tokens, "Primitives", "Color tokens", "theme::ColorRole · semantic palette"),
    (Page::MainMenu, "Compositions", "App menu", "menus::popups · main menu"),
    (Page::ActionsPalette, "Compositions", "Actions palette", "menus::popups · command search"),
    (Page::SharePopover, "Compositions", "Share popover", "live::share_popover"),
    (Page::CanvasFeedback, "Compositions", "Canvas feedback", "canvas_banner · insertion_line · drag_ghost"),
    (Page::CursorPlayground, "Compositions", "Cursor playground", "existing PNGs · original vector proposals · click points"),
    (Page::Tabs, "Workspace", "Document tabs", "tabs::tab_bar"),
    (Page::Toolbar, "Workspace", "Tool toolbar", "toolbar::show"),
    (Page::Layers, "Workspace", "Pages and layers", "layers::left_panel"),
    (Page::Assets, "Workspace", "Assets", "assets_panel::show"),
    (Page::Agent, "Workspace", "Agent chat", "agent_panel::show"),
    (Page::AgentConversation, "Workspace", "Agent conversation", "agent_panel::show · transcript and tools"),
    (Page::AgentRunning, "Workspace", "Agent working", "agent_panel::show · active turn"),
    (Page::AgentHistory, "Workspace", "Agent history", "agent_panel::show · conversation list"),
    (Page::Versions, "Workspace", "Versions", "versions::panel"),
    (Page::Home, "Workspace", "Home", "home::show"),
    (Page::CanvasProperties, "Properties", "Page", "right::right_panel · no selection"),
    (Page::RectangleProperties, "Properties", "Rectangle", "right::right_panel · rectangle"),
    (Page::FrameProperties, "Properties", "Frame", "right::right_panel · frame"),
    (Page::AutoLayoutProperties, "Properties", "Auto layout", "right::right_panel · auto layout"),
    (Page::TextProperties, "Properties", "Text", "right::right_panel · text"),
    (Page::VectorProperties, "Properties", "Vector", "right::right_panel · vector"),
    (Page::ComponentProperties, "Properties", "Component", "right::right_panel · component"),
    (Page::MultiProperties, "Properties", "Multiple selection", "right::right_panel · mixed values"),
    (Page::PropHeader, "Sections", "Layer header", "right::catalog_section · header"),
    (Page::PropPosition, "Sections", "Position", "right::catalog_section · position"),
    (Page::PropLayout, "Sections", "Auto layout controls", "right::catalog_section · layout"),
    (Page::PropAppearance, "Sections", "Appearance", "right::catalog_section · appearance"),
    (Page::PropTypography, "Sections", "Text settings", "right::catalog_section · typography"),
    (Page::PropFills, "Sections", "Fill rows", "right::catalog_section · fills"),
    (Page::PropStrokes, "Sections", "Stroke rows", "right::catalog_section · strokes"),
    (Page::PropEffects, "Sections", "Effect rows", "right::catalog_section · effects"),
    (Page::PropSelectionColors, "Sections", "Selection colors", "right::catalog_section · selection colors"),
    (Page::PropGrids, "Sections", "Layout grids", "right::catalog_section · grids"),
    (Page::PropExport, "Sections", "Export settings", "right::catalog_section · export"),
    (Page::PropComponent, "Sections", "Component settings", "right::catalog_section · component"),
    (Page::PropVariableModes, "Sections", "Variable modes", "right::catalog_section · variable modes"),
    (Page::PropScale, "Sections", "Scale controls", "right::catalog_section · scale"),
    (Page::Effects, "Properties", "Effects", "right::right_panel · six effect types"),
    (Page::EffectDropShadow, "Properties", "Drop shadow", "right::right_panel · drop shadow"),
    (Page::EffectInnerShadow, "Properties", "Inner shadow", "right::right_panel · inner shadow"),
    (Page::EffectLayerBlur, "Properties", "Layer blur", "right::right_panel · layer blur"),
    (Page::EffectBackgroundBlur, "Properties", "Background blur", "right::right_panel · background blur"),
    (Page::EffectGlass, "Properties", "Glass", "right::right_panel · glass"),
    (Page::EffectNoise, "Properties", "Noise", "right::right_panel · noise"),
    (Page::Prototype, "Properties", "Prototype", "right::right_panel · prototype tab"),
    (Page::Inspect, "Properties", "Inspect", "right::right_panel · inspect tab"),
    (Page::FramePresets, "Properties", "Frame presets", "frame_presets::panel"),
    (Page::Comments, "Properties", "Comments", "comments::panel"),
    (Page::ColorPicker, "Popovers", "Color picker", "colorpicker::show"),
    (Page::GradientPicker, "Popovers", "Gradient editor", "colorpicker::show · gradient"),
    (Page::ImagePicker, "Popovers", "Image editor", "colorpicker::show · image"),
    (Page::PatternPicker, "Popovers", "Pattern editor", "colorpicker::show · pattern"),
    (Page::ColorLibrary, "Popovers", "Color libraries", "colorpicker::show · libraries tab"),
    (Page::Variables, "Windows", "Variables", "variables_panel::show"),
    (Page::Shortcuts, "Windows", "Shortcuts", "shortcuts_panel::show"),
    (Page::Plugins, "Windows", "Plugins", "plugins_ui::show"),
    (Page::Preferences, "Windows", "Preferences", "dialogs::show"),
    (Page::About, "Windows", "About", "dialogs::show"),
    (Page::ExportDialog, "Windows", "Export", "dialogs::show · export preview"),
    (Page::FindReplace, "Windows", "Find and replace", "dialogs::show · search"),
    (Page::BatchRename, "Windows", "Rename layers", "dialogs::show · batch rename"),
    (Page::LinkDialog, "Windows", "Create link", "dialogs::show · menu input"),
    (Page::ImportReport, "Windows", "Import report", "import_report::show"),
];

pub struct State {
    page: Page,
    last_page: Option<Page>,
    dark: bool,
    query: String,
    clicks: usize,
    token_search: String,
    theme_preview_text: String,
    theme_recipe: ThemeRecipe,
    theme_picker_open: bool,
    theme_picker_anchor: egui::Pos2,
    theme_export_css: bool,
    theme_export_message: String,
    applied_theme: Option<(bool, ThemeRecipe)>,
    button_toggle: bool,
    button_selection: usize,
    number: f64,
    bound_number: f64,
    bound: bool,
    text: String,
    draft: String,
    multiline: String,
    code_sample: String,
    combo: String,
    text_presets: String,
    select_choice: usize,
    sizing_choice: usize,
    floating_open: [bool; 3],
    segment: usize,
    segment_auto: usize,
    icon_segment: usize,
    checked: bool,
    checked_other: bool,
    switch_primary: bool,
    switch_secondary: bool,
    switch_embedded: bool,
    radio: usize,
    slider: f64,
    slider_classic_default: f64,
    slider_classic_delta: f64,
    slider_classic_left: f64,
    slider_variants: [f64; 5],
    shimmer_playing: bool,
    tab_choice: usize,
    tab_content_choice: usize,
    tab_overflow_buttons: usize,
    tab_overflow_fade: usize,
    disclosure: bool,
    menu_choice: usize,
    property_selection: Option<usize>,
    property_binding_attached: bool,
    property_binding_located: bool,
    property_names: [String; 7],
    property_visible: [bool; 7],
    cursor_playground: cursor_playground::State,
}

impl Default for State {
    fn default() -> Self {
        Self {
            page: Page::Buttons,
            last_page: None,
            dark: true,
            query: String::new(),
            clicks: 0,
            token_search: String::new(),
            theme_preview_text: "Editable text".into(),
            theme_recipe: ThemeRecipe::default(),
            theme_picker_open: false,
            theme_picker_anchor: pos2(360.0, 150.0),
            theme_export_css: false,
            theme_export_message: String::new(),
            applied_theme: None,
            button_toggle: false,
            button_selection: 0,
            number: 100.0,
            bound_number: 64.0,
            bound: true,
            text: "Component name".into(),
            draft: String::new(),
            multiline: "A note with more than one line.".into(),
            code_sample:
                "// Edit this WGSL source\nfn tint(uv: vec2f) -> vec4f {\n    let pulse: f32 = 0.75;\n    return vec4f(uv.x, pulse, uv.y, 1.0);\n}"
                    .into(),
            combo: "1x".into(),
            text_presets: "Hover".into(),
            select_choice: 1,
            sizing_choice: 0,
            floating_open: [true, false, false],
            segment: 0,
            segment_auto: 1,
            icon_segment: 0,
            checked: true,
            checked_other: false,
            switch_primary: true,
            switch_secondary: false,
            switch_embedded: true,
            radio: 0,
            slider: 42.0,
            slider_classic_default: 0.0,
            slider_classic_delta: 2.5,
            slider_classic_left: 0.0,
            slider_variants: [50.0, 180.0, 75.0, 0.0, 50.0],
            shimmer_playing: true,
            tab_choice: 0,
            tab_content_choice: 0,
            tab_overflow_buttons: 0,
            tab_overflow_fade: 0,
            disclosure: true,
            menu_choice: 0,
            property_selection: None,
            property_binding_attached: true,
            property_binding_located: false,
            property_names: [
                "Property 1".into(),
                "Text".into(),
                "Boolean".into(),
                "Instance".into(),
                "Slot".into(),
                "Label".into(),
                "Content".into(),
            ],
            property_visible: [true; 7],
            cursor_playground: cursor_playground::State::default(),
        }
    }
}

impl State {
    pub fn select_page(&mut self, label: &str) -> bool {
        let Some((page, ..)) = PAGES.iter().find(|(_, _, name, _)| name.eq_ignore_ascii_case(label)) else { return false };
        self.page = *page;
        true
    }
}

/// Synthetic document used only by the catalog's process.
pub struct Fixture {
    frame: Option<u64>,
    auto: Option<u64>,
    rect: Option<u64>,
    color: Option<u64>,
    bound_color: Option<u64>,
    bound_color_variable: Option<String>,
    text: Option<u64>,
    vector: Option<u64>,
    component: Option<u64>,
    layer_tree: Vec<u64>,
    mask_group: Option<u64>,
    effects: Option<u64>,
    effect_nodes: [Option<u64>; 6],
    gradient: Option<u64>,
    image: Option<u64>,
    pattern: Option<u64>,
}

impl Fixture {
    pub fn new(session: &mut Session) -> Self {
        fn add(session: &mut Session, command: &str, params: serde_json::Value) -> Option<u64> {
            session.execute(command, &params).ok()?.get("id")?.as_u64()
        }
        let frame = add(session, "frame.create", json!({"x": 50, "y": 50, "width": 480, "height": 320, "name": "Example frame"}));
        let auto = add(
            session,
            "frame.create",
            json!({"x": 600, "y": 50, "width": 360, "height": 240, "name": "Auto layout", "props": {"layout": "horizontal", "padding": 16, "itemSpacing": 12}}),
        );
        let rect =
            add(session, "shape.rectangle", json!({"x": 80, "y": 90, "width": 140, "height": 96, "name": "Card", "props": {"fill": "#0D99FF"}}));
        let color = add(
            session,
            "shape.rectangle",
            json!({"x": 1500, "y": 50, "width": 140, "height": 96, "name": "Color control", "props": {"fill": "#0C8CE9"}}),
        );
        let bound_color = add(
            session,
            "shape.rectangle",
            json!({"x": 1500, "y": 170, "width": 140, "height": 96, "name": "Bound color control", "props": {"fill": "#FF383C"}}),
        );
        if let Some(id) = rect {
            let _ = session.execute("stroke.add", &json!({"ids": [id], "color": "#FFFFFF"}));
            let _ = session.execute("fill.add", &json!({"ids": [id], "color": "#D277FF80"}));
        }
        if let Some(id) = frame {
            let _ = session.execute("grid.add", &json!({"ids": [id], "grid": {"kind": "columns", "count": 4, "gutter": 16}}));
        }
        let text = add(session, "text.create", json!({"x": 80, "y": 215, "text": "Lizard component", "name": "Heading", "props": {"fontSize": 24}}));
        let vector = add(session, "vector.create", json!({"points": [[0, 0], [90, 0], [45, 76]], "closed": true, "name": "Triangle"}));
        let component_frame = add(session, "frame.create", json!({"x": 1000, "y": 50, "width": 180, "height": 80, "name": "Button"}));
        let component = component_frame.and_then(|id| add(session, "component.create", json!({"ids": [id]})));
        let effects = add(
            session,
            "shape.rectangle",
            json!({"x": 1000, "y": 200, "width": 180, "height": 120, "name": "Effects", "props": {"fill": "#D277FF"}}),
        );
        if let Some(id) = effects {
            for kind in ["dropShadow", "innerShadow", "layerBlur", "backgroundBlur", "glass", "noise"] {
                let _ = session.execute("effect.add", &json!({"ids": [id], "type": kind}));
            }
        }
        let effect_nodes = std::array::from_fn(|index| {
            let kind = ["dropShadow", "innerShadow", "layerBlur", "backgroundBlur", "glass", "noise"][index];
            let id = add(
                session,
                "shape.rectangle",
                json!({"x": 1000, "y": 360 + index * 160, "width": 180, "height": 120, "name": kind, "props": {"fill": "#D277FF"}}),
            );
            if let Some(id) = id {
                let _ = session.execute("effect.add", &json!({"ids": [id], "type": kind}));
            }
            id
        });
        let gradient = add(session, "shape.rectangle", json!({"x": 1240, "y": 50, "width": 180, "height": 120, "name": "Gradient"}));
        if let Some(id) = gradient {
            let _ = session.execute("node.set", &json!({"ids": [id], "fills": [{"type": "linearGradient", "stops": ["#0D99FF", "#D277FF"]}]}));
        }
        let image = add(session, "shape.rectangle", json!({"x": 1240, "y": 210, "width": 180, "height": 120, "name": "Image fill"}));
        if let Some(id) = image {
            let _ = session.execute("fill.set", &json!({"ids": [id], "paint": {"type": "image", "image": "catalog-sample"}}));
        }
        let pattern = add(session, "shape.rectangle", json!({"x": 1240, "y": 370, "width": 180, "height": 120, "name": "Pattern fill"}));
        if let (Some(id), Some(source)) = (pattern, rect) {
            let _ = session.execute("fill.set", &json!({"ids": [id], "paint": {"type": "pattern", "source": source}}));
        }
        let _ = session.execute("variable.createCollection", &json!({"name": "Theme", "modes": ["Light", "Dark"]}));
        let _ = session.execute("variable.create", &json!({"collection": "Theme", "name": "brand", "type": "color", "value": "#0D99FF"}));
        let bound_color_variable = session
            .execute("variable.create", &json!({"collection": "Theme", "name": "Accents/Red", "type": "color", "value": "#FF383C"}))
            .ok()
            .and_then(|value| value.get("id").and_then(serde_json::Value::as_str).map(str::to_owned));
        if let (Some(id), Some(variable)) = (bound_color, &bound_color_variable) {
            let _ = session.execute("variable.bind", &json!({"ids": [id], "field": "fill", "variable": variable}));
        }
        let _ = session.execute("variable.create", &json!({"collection": "Theme", "name": "spacing", "type": "number", "value": 16}));
        let _ = session.execute("style.create", &json!({"type": "paint", "name": "Brand/Blue", "value": [{"type": "solid", "color": "#0D99FF"}]}));
        let _ = session.execute("style.create", &json!({"type": "paint", "name": "Brand/Pink", "value": [{"type": "solid", "color": "#D277FF"}]}));
        let _ = session.execute("style.create", &json!({"type": "text", "name": "Type/Body", "value": {"fontFamily": "Inter", "fontSize": 14}}));
        // Real document nodes exercise the production Layers tree, including its type icons.
        if let Some(child) = add(session, "shape.rectangle", json!({"x": 1760, "y": 1650, "width": 96, "height": 96})) {
            let _ = add(session, "object.group", json!({"ids": [child], "name": "Group 3"}));
        }
        if let Some(component) = component
            && let Some(id) = add(session, "instance.create", json!({"component": component, "x": 1760, "y": 1530}))
        {
            let _ = session.execute("node.set", &json!({"id": id, "props": {"name": "Component Instance"}}));
        }
        for (name, direction, y) in
            [("vertical auto layout", "vertical", 1410), ("grid auto layout", "grid", 1290), ("horizontal auto layout", "horizontal", 1170)]
        {
            if let Some(id) = add(session, "frame.create", json!({"x": 1760, "y": y, "width": 160, "height": 96, "name": name})) {
                let _ = add(session, "shape.rectangle", json!({"x": 8, "y": 8, "width": 32, "height": 32, "parent": id}));
                let _ = session.execute("layout.add", &json!({"ids": [id], "direction": direction}));
            }
        }
        let mask = add(session, "shape.ellipse", json!({"x": 1760, "y": 1050, "width": 96, "height": 96, "name": "Ellipse 4"}));
        let masked_rectangle = add(session, "shape.rectangle", json!({"x": 1770, "y": 1040, "width": 96, "height": 96, "name": "Rectangle 21"}));
        let masked_ellipse_a = add(session, "shape.ellipse", json!({"x": 1780, "y": 1030, "width": 96, "height": 96, "name": "mask children"}));
        let masked_ellipse_b = add(session, "shape.ellipse", json!({"x": 1790, "y": 1020, "width": 96, "height": 96, "name": "mask children"}));
        let mask_group = match (mask, masked_rectangle, masked_ellipse_a, masked_ellipse_b) {
            (Some(mask), Some(rectangle), Some(ellipse_a), Some(ellipse_b)) => {
                add(session, "object.mask", json!({"ids": [mask, rectangle, ellipse_a, ellipse_b]}))
            }
            _ => None,
        };
        // Keep the Pencil layer-icon examples at the top of the production Layers preview.
        let _ = add(session, "shape.line", json!({"x": 1760, "y": 800, "width": 96, "name": "Line 1"}));
        let _ = add(session, "shape.arrow", json!({"x": 1760, "y": 680, "width": 96, "name": "Arrow 2"}));
        let _ = add(session, "shape.ellipse", json!({"x": 1760, "y": 560, "width": 96, "height": 96, "name": "Ellipse 3"}));
        let _ = add(session, "shape.polygon", json!({"x": 1760, "y": 440, "width": 96, "height": 96, "name": "Polygon 1"}));
        let _ = add(session, "shape.star", json!({"x": 1760, "y": 320, "width": 96, "height": 96, "name": "Star 1"}));
        if let Some(id) = add(session, "shape.rectangle", json!({"x": 1760, "y": 200, "width": 96, "height": 96, "name": "image"})) {
            let _ = session.execute("fill.set", &json!({"ids": [id], "paint": {"type": "image", "image": "catalog-sample"}}));
        }
        let layer_tree = add(session, "frame.create", json!({"x": 1760, "y": 1850, "width": 160, "height": 80, "name": "Tree button"}))
            .and_then(|frame| {
                let text = add(session, "text.create", json!({"parent": frame, "x": 12, "y": 12, "text": "Button label", "name": "Button label"}))?;
                let group = add(session, "object.group", json!({"ids": [text], "name": "Content"}))?;
                let variant = add(session, "component.create", json!({"ids": [frame]}))?;
                add(session, "component.addVariant", json!({"id": variant}))?;
                let set = session.doc().ok()?.node(NodeId(variant))?.parent?;
                Some(vec![set.0, variant, group])
            })
            .unwrap_or_default();
        let _ =
            session.execute("comment.add", &json!({"x": 110, "y": 100, "text": "Please review this spacing.", "author": "Matheus", "anchor": true}));
        let _ = session.execute("version.autosave", &json!({}));
        let _ = session.execute("select.set", &json!({"ids": []}));
        Self {
            frame,
            auto,
            rect,
            color,
            bound_color,
            bound_color_variable,
            text,
            vector,
            component,
            layer_tree,
            mask_group,
            effects,
            effect_nodes,
            gradient,
            image,
            pattern,
        }
    }

    fn ids(&self, page: Page) -> Vec<u64> {
        match page {
            Page::Layers => self.layer_tree.first().copied().into_iter().collect(),
            Page::ColorControl => self.color.into_iter().collect(),
            Page::RectangleProperties
            | Page::ColorPicker
            | Page::ColorLibrary
            | Page::Prototype
            | Page::PropHeader
            | Page::PropPosition
            | Page::PropAppearance
            | Page::PropFills
            | Page::PropStrokes
            | Page::PropExport
            | Page::PropScale => self.rect.into_iter().collect(),
            Page::FrameProperties | Page::Inspect | Page::PropGrids | Page::PropVariableModes => self.frame.into_iter().collect(),
            Page::AutoLayoutProperties | Page::PropLayout => self.auto.into_iter().collect(),
            Page::TextProperties | Page::PropTypography => self.text.into_iter().collect(),
            Page::VectorProperties => self.vector.into_iter().collect(),
            Page::ComponentProperties | Page::PropComponent => self.component.into_iter().collect(),
            Page::Effects | Page::PropEffects => self.effects.into_iter().collect(),
            Page::EffectDropShadow => self.effect_nodes[0].into_iter().collect(),
            Page::EffectInnerShadow => self.effect_nodes[1].into_iter().collect(),
            Page::EffectLayerBlur => self.effect_nodes[2].into_iter().collect(),
            Page::EffectBackgroundBlur => self.effect_nodes[3].into_iter().collect(),
            Page::EffectGlass => self.effect_nodes[4].into_iter().collect(),
            Page::EffectNoise => self.effect_nodes[5].into_iter().collect(),
            Page::GradientPicker => self.gradient.into_iter().collect(),
            Page::ImagePicker => self.image.into_iter().collect(),
            Page::PatternPicker => self.pattern.into_iter().collect(),
            Page::MultiProperties | Page::PropSelectionColors => self.rect.into_iter().chain(self.text).collect(),
            _ => Vec::new(),
        }
    }
}

fn enter_page(state: &mut State, fig: &mut LizardApp, fixture: &Fixture, ctx: &egui::Context) {
    if state.last_page == Some(state.page) {
        return;
    }
    state.last_page = Some(state.page);
    state.theme_picker_open = false;
    fig.ui.picker = None;
    fig.ui.main_menu = false;
    lizard_ui_egui::menus::set_palette(&mut fig.ui, false);
    lizard_ui_egui::colorpicker::close_library_preview(ctx);
    fig.ui.style_picker = None;
    fig.ui.shortcuts_panel = false;
    fig.ui.plugins_window = false;
    fig.ui.preferences = false;
    fig.ui.about = false;
    fig.ui.export_preview = false;
    fig.ui.find = None;
    fig.ui.batch_rename = None;
    fig.ui.menu_input = None;
    fig.ui.import_report = None;
    fig.ui.variables = None;
    fig.ui.versions.open = false;
    fig.ui.agent.active = None;
    fig.ui.agent.busy = false;
    fig.ui.agent.history_open = false;
    fig.ui.left_tab = LeftTab::File;
    fig.ui.right_tab = RightTab::Design;
    let _ = fig.session.execute("select.set", &json!({"ids": fixture.ids(state.page)}));
    match state.page {
        Page::Layers => {
            if let Some(id) = fixture.mask_group {
                fig.ui.expanded.insert(NodeId(id));
            }
            for id in &fixture.layer_tree {
                fig.ui.expanded.insert(NodeId(*id));
            }
        }
        Page::Assets => fig.ui.left_tab = LeftTab::Assets,
        Page::Agent | Page::AgentConversation | Page::AgentRunning | Page::AgentHistory => {
            fig.ui.left_tab = LeftTab::Agent;
            fig.ui.agent.draft = if state.page == Page::AgentConversation {
                "Ajuste o layout deste componente e confira o resultado em telas menores. ".repeat(18)
            } else {
                String::new()
            };
            if state.page != Page::Agent {
                use lizard_ui_egui::agent_panel::{AgentKind, ChatMessage, ChatTool, Conversation};
                let key = fig.session.ds().ok().map_or_else(|| "catalog".to_owned(), |ds| format!("untitled:{}", ds.uid));
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_secs());
                let sample_age = if state.page == Page::AgentRunning { 30 * 86_400 } else { 11 * 60 };
                let user = ChatMessage {
                    role: "user".into(),
                    text: "Ajuste o layout deste componente e confira o resultado.".into(),
                    created_at: now.saturating_sub(sample_age),
                    ..Default::default()
                };
                let assistant = ChatMessage {
                    role: "assistant".into(),
                    text: "**Layout ajustado.** Verifiquei o frame e alinhei o conteúdo ao grid.".into(),
                    model_label: "GPT-6.1-Sol".into(),
                    thinking: "Vou inspecionar a estrutura e aplicar o espaçamento.".into(),
                    tools: vec![ChatTool {
                        id: "node.get".into(),
                        title: "Lizard · get_node".into(),
                        status: "completed".into(),
                        input: Some(json!({"id": 42, "depth": 1})),
                        output: Some(json!({
                            "id": 42,
                            "name": "Button",
                            "type": "component",
                            "componentPath": "Library/Components/Buttons/Variants/Primary/With icon/Default/Responsive configuration",
                            "children": (0..24).map(|index| format!("Layer {index}")).collect::<Vec<_>>()
                        })),
                        ..Default::default()
                    }],
                    work_seconds: (state.page != Page::AgentRunning).then_some(13),
                    ..Default::default()
                };
                fig.ui.agent.conversations = vec![Conversation {
                    id: 1,
                    document: key,
                    title: "Layout do componente".into(),
                    agent: AgentKind::Codex,
                    messages: vec![user, assistant],
                    remote_session: None,
                    remote_sessions: Vec::new(),
                }];
                fig.ui.agent.active = Some(1);
                fig.ui.agent.busy = state.page == Page::AgentRunning;
                fig.ui.agent.runtime_conversation = fig.ui.agent.busy.then_some(1);
                fig.ui.agent.history_open = state.page == Page::AgentHistory;
                if fig.ui.agent.busy {
                    fig.ui.agent.turn_started = Some(std::time::Instant::now());
                }
            }
        }
        Page::Versions => fig.ui.versions.open = true,
        Page::MainMenu => {
            fig.ui.main_menu = true;
            lizard_ui_egui::menus::open_main_menu(ctx, "File");
        }
        Page::ActionsPalette => lizard_ui_egui::menus::set_palette(&mut fig.ui, true),
        Page::Prototype => fig.ui.right_tab = RightTab::Prototype,
        Page::Inspect => fig.ui.right_tab = RightTab::Inspect,
        Page::ColorPicker | Page::GradientPicker | Page::ImagePicker | Page::PatternPicker | Page::ColorLibrary => {
            fig.ui.picker = Some(lizard_ui_egui::colorpicker::PickerTarget::Paint {
                fills: true,
                index: 0,
                ids: fixture.ids(state.page).into_iter().map(NodeId).collect(),
            })
        }
        Page::Variables => fig.ui.variables = Some(lizard_ui_egui::variables_panel::Editor::on(None)),
        Page::Shortcuts => fig.ui.shortcuts_panel = true,
        Page::Plugins => fig.ui.plugins_window = true,
        Page::Preferences => fig.ui.preferences = true,
        Page::About => fig.ui.about = true,
        Page::ExportDialog => fig.ui.export_preview = true,
        Page::FindReplace => fig.ui.find = Some(("card".into(), "tile".into())),
        Page::BatchRename => {
            fig.ui.batch_rename = Some(lizard_ui_egui::dialogs::BatchRenameUi {
                ids: fixture.rect.into_iter().chain(fixture.text).map(NodeId).collect(),
                start: 1,
                ..Default::default()
            })
        }
        Page::LinkDialog => fig.ui.menu_input = Some(lizard_ui_egui::dialogs::MenuInput::Link("https://example.com".into())),
        Page::ImportReport => {
            if let Ok(ds) = fig.session.ds() {
                fig.ui.import_report = Some(lizard_ui_egui::import_report::Report {
                    name: "Example import.fig".into(),
                    pages: Some(1),
                    layers: Some(7),
                    images: Some(1),
                    appended: false,
                    placed: false,
                    uid: ds.uid,
                    notes: vec!["One unsupported effect was skipped.".into()],
                    shown: vec!["One unsupported effect was skipped.".into()],
                    note_count: 1,
                });
            }
        }
        _ => {}
    }
    if state.page == Page::ColorLibrary {
        lizard_ui_egui::colorpicker::open_library_preview(ctx);
    }
    if matches!(
        state.page,
        Page::EffectDropShadow | Page::EffectInnerShadow | Page::EffectLayerBlur | Page::EffectBackgroundBlur | Page::EffectGlass | Page::EffectNoise
    ) {
        ctx.data_mut(|data| {
            data.insert_temp(Id::new(("fx_open", 0usize)), true);
            data.insert_temp(Id::new(("fx_open", "anchor")), pos2(440.0, 190.0));
        });
    }
    let tool = match state.page {
        Page::FramePresets => "frame",
        Page::PropScale => "scale",
        _ => "move",
    };
    let _ = fig.session.execute("tool.set", &json!({"tool": tool}));
}

pub fn show(ui: &mut Ui, state: &mut State, fig: &mut LizardApp, fixture: &Fixture) {
    let theme_key = (state.dark, state.theme_recipe);
    if state.applied_theme != Some(theme_key) {
        theme::apply(ui.ctx(), Tokens::from_recipe(state.dark, state.theme_recipe));
        state.applied_theme = Some(theme_key);
    }
    let t = Tokens::get(ui.ctx());
    fig.canvas.rect = ui.max_rect();
    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
    let total = ui.available_size();
    let sidebar_width = 256.0;
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(sidebar_width, total.y), Layout::top_down(egui::Align::Min), |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                ui.set_width(sidebar_width - 32.0);
                ui.set_min_height((total.y - 32.0).max(0.0));
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(egui::RichText::new("Workbench UI").font(theme::semibold(16.0)).color(t.text));
                ui.label(
                    egui::RichText::new(format!("{} component pages · local only", PAGES.len())).font(theme::regular(11.0)).color(t.text_secondary),
                );
                ui.add_space(8.0);
                if Button::new(if state.dark { "Light mode" } else { "Dark mode" })
                    .variant(ButtonVariant::Ghost)
                    .leading(if state.dark { Icon::Sun } else { Icon::Moon })
                    .align(ButtonAlign::Start)
                    .size(ButtonSize::Large)
                    .width(ControlWidth::Fill)
                    .show(ui)
                    .clicked()
                {
                    state.dark = !state.dark;
                    state.applied_theme = None;
                }
                ui.add_space(8.0);
                SearchField::new(Id::new("catalog_filter"), "Filter components").clearable().show(ui, &mut state.query);
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("catalog_nav").auto_shrink([false, false]).show(ui, |ui| {
                    let mut last_group = "";
                    for &(page, group, name, _) in PAGES {
                        if !state.query.is_empty()
                            && !name.to_lowercase().contains(&state.query.to_lowercase())
                            && !group.to_lowercase().contains(&state.query.to_lowercase())
                        {
                            continue;
                        }
                        if group != last_group {
                            last_group = group;
                            ui.add_space(11.0);
                            ui.label(egui::RichText::new(group.to_uppercase()).font(theme::semibold(10.0)).color(t.text_tertiary));
                            ui.add_space(3.0);
                        }
                        let selected = state.page == page;
                        let response = Button::new(name)
                            .variant(ButtonVariant::Navigation)
                            .behavior(ButtonBehavior::Toggle { selected })
                            .align(ButtonAlign::Start)
                            .size(ButtonSize::Large)
                            .width(ControlWidth::Fill)
                            .show(ui);
                        if selected && state.last_page != Some(page) {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                        if response.clicked() {
                            state.page = page;
                        }
                    }
                });
            });
        });
        ui.separator();
        ui.allocate_ui_with_layout(vec2((total.x - sidebar_width - 27.0).max(400.0), total.y), Layout::top_down(egui::Align::Min), |ui| {
            enter_page(state, fig, fixture, ui.ctx());
            let (name, source) =
                PAGES.iter().find(|(page, ..)| *page == state.page).map_or(("Components", ""), |(_, _, name, source)| (*name, *source));
            ui.add_space(12.0);
            ui.heading(name);
            ui.label(egui::RichText::new(source).size(11.0).color(t.text_secondary));
            ui.add_space(14.0);
            if matches!(
                state.page,
                Page::ColorPicker
                    | Page::GradientPicker
                    | Page::ImagePicker
                    | Page::PatternPicker
                    | Page::ColorLibrary
                    | Page::Variables
                    | Page::Shortcuts
                    | Page::Plugins
                    | Page::Preferences
                    | Page::About
                    | Page::ExportDialog
                    | Page::FindReplace
                    | Page::BatchRename
                    | Page::LinkDialog
                    | Page::ImportReport
            ) && ui.button("Reopen preview").clicked()
            {
                state.last_page = None;
            }
            egui::ScrollArea::vertical().id_salt("catalog_preview").show(ui, |ui| {
                ui.set_min_width((total.x - 290.0).max(380.0));
                match state.page {
                    Page::Buttons => buttons(ui, state),
                    Page::Numeric => numeric(ui, state),
                    Page::Text => text_fields(ui, state),
                    Page::CodeBlock => code_blocks(ui, state),
                    Page::Select => selects(ui, state),
                    Page::FloatingPanel => floating_panels(ui, state),
                    Page::Segmented => segmented(ui, state),
                    Page::Choices => choices(ui, state),
                    Page::Switch => switches(ui, state),
                    Page::Paint => paint(ui),
                    Page::ColorControl => color_control(ui, state, fig, fixture),
                    Page::PropertyController => property_controllers(ui, state),
                    Page::Tooltip => tooltips(ui),
                    Page::Shortcut => shortcut_primitive(ui),
                    Page::Sliders => sliders(ui, state),
                    Page::PrimitiveTabs => primitive_tabs(ui, state),
                    Page::Shimmer => shimmer(ui, state),
                    Page::Surface => surfaces(ui),
                    Page::Structure => structure(ui, state),
                    Page::Menus => menus(ui, state),
                    Page::Typography => typography(ui),
                    Page::Icons => iconography(ui),
                    Page::Theme => theme_preview(ui, state),
                    Page::ThemeEditor => theme_editor::show(
                        ui,
                        &mut state.theme_recipe,
                        &mut state.theme_picker_open,
                        &mut state.theme_picker_anchor,
                        &mut state.theme_export_css,
                        &mut state.theme_export_message,
                        &mut state.theme_preview_text,
                    ),
                    Page::Tokens => tokens(ui, state),
                    Page::CanvasFeedback => canvas_feedback(ui),
                    Page::CursorPlayground => cursor_playground::show(ui, &mut state.cursor_playground),
                    Page::SharePopover => {
                        let anchor = ui.button("Share");
                        lizard_ui_egui::live::share_popover(fig, &anchor);
                    }
                    _ => production_preview(ui, fig, state.page),
                }
            });
        });
    });
    match state.page {
        Page::ColorControl | Page::ColorPicker | Page::GradientPicker | Page::ImagePicker | Page::PatternPicker | Page::ColorLibrary => {
            lizard_ui_egui::colorpicker::show(fig, ui.ctx())
        }
        Page::Variables => lizard_ui_egui::variables_panel::show(fig, ui.ctx()),
        Page::Shortcuts => lizard_ui_egui::shortcuts_panel::show(fig, ui.ctx()),
        Page::Plugins => lizard_ui_egui::plugins_ui::show(fig, ui.ctx()),
        Page::Preferences | Page::About | Page::ExportDialog | Page::FindReplace | Page::BatchRename | Page::LinkDialog => {
            lizard_ui_egui::dialogs::show(fig, ui.ctx())
        }
        Page::ImportReport => lizard_ui_egui::import_report::show(fig, ui.ctx()),
        Page::MainMenu | Page::ActionsPalette => lizard_ui_egui::menus::popups(fig, ui.ctx()),
        _ => {}
    }
}

pub fn audit() -> Result<usize, String> {
    let mut session = Session::new();
    let fixture = Fixture::new(&mut session);
    for (name, id) in [
        ("frame", fixture.frame),
        ("auto layout", fixture.auto),
        ("rectangle", fixture.rect),
        ("color control", fixture.color),
        ("text", fixture.text),
        ("vector", fixture.vector),
        ("component", fixture.component),
        ("effects", fixture.effects),
        ("gradient", fixture.gradient),
        ("image", fixture.image),
        ("pattern", fixture.pattern),
    ] {
        if id.is_none() {
            return Err(format!("missing {name} fixture"));
        }
    }
    if fixture.effect_nodes.iter().any(Option::is_none) {
        return Err("missing effect fixture".into());
    }
    let doc = session.doc().map_err(|error| error.to_string())?;
    for (index, id) in fixture.effect_nodes.iter().enumerate() {
        if id.and_then(|id| doc.node(NodeId(id))).is_none_or(|node| node.effects.len() != 1) {
            return Err(format!("effect fixture {index} has no effect"));
        }
    }
    for (name, id, kind) in [("gradient", fixture.gradient, "gradient"), ("image", fixture.image, "image"), ("pattern", fixture.pattern, "pattern")] {
        let actual = id.and_then(|id| doc.node(NodeId(id))).and_then(|node| node.fills.first()).map(|paint| match &paint.kind {
            lizard_color::PaintKind::Gradient(_) => "gradient",
            lizard_color::PaintKind::Image(_) => "image",
            lizard_color::PaintKind::Pattern(_) => "pattern",
            _ => "other",
        });
        if actual != Some(kind) {
            return Err(format!("{name} fixture has {actual:?} fill"));
        }
    }
    let mut fig = LizardApp::new(session, Box::new(lizard_ui_egui::NoServices));
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx, true, 0);
    theme::apply(&ctx, Tokens::dark());
    let mut state = State::default();
    for &(page, _, label, _) in PAGES {
        eprintln!("Auditing {label}");
        state.page = page;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1340.0, 880.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| show(ui, &mut state, &mut fig, &fixture));
        // A headless audit has no renderer to upload the font and image textures.
        output.textures_delta.clear();
        if output.shapes.is_empty() {
            return Err(format!("{label} rendered no shapes"));
        }
    }
    Ok(PAGES.len())
}

fn production_preview(ui: &mut Ui, fig: &mut LizardApp, page: Page) {
    let t = Tokens::get(ui.ctx());
    if matches!(
        page,
        Page::MainMenu
            | Page::ActionsPalette
            | Page::ColorPicker
            | Page::GradientPicker
            | Page::ImagePicker
            | Page::PatternPicker
            | Page::ColorLibrary
            | Page::Variables
            | Page::Shortcuts
            | Page::Plugins
            | Page::Preferences
            | Page::About
            | Page::ExportDialog
            | Page::FindReplace
            | Page::BatchRename
            | Page::LinkDialog
            | Page::ImportReport
    ) {
        ui.label("Use the floating production window. Its controls are interactive.");
        return;
    }
    let wide = matches!(page, Page::Tabs | Page::Toolbar | Page::Home);
    let width = if wide {
        760.0
    } else if page == Page::Layers {
        328.0
    } else {
        417.0
    };
    let height = if page == Page::Layers { 836.0 } else { 620.0 };
    egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).inner_margin(egui::Margin::same(1)).show(ui, |ui| {
        ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(width);
            if let Some(section) = property_section(page) {
                lizard_ui_egui::right::catalog_section(fig, ui, section);
                return;
            }
            match page {
                Page::Tabs => lizard_ui_egui::tabs::tab_bar(fig, ui),
                Page::Toolbar => {
                    fig.canvas.rect = ui.max_rect();
                    lizard_ui_egui::toolbar::show(fig, ui);
                }
                Page::Layers | Page::Assets | Page::Agent | Page::AgentConversation | Page::AgentRunning | Page::AgentHistory | Page::Versions => {
                    lizard_ui_egui::layers::left_panel(fig, ui)
                }
                Page::Home => lizard_ui_egui::home::show(fig, ui),
                Page::FramePresets => lizard_ui_egui::frame_presets::panel(fig, ui),
                Page::Comments => lizard_ui_egui::comments::panel(fig, ui),
                _ => lizard_ui_egui::right::right_panel(fig, ui),
            }
        });
    });
}

fn property_section(page: Page) -> Option<lizard_ui_egui::right::CatalogSection> {
    use lizard_ui_egui::right::CatalogSection as S;
    Some(match page {
        Page::PropHeader => S::Header,
        Page::PropPosition => S::Position,
        Page::PropLayout => S::Layout,
        Page::PropAppearance => S::Appearance,
        Page::PropTypography => S::Typography,
        Page::PropFills => S::Fills,
        Page::PropStrokes => S::Strokes,
        Page::PropEffects => S::Effects,
        Page::PropSelectionColors => S::SelectionColors,
        Page::PropGrids => S::Grids,
        Page::PropExport => S::Export,
        Page::PropComponent => S::Component,
        Page::PropVariableModes => S::VariableModes,
        Page::PropScale => S::Scale,
        _ => return None,
    })
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
    use lizard_ui_egui::primitives::{Button, ButtonAlign, ButtonBehavior, ButtonSize, ButtonVariant, ControlWidth, Menu, MenuItem};

    example(ui, "Visual variants", "Hover, press, focus, and disabled states use Lizard's semantic color roles.", |ui| {
        for (variant, label) in [
            (ButtonVariant::Primary, "Primary"),
            (ButtonVariant::Secondary, "Secondary"),
            (ButtonVariant::Danger, "Danger"),
            (ButtonVariant::DangerSecondary, "Danger secondary"),
            (ButtonVariant::DangerGhost, "Danger ghost"),
            (ButtonVariant::DangerLink, "Danger link"),
            (ButtonVariant::Ghost, "Ghost"),
            (ButtonVariant::Link, "Link"),
            (ButtonVariant::Input, "Input"),
            (ButtonVariant::Property, "Property"),
            (ButtonVariant::Overlay, "Overlay"),
        ] {
            ui.horizontal(|ui| {
                ui.set_min_width(330.0);
                let response = Button::new(label).variant(variant).width(ControlWidth::Fixed(170.0)).show(ui);
                if response.clicked() {
                    state.clicks += 1;
                }
                Button::new("Disabled").variant(variant).enabled(false).width(ControlWidth::Fixed(110.0)).show(ui);
            });
        }
    });
    example(ui, "Behavior", "Toggle owns its state in the caller; select opens a real menu and updates its value.", |ui| {
        ui.horizontal(|ui| {
            if Button::new("Toggle")
                .variant(ButtonVariant::Ghost)
                .behavior(ButtonBehavior::Toggle { selected: state.button_toggle })
                .leading(Icon::Check)
                .show(ui)
                .clicked()
            {
                state.button_toggle = !state.button_toggle;
            }
            let options = ["Design", "Prototype", "Inspect"];
            let label = options.get(state.button_selection).copied().unwrap_or("Design");
            let response = Button::new(label)
                .variant(ButtonVariant::Input)
                .behavior(ButtonBehavior::Select { expanded: false })
                .width(ControlWidth::Fixed(130.0))
                .show(ui);
            Menu::popup(&response).width(response.rect.width()).show(|ui| {
                for (index, choice) in options.into_iter().enumerate() {
                    if MenuItem::new(choice).checked(state.button_selection == index).show(ui).clicked() {
                        state.button_selection = index;
                        ui.close();
                    }
                }
            });
            if Button::new("Input-like action").variant(ButtonVariant::Input).show(ui).clicked() {
                state.clicks += 1;
            }
        });
        ui.label(format!("Toggle: {} · Selected: {} · Actions: {}", state.button_toggle, state.button_selection, state.clicks));
    });
    example(ui, "Icons and content", "Icons come from the Lizard icon set and can lead, trail, or stand alone.", |ui| {
        ui.horizontal_wrapped(|ui| {
            for button in [
                Button::new("Text only").variant(ButtonVariant::Secondary),
                Button::new("Add layer").variant(ButtonVariant::Secondary).leading(Icon::Plus),
                Button::new("Continue").variant(ButtonVariant::Primary).trailing(Icon::Arrow),
                Button::new("Open menu").variant(ButtonVariant::Input).leading(Icon::Grid).trailing(Icon::Chevron),
                Button::icon(Icon::Plus, "Add").variant(ButtonVariant::Primary),
                Button::icon(Icon::Eye, "Show"),
                Button::icon(Icon::Lock, "Lock").variant(ButtonVariant::Secondary),
                Button::icon(Icon::Close, "Delete").variant(ButtonVariant::DangerGhost),
            ] {
                if button.show(ui).clicked() {
                    state.clicks += 1;
                }
            }
        });
    });
    example(ui, "Alignment and width", "Start, center, and end work with text and icons at fixed or fill widths.", |ui| {
        for (align, label) in [(ButtonAlign::Start, "Start"), (ButtonAlign::Center, "Center"), (ButtonAlign::End, "End")] {
            ui.horizontal(|ui| {
                Button::new(label).variant(ButtonVariant::Secondary).align(align).width(ControlWidth::Fixed(175.0)).show(ui);
                Button::new(label).variant(ButtonVariant::Input).align(align).leading(Icon::Grid).width(ControlWidth::Fixed(175.0)).show(ui);
            });
        }
        ui.horizontal(|ui| {
            Button::new("Hug").variant(ButtonVariant::Secondary).width(ControlWidth::Hug).show(ui);
            Button::new("Fixed 120").variant(ButtonVariant::Secondary).width(ControlWidth::Fixed(120.0)).show(ui);
        });
        ui.allocate_ui(vec2(360.0, 28.0), |ui| {
            Button::new("Fill · property trigger")
                .variant(ButtonVariant::Property)
                .leading(Icon::Grid)
                .align(ButtonAlign::Start)
                .width(ControlWidth::Fill)
                .show(ui);
        });
    });
    example(ui, "Sizes", "The regular button is 24 px high; large buttons are 32 px high.", |ui| {
        ui.horizontal(|ui| {
            Button::new("Regular").variant(ButtonVariant::Primary).show(ui);
            Button::new("Large").variant(ButtonVariant::Primary).size(ButtonSize::Large).show(ui);
            Button::icon(Icon::Plus, "Add large").variant(ButtonVariant::Secondary).size(ButtonSize::Large).show(ui);
        });
    });
}

fn apply_edit(value: &mut f64, edit: Option<FieldEdit>) {
    if let Some(FieldEdit::Set(next) | FieldEdit::Scrub(next, _)) = edit {
        *value = next;
    }
}

fn numeric(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{ControlWidth, MenuItem, NumberField, Select, SelectPosition};

    example(
        ui,
        "Numeric value",
        "The 88 × 24 opacity field uses the same primitive as the property panel. Edit the value or scrub its icon.",
        |ui| {
            ui.horizontal(|ui| {
                let current = state.number;
                let edit = widgets::number_field(ui, Id::new("library_number"), Lead::Text("W"), Some(current), "", 110.0, 1.0);
                apply_edit(&mut state.number, edit);
                let current = state.number;
                let edit = widgets::number_field(ui, Id::new("library_percent"), Lead::Icon(Icon::Opacity, "Opacity"), Some(current), "%", 88.0, 1.0);
                apply_edit(&mut state.number, edit);
            });
        },
    );
    example(ui, "Variable states", "Hover either field to reveal its outline and variable action. The picker is a separate component.", |ui| {
        ui.horizontal(|ui| {
            let (edit, event) = widgets::number_field_bindable(
                ui,
                Id::new("library_number_unbound"),
                Lead::Icon(Icon::Opacity, "Opacity"),
                Some(state.number),
                "%",
                88.0,
                1.0,
                Some(None),
                Empty::Text(""),
            );
            apply_edit(&mut state.number, edit.and_then(|edit| edit.settled()));
            if event.is_some() {
                state.bound = true;
            }
            let binding = if state.bound { Some(Some("Opacity variable")) } else { Some(None) };
            let (_, event) = widgets::number_field_bindable(
                ui,
                Id::new("library_number_bound"),
                Lead::Icon(Icon::Opacity, "Opacity"),
                Some(state.bound_number),
                "%",
                88.0,
                1.0,
                binding,
                Empty::Text(""),
            );
            if let Some(event) = event {
                state.bound = matches!(event, widgets::BindEvent::Open(_));
            }
        });
    });
    example(ui, "Empty and mixed", "The same field also shows Auto and Mixed values.", |ui| {
        ui.horizontal(|ui| {
            let _ = widgets::number_field_or(ui, Id::new("library_auto"), None, "Auto", 110.0, 1.0);
            let _ = widgets::number_field_mixed(ui, Id::new("library_mixed"), Lead::Text("X"), None, "", 110.0, 1.0);
        });
    });
    example(ui, "Sizing field", "The trailing sizing selector is part of the same control.", |ui| {
        let options = ["Fixed", "Hug contents", "Fill container"];
        let (edit, _, response) = widgets::number_field_with_sizing(
            ui,
            Id::new("library_sizing"),
            Lead::Text("W"),
            Some(state.number),
            170.0,
            None,
            options.get(state.sizing_choice).copied().unwrap_or(options[0]),
        );
        apply_edit(&mut state.number, edit.and_then(|edit| edit.settled()));
        let mut anchor = response.clone();
        anchor.rect = Rect::from_min_max(pos2(response.rect.right() - 170.0, response.rect.top()), response.rect.right_bottom());
        Select::popup_for(ui, &anchor, state.sizing_choice, options.len(), 170.0, SelectPosition::BottomLeft).show(|ui| {
            for (index, name) in options.iter().enumerate() {
                if MenuItem::new(name).checked(index == state.sizing_choice).show(ui).clicked() {
                    state.sizing_choice = index;
                    ui.close();
                }
            }
        });
    });
    example(ui, "Numeric input with presets", "Edit the number directly or open the joined chevron to pick a suggested font size.", |ui| {
        let sizes = [10.0, 11.0, 12.0, 14.0, 16.0, 20.0, 24.0, 32.0, 48.0];
        let response = NumberField::new(Id::new("catalog_number_presets"), Lead::None, Some(state.number), "")
            .options(&sizes)
            .show(ui, ControlWidth::Fixed(160.0));
        apply_edit(&mut state.number, response.edit.and_then(|edit| edit.settled()));
        if let Some(value) = response.selected_option {
            state.number = value;
        }
    });
    example(ui, "Width modes", "Hug follows the value; Fixed stays at 88 pt; Fill takes the remaining width of this 320 pt container.", |ui| {
        ui.vertical(|ui| {
            ui.set_max_width(320.0);
            for (label, width) in [("Hug", ControlWidth::Hug), ("Fixed", ControlWidth::Fixed(88.0)), ("Fill", ControlWidth::Fill)] {
                ui.horizontal(|ui| {
                    ui.add_sized([48.0, 24.0], egui::Label::new(label));
                    let response =
                        NumberField::new(Id::new(("catalog_number_width", label)), Lead::Icon(Icon::Opacity, "Opacity"), Some(state.number), "%")
                            .show(ui, width);
                    apply_edit(&mut state.number, response.edit.and_then(|edit| edit.settled()));
                });
            }
        });
    });
}

fn code_blocks(ui: &mut Ui, state: &mut State) {
    example(ui, "WGSL editor", "Editable code with syntax colors, line numbers, and scrolling inside the block.", |ui| {
        CodeBlock::new(Id::new("catalog_wgsl_code")).size(560.0, 300.0).language(CodeLanguage::Wgsl).show(ui, &mut state.code_sample);
    });
    example(ui, "Plain code", "The same primitive can display other source formats without WGSL highlighting.", |ui| {
        let mut sample = "const value = 42;\nconsole.log(value);".to_owned();
        CodeBlock::new(Id::new("catalog_plain_code")).size(420.0, 100.0).editable(false).show(ui, &mut sample);
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
    example(ui, "Text input with presets", "Edit the text directly or open the joined chevron to choose a variant value.", |ui| {
        let values = ["Default", "Hover", "Active"];
        if let Some(next) = TextFieldWithPresets::new(Id::new("library_text_presets"), &state.text_presets, &values)
            .rename_action()
            .show(ui, ControlWidth::Fixed(160.0))
        {
            state.text_presets = next;
        }
    });
    example(ui, "Multiline", "The draft commits when the editor loses focus.", |ui| {
        if let Some(next) = widgets::multiline_field(ui, Id::new("library_multiline"), &state.multiline, "Description", 3) {
            state.multiline = next;
        }
    });
}

fn selects(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{ControlWidth, Select, SelectLead, SelectPosition, SelectVariant};

    const OPTIONS: [&str; 3] = ["Normal", "Multiply", "Screen"];
    example(ui, "Overlay · default position", "The popup matches the trigger width and its selected row is centered over the trigger.", |ui| {
        if let Some(index) = Select::new(&OPTIONS, state.select_choice).label("Blend mode").width(ControlWidth::Fixed(156.0)).show(ui) {
            state.select_choice = index;
        }
    });
    example(ui, "Leading art", "The same Select supports a plain value, an icon, or a category color.", |ui| {
        ui.horizontal(|ui| {
            for lead in [None, Some(SelectLead::Icon(Icon::Eye)), Some(SelectLead::Dot(Tokens::get(ui.ctx()).accent))] {
                let mut select = Select::new(&OPTIONS, state.select_choice).width(ControlWidth::Fixed(118.0));
                if let Some(lead) = lead {
                    select = select.lead(lead);
                }
                if let Some(index) = select.show(ui) {
                    state.select_choice = index;
                }
            }
        });
    });
    example(ui, "Position", "Choose whether the list opens over, below, or above the trigger; side alignment is configurable.", |ui| {
        for row in [
            [
                ("Bottom left", SelectPosition::BottomLeft),
                ("Bottom center", SelectPosition::BottomCenter),
                ("Bottom right", SelectPosition::BottomRight),
            ],
            [("Top left", SelectPosition::TopLeft), ("Top center", SelectPosition::TopCenter), ("Top right", SelectPosition::TopRight)],
        ] {
            ui.horizontal(|ui| {
                for (label, position) in row {
                    ui.vertical(|ui| {
                        ui.label(label);
                        if let Some(index) =
                            Select::new(&OPTIONS, state.select_choice).label(label).position(position).width(ControlWidth::Fixed(140.0)).show(ui)
                        {
                            state.select_choice = index;
                        }
                    });
                }
            });
        }
    });
    example(ui, "Width modes", "Hug fits the widest option, Fixed uses 156 pt, and Fill occupies the remaining width.", |ui| {
        ui.vertical(|ui| {
            ui.set_max_width(360.0);
            for (label, width) in [("Hug", ControlWidth::Hug), ("Fixed", ControlWidth::Fixed(156.0)), ("Fill", ControlWidth::Fill)] {
                ui.horizontal(|ui| {
                    ui.add_sized([48.0, 24.0], egui::Label::new(label));
                    if let Some(index) = Select::new(&OPTIONS, state.select_choice).label(label).width(width).show(ui) {
                        state.select_choice = index;
                    }
                });
            }
        });
    });
    example(ui, "Ghost variant", "Transparent at rest; the secondary fill appears on hover or while open.", |ui| {
        ui.horizontal(|ui| {
            if let Some(index) = Select::new(&OPTIONS, state.select_choice)
                .label("Ghost blend mode")
                .variant(SelectVariant::Ghost)
                .width(ControlWidth::Fixed(156.0))
                .show(ui)
            {
                state.select_choice = index;
            }
            Select::new(&OPTIONS, state.select_choice).label("Disabled blend mode").enabled(false).width(ControlWidth::Fixed(156.0)).show(ui);
        });
    });
    example(ui, "Outlined variant", "Typography font selectors use the panel surface with a persistent field outline.", |ui| {
        if let Some(index) = Select::new(&OPTIONS, state.select_choice)
            .label("Outlined blend mode")
            .variant(SelectVariant::Outlined)
            .width(ControlWidth::Fixed(156.0))
            .show(ui)
        {
            state.select_choice = index;
        }
    });
}

fn floating_panels(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{Button, ButtonVariant, FloatingPanel, FloatingPanelHeight, FloatingPanelWidth};

    example(ui, "Panel sizes", "Open a panel, then drag its 40 pt header. Its position stays put while you edit the controls.", |ui| {
        ui.horizontal(|ui| {
            for (index, label) in [(0, "Compact · fixed"), (1, "Regular · hug"), (2, "Wide · fixed")] {
                if Button::new(label).variant(ButtonVariant::Secondary).show(ui).clicked() {
                    state.floating_open[index] = true;
                }
            }
        });
    });
    example(
        ui,
        "Production uses",
        "Effects, stroke settings, corner smoothing, export settings, typography, and prototype interactions share this movable panel surface.",
        |ui| {
            ui.label("Each panel composes its own header controls on the same draggable surface.");
        },
    );

    for (index, title, width_mode, height_mode) in [
        (0, "Drop shadow", FloatingPanelWidth::Compact, FloatingPanelHeight::Fixed(285.0)),
        (1, "Color picker", FloatingPanelWidth::Regular, FloatingPanelHeight::Hug),
        (2, "Extended properties", FloatingPanelWidth::Wide, FloatingPanelHeight::Fixed(240.0)),
    ] {
        if !state.floating_open[index] {
            continue;
        }
        let width = match width_mode {
            FloatingPanelWidth::Compact => 240.0,
            FloatingPanelWidth::Regular => 304.0,
            FloatingPanelWidth::Wide => 400.0,
            FloatingPanelWidth::Fixed(width) => width,
        };
        let origin =
            pos2(ui.ctx().content_rect().center().x - width / 2.0 + index as f32 * 30.0, ui.ctx().content_rect().top() + 140.0 + index as f32 * 40.0);
        let mut open = state.floating_open[index];
        let mut close = false;
        FloatingPanel::new(Id::new(("catalog_floating_panel", index)), origin)
            .width(width_mode)
            .height(height_mode)
            .open(&mut open)
            .show_with_header(ui.ctx(), |ui, header| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, Color32::from_rgb(68, 68, 68)));
                match index {
                    0 => {
                        icons::paint_panel_reference(
                            ui.painter(),
                            Rect::from_center_size(pos2(header.left() + 20.0, header.center().y), vec2(14.0, 14.0)),
                            32,
                            Color32::WHITE,
                        );
                        ui.painter().text(
                            pos2(header.left() + 36.0, header.center().y),
                            egui::Align2::LEFT_CENTER,
                            title,
                            theme::semibold(11.0),
                            Color32::WHITE,
                        );
                    }
                    1 => {
                        for (tab, label) in ["Custom", "Libraries"].iter().enumerate() {
                            let tab_rect = Rect::from_min_size(pos2(header.left() + 12.0 + tab as f32 * 84.0, header.top() + 8.0), vec2(80.0, 24.0));
                            let tab_id = Id::new(("catalog_panel_tab", index));
                            let active = ui.data(|d| d.get_temp::<usize>(tab_id)).unwrap_or(0);
                            let response = ui.interact(tab_rect, tab_id.with(tab), Sense::click());
                            if response.clicked() {
                                ui.data_mut(|d| d.insert_temp(tab_id, tab));
                            }
                            let ink = if active == tab { Color32::WHITE } else { Color32::from_white_alpha(160) };
                            ui.painter().text(tab_rect.center(), egui::Align2::CENTER_CENTER, label, theme::semibold(11.0), ink);
                        }
                    }
                    _ => {
                        ui.painter().text(
                            pos2(header.left() + 16.0, header.center().y),
                            egui::Align2::LEFT_CENTER,
                            title,
                            theme::semibold(11.0),
                            Color32::WHITE,
                        );
                        let action = Rect::from_center_size(pos2(header.right() - 48.0, header.center().y), vec2(24.0, 24.0));
                        let response = ui.interact(action, Id::new(("catalog_panel_action", index)), Sense::click());
                        if response.hovered() {
                            ui.painter().rect_filled(action, egui::CornerRadius::same(5), Color32::from_rgb(56, 56, 56));
                        }
                        icons::paint(ui.painter(), action.shrink(6.0), Icon::Dots, Color32::WHITE);
                    }
                }
                let close_rect = Rect::from_center_size(pos2(header.right() - 20.0, header.center().y), vec2(24.0, 24.0));
                if FloatingPanel::close_button(ui, Id::new(("catalog_floating_close", index)), close_rect, |painter, rect| {
                    icons::paint_panel_reference(painter, rect.shrink(7.0), 36, Color32::WHITE);
                }) {
                    close = true;
                }
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label("Alignment");
                    ui.add_space(16.0);
                    let _ = widgets::number_field(ui, Id::new(("catalog_floating_x", index)), Lead::Text("X"), Some(0.0), "", 88.0, 1.0);
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label("Blur");
                    ui.add_space(45.0);
                    let _ = widgets::number_field(ui, Id::new(("catalog_floating_blur", index)), Lead::None, Some(4.0), "", 88.0, 1.0);
                });
                ui.add_space(20.0);
            });
        state.floating_open[index] = open && !close;
    }
}

fn property_controllers(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{PropertyBinding, PropertyController, PropertyControllerKind as Kind};

    example(ui, "Component properties", "Hover for settings and delete. Click a row to rename it; Enter saves and Escape cancels.", |ui| {
        ui.allocate_ui_with_layout(vec2(320.0, 200.0), Layout::top_down(egui::Align::Min), |ui| {
            ui.label(egui::RichText::new("Properties").font(theme::property_section_title()).color(Tokens::get(ui.ctx()).text));
            let rows = [
                (Kind::Variant, Some("Default"), false),
                (Kind::Text, Some("value"), true),
                (Kind::Boolean, Some("True"), true),
                (Kind::InstanceSwap, Some("Button"), true),
                (Kind::Slot, None, true),
            ];
            for (index, (kind, value, unused)) in rows.into_iter().enumerate() {
                if !state.property_visible[index] {
                    continue;
                }
                let name = state.property_names[index].clone();
                let mut controller = PropertyController::new(kind, &name).unused(unused).selected(state.property_selection == Some(index));
                if let Some(value) = value {
                    controller = controller.value(value);
                }
                let response = controller.show(ui);
                if let Some(name) = response.renamed {
                    state.property_names[index] = name;
                }
                if response.settings_clicked {
                    state.property_selection = (state.property_selection != Some(index)).then_some(index);
                }
                if response.delete_clicked {
                    state.property_visible[index] = false;
                    state.property_selection = None;
                }
            }
            if let Some(index) = state.property_selection {
                ui.label(format!("Settings for {}", state.property_names[index]));
            }
        });
    });
    example(ui, "Bound properties", "A property linked to a component layer has no warning icon.", |ui| {
        ui.allocate_ui_with_layout(vec2(320.0, 72.0), Layout::top_down(egui::Align::Min), |ui| {
            for (index, kind, value) in [(5, Kind::Text, Some("Button")), (6, Kind::Slot, None)] {
                if !state.property_visible[index] {
                    continue;
                }
                let name = state.property_names[index].clone();
                let mut controller = PropertyController::new(kind, &name);
                if let Some(value) = value {
                    controller = controller.value(value);
                }
                let response = controller.show(ui);
                if let Some(name) = response.renamed {
                    state.property_names[index] = name;
                }
                if response.delete_clicked {
                    state.property_visible[index] = false;
                }
            }
        });
    });
    example(ui, "Applied property", "A layer value bound to a component property. Use the target to find the property or unlink it.", |ui| {
        ui.allocate_ui_with_layout(vec2(320.0, 88.0), Layout::top_down(egui::Align::Min), |ui| {
            if state.property_binding_attached {
                let response = PropertyBinding::new(Kind::Text, "Texto").show(ui);
                if response.locate_clicked {
                    state.property_binding_located = true;
                }
                if response.detach_clicked {
                    state.property_binding_attached = false;
                    state.property_binding_located = false;
                }
                if state.property_binding_located {
                    ui.label("Property selected");
                }
            } else if Button::new("Bind Texto").variant(ButtonVariant::Secondary).show(ui).clicked() {
                state.property_binding_attached = true;
            }
        });
    });
    if Button::new("Reset examples").variant(ButtonVariant::Secondary).show(ui).clicked() {
        state.property_names =
            ["Property 1".into(), "Text".into(), "Boolean".into(), "Instance".into(), "Slot".into(), "Label".into(), "Content".into()];
        state.property_visible = [true; 7];
        state.property_selection = None;
        state.property_binding_attached = true;
        state.property_binding_located = false;
    }
}

fn tooltips(ui: &mut Ui) {
    use lizard_ui_egui::primitives::Tooltip;

    example(ui, "Live tooltips", "Hover the controls to inspect timing, position, surface and text.", |ui| {
        ui.horizontal(|ui| {
            let simple = Button::new("Simple tooltip").variant(ButtonVariant::Secondary).show(ui);
            Tooltip::new("Create component").show_for(&simple);
            let shortcut = Button::new("With shortcut").variant(ButtonVariant::Secondary).show(ui);
            Tooltip::new("Duplicate selection").shortcut("⌘D").show_for(&shortcut);
            let detailed = Button::new("With description").variant(ButtonVariant::Secondary).show(ui);
            Tooltip::new("Apply variable").description("Bind this value to a variable or component property.").show_for(&detailed);
        });
    });
    example(ui, "Tooltip specimens", "Always visible previews use the same primitive as the hover tooltips.", |ui| {
        ui.horizontal(|ui| {
            Tooltip::new("Create component").preview(ui);
            ui.add_space(12.0);
            Tooltip::new("Duplicate selection").shortcut("⌘D").preview(ui);
        });
        ui.add_space(16.0);
        Tooltip::new("Apply variable").description("Bind this value to a variable or component property.").preview(ui);
    });
}

fn shortcut_primitive(ui: &mut Ui) {
    use lizard_ui_egui::primitives::Shortcut;

    example(ui, "Shortcut specimens", "The same compact badge is used in menus, tooltips and shortcut help.", |ui| {
        ui.horizontal(|ui| {
            for keys in ["⌘V", "⌘A", "⌘\\", "⇧1", "⇧R", "⌥⌘K", "Ctrl+Shift+K", "↵"] {
                Shortcut::new(keys).show(ui);
            }
        });
    });
}

fn segmented(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{ControlWidth, Segment, SegmentSize, SegmentSizing, SegmentedControl};

    example(ui, "Equal width · animated", "The selected surface slides between equally sized segments by default.", |ui| {
        let segments = [Segment::text("Design"), Segment::text("Prototype"), Segment::text("Inspect")];
        if let Some(index) = SegmentedControl::new(&segments, Some(state.segment)).show(ui) {
            state.segment = index;
        }
    });
    example(ui, "Fixed width", "The three equal segments can also occupy a specified 320 px width.", |ui| {
        let segments = [Segment::text("Design"), Segment::text("Prototype"), Segment::text("Inspect")];
        if let Some(index) = SegmentedControl::new(&segments, Some(state.segment)).width(ControlWidth::Fixed(320.0)).show(ui) {
            state.segment = index;
        }
    });
    example(ui, "Icon segments", "Icons share the same animated surface; hover shows each name.", |ui| {
        if let Some(index) = widgets::segmented(
            ui,
            &[(Icon::AlignLeft, "Left"), (Icon::AlignHCenter, "Center"), (Icon::AlignRight, "Right")],
            Some(state.icon_segment),
            38.0,
        ) {
            state.icon_segment = index;
        }
    });
    example(ui, "Auto width", "Each segment hugs its text; the indicator changes width as it moves.", |ui| {
        let segments = [Segment::text("Small"), Segment::text("Medium option"), Segment::text("Large")];
        if let Some(index) = SegmentedControl::new(&segments, Some(state.segment_auto)).sizing(SegmentSizing::Auto).show(ui) {
            state.segment_auto = index;
        }
    });
    example(ui, "Large", "The 32 px size keeps the same default animation at a fixed width.", |ui| {
        let segments = [Segment::text("Left"), Segment::text("Center"), Segment::text("Right")];
        if let Some(index) =
            SegmentedControl::new(&segments, Some(state.segment_auto)).size(SegmentSize::Large).width(ControlWidth::Fixed(320.0)).show(ui)
        {
            state.segment_auto = index;
        }
    });
}

fn switches(ui: &mut Ui, state: &mut State) {
    example(ui, "Standalone", "Click to toggle. The thumb and track use a short transition.", |ui| {
        if Switch::new(Id::new("catalog_switch_grid"), state.switch_primary, "Show grid").show(ui).clicked() {
            state.switch_primary = !state.switch_primary;
        }
        if Switch::new(Id::new("catalog_switch_snap"), state.switch_secondary, "Snap to pixels").show(ui).clicked() {
            state.switch_secondary = !state.switch_secondary;
        }
    });
    example(ui, "Embedded", "The same switch fits a row whose layout is owned by its parent.", |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(220.0, 24.0), Sense::hover());
        if Switch::new(Id::new("catalog_switch_embedded"), state.switch_embedded, "Show guides").show_at(ui, rect).clicked() {
            state.switch_embedded = !state.switch_embedded;
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
                ("Solid", lizard_color::Paint::solid(lizard_color::Color::new(0.08, 0.58, 0.96, 1.0))),
                ("Alpha", lizard_color::Paint::solid(lizard_color::Color::new(0.95, 0.35, 0.40, 0.45))),
                ("Pattern", lizard_color::Paint::pattern(1)),
                ("Image", lizard_color::Paint::image("sample", lizard_color::ImageScaleMode::Fill)),
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

fn apply_catalog_color_change(fig: &mut LizardApp, id: u64, change: lizard_ui_egui::primitives::ColorControlResponse) {
    use lizard_ui_egui::widgets::FieldEdit;

    if change.open_picker {
        fig.ui.picker = Some(lizard_ui_egui::colorpicker::PickerTarget::Paint { fills: true, index: 0, ids: vec![NodeId(id)] });
    }
    if let Some(text) = change.edited_hex
        && let Some(color) = lizard_color::Color::from_hex_or_name(&text)
    {
        fig.run("fill.set", json!({"ids": [id], "index": 0, "color": color.to_css_hex()}));
    }
    if let Some((color, done)) = change.edited_color {
        let payload = json!({"ids": [id], "index": 0, "color": color.with_alpha(1.0).to_css_hex(), "opacity": color.a});
        let _ = fig.session.begin_interaction("Color");
        let _ = fig.session.execute("fill.set", &payload);
        if done {
            let _ = fig.session.commit_interaction();
        }
    }
    match change.opacity_edit {
        Some(FieldEdit::Set(value)) => {
            fig.run("fill.set", json!({"ids": [id], "index": 0, "opacity": (value / 100.0).clamp(0.0, 1.0)}));
        }
        Some(FieldEdit::Scrub(value, done)) => {
            let _ = fig.session.begin_interaction("Opacity");
            let _ = fig.session.execute("fill.set", &json!({"ids": [id], "index": 0, "opacity": (value / 100.0).clamp(0.0, 1.0)}));
            if done {
                let _ = fig.session.commit_interaction();
            }
        }
        None => {}
    }
}

fn color_control(ui: &mut Ui, _state: &mut State, fig: &mut LizardApp, fixture: &Fixture) {
    use lizard_ui_egui::primitives::{ColorControl, ColorModel, ColorValue, ControlWidth};
    use lizard_ui_egui::widgets::FieldEdit;

    let Some(id) = fixture.color else { return };
    let paint = fig
        .session
        .doc()
        .ok()
        .and_then(|doc| doc.node(NodeId(id)).and_then(|node| node.fills.first().cloned()))
        .unwrap_or_else(|| lizard_color::Paint::solid(lizard_color::Color::BLACK));
    let hex = paint.solid_color().map(|color| color.to_hex()).unwrap_or_default();
    example(
        ui,
        "Solid color",
        "The label composes with the primitive. The swatch opens the production picker; hex and opacity edit the sample node.",
        |ui| {
            let change = ui
                .horizontal(|ui| {
                    ui.label("Color");
                    ui.add_space(42.0);
                    ColorControl::new(Id::new("catalog_color_control"), &paint, ColorValue::EditableHex(&hex), f64::from(paint.opacity) * 100.0)
                        .show(ui, 156.0)
                })
                .inner;
            apply_catalog_color_change(fig, id, change);
        },
    );

    example(ui, "Color models", "The 144 × 24 fields from Pencil node Y0mRc. Each edit updates the same sample color.", |ui| {
        for (label, model) in
            [("Hex", ColorModel::Hex), ("RGB", ColorModel::Rgb), ("HSL", ColorModel::Hsl), ("HSB", ColorModel::Hsb), ("CSS", ColorModel::Css)]
        {
            ui.horizontal(|ui| {
                ui.add_sized([48.0, 24.0], egui::Label::new(label));
                let change =
                    ColorControl::new(Id::new(("catalog_color_model", label)), &paint, ColorValue::Model(model), f64::from(paint.opacity) * 100.0)
                        .show(ui, 144.0);
                apply_catalog_color_change(fig, id, change);
            });
        }
    });

    let gradient = fixture.gradient.and_then(|id| {
        fig.session.doc().ok().and_then(|doc| doc.node(NodeId(id)).and_then(|node| node.fills.first().cloned()).map(|paint| (id, paint)))
    });
    if let Some((id, paint)) = gradient {
        example(ui, "Named value", "The same primitive can show a gradient, image, or pattern name in the value slot.", |ui| {
            let label = widgets::paint_name(&paint);
            let change = ColorControl::new(Id::new("catalog_gradient_control"), &paint, ColorValue::Label(&label), f64::from(paint.opacity) * 100.0)
                .show(ui, 156.0);
            if change.open_picker {
                fig.ui.picker = Some(lizard_ui_egui::colorpicker::PickerTarget::Paint { fills: true, index: 0, ids: vec![NodeId(id)] });
            }
            if let Some(FieldEdit::Set(value) | FieldEdit::Scrub(value, _)) = change.opacity_edit {
                fig.run("fill.set", json!({"ids": [id], "index": 0, "opacity": (value / 100.0).clamp(0.0, 1.0)}));
            }
        });
    }

    if let Some(bound_id) = fixture.bound_color {
        example(ui, "Bound variable", "The sample is bound to a real document variable; hover to detach it.", |ui| {
            let Some((bound_paint, binding)) = fig.session.doc().ok().and_then(|doc| {
                let node = doc.node(NodeId(bound_id))?;
                let paint = node.fills.first()?.clone();
                let binding = lizard_engine::cmd::lists::paint_binding_key(node, true, 0)
                    .and_then(|key| lizard_ui_egui::variable_ui::bound_name(doc, &[NodeId(bound_id)], key).map(|name| (key.to_owned(), name)));
                Some((paint, binding))
            }) else {
                return;
            };
            let bound_hex = bound_paint.solid_color().map(|color| color.to_hex()).unwrap_or_default();
            let value = binding.as_ref().map_or(ColorValue::EditableHex(&bound_hex), |(_, name)| ColorValue::BoundVariable(name));
            let change =
                ColorControl::new(Id::new("catalog_bound_color"), &bound_paint, value, f64::from(bound_paint.opacity) * 100.0).show(ui, 156.0);
            if change.open_picker {
                fig.ui.picker = Some(lizard_ui_egui::colorpicker::PickerTarget::Paint { fills: true, index: 0, ids: vec![NodeId(bound_id)] });
            }
            if change.detach_variable
                && let Some((key, _)) = &binding
            {
                fig.run("variable.bind", json!({"ids": [bound_id], "field": key, "variable": null}));
            }
            if let Some(text) = change.edited_hex
                && let Some(color) = lizard_color::Color::from_hex_or_name(&text)
            {
                fig.run("fill.set", json!({"ids": [bound_id], "index": 0, "color": color.to_css_hex()}));
            }
            if let Some(FieldEdit::Set(value) | FieldEdit::Scrub(value, _)) = change.opacity_edit {
                fig.run("fill.set", json!({"ids": [bound_id], "index": 0, "opacity": (value / 100.0).clamp(0.0, 1.0)}));
            }
            if binding.is_none()
                && let Some(variable) = &fixture.bound_color_variable
                && ui.button("Bind again").clicked()
            {
                fig.run("variable.bind", json!({"ids": [bound_id], "field": "fill", "variable": variable}));
            }
        });
    }
    example(ui, "Disabled", "The surrounding composition controls availability; the primitive keeps its layout.", |ui| {
        ui.add_enabled_ui(false, |ui| {
            ColorControl::new(Id::new("catalog_disabled_color"), &paint, ColorValue::EditableHex(&hex), f64::from(paint.opacity) * 100.0)
                .show(ui, 156.0);
        });
    });
    example(ui, "Width modes", "Hug fits the value, Fixed uses 156 pt, and Fill takes the remaining space in this 360 pt container.", |ui| {
        ui.vertical(|ui| {
            ui.set_max_width(360.0);
            for (label, width) in [("Hug", ControlWidth::Hug), ("Fixed", ControlWidth::Fixed(156.0)), ("Fill", ControlWidth::Fill)] {
                ui.horizontal(|ui| {
                    ui.add_sized([48.0, 24.0], egui::Label::new(label));
                    let change = ColorControl::new(
                        Id::new(("catalog_color_width", label)),
                        &paint,
                        ColorValue::EditableHex(&hex),
                        f64::from(paint.opacity) * 100.0,
                    )
                    .show_sized(ui, width);
                    apply_catalog_color_change(fig, id, change);
                });
            }
        });
    });
}

fn primitive_tabs(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{ControlWidth, Tab, TabOverflow, Tabs};

    let basic = [Tab::new("General").value("general"), Tab::new("Advanced").value("advanced"), Tab::new("Export").value("export")];
    example(ui, "Default", "Click a tab or use the arrow, Home and End keys. Selection belongs to the caller.", |ui| {
        if let Some(index) = Tabs::new(Id::new("catalog_tabs_default"), &basic, Some(state.tab_choice)).width(ControlWidth::Fixed(300.0)).show(ui) {
            state.tab_choice = index;
        }
    });
    example(ui, "Single tab", "A single tab remains transparent, as in the reference.", |ui| {
        Tabs::new(Id::new("catalog_tabs_single"), &[Tab::new("General")], Some(0)).width(ControlWidth::Hug).show(ui);
    });
    example(ui, "Content", "The selected index controls which panel the caller composes below the tab row.", |ui| {
        if let Some(index) =
            Tabs::new(Id::new("catalog_tabs_content"), &basic, Some(state.tab_content_choice)).width(ControlWidth::Fixed(300.0)).show(ui)
        {
            state.tab_content_choice = index;
        }
        let title = ["General settings", "Advanced settings", "Export settings"][state.tab_content_choice.min(2)];
        egui::Frame::NONE.fill(Tokens::get(ui.ctx()).field).corner_radius(5).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_min_width(276.0);
            ui.label(title);
        });
    });
    example(ui, "Disabled", "An individual tab and an entire tab group can be disabled.", |ui| {
        let items = [Tab::new("General"), Tab::new("Advanced").disabled(true), Tab::new("Export")];
        Tabs::new(Id::new("catalog_tabs_disabled_item"), &items, Some(0)).width(ControlWidth::Fixed(300.0)).show(ui);
        Tabs::new(Id::new("catalog_tabs_disabled_group"), &basic, Some(0)).disabled(true).width(ControlWidth::Fixed(300.0)).show(ui);
    });
    example(ui, "Width", "Hug, fixed and fill follow the same sizing contract as other primitives.", |ui| {
        ui.set_max_width(400.0);
        for (label, width) in [("Hug", ControlWidth::Hug), ("Fixed", ControlWidth::Fixed(220.0)), ("Fill", ControlWidth::Fill)] {
            ui.label(label);
            Tabs::new(Id::new(("catalog_tabs_width", label)), &basic, Some(1)).width(width).show(ui);
        }
    });
    let overflow = ["All", "ASCII", "Latin", "Punctuation", "Math", "Arrows", "Currency", "Symbols", "Emoji"];
    let overflow_tabs: Vec<_> = overflow.iter().map(|label| Tab::new(label)).collect();
    example(ui, "Overflow · buttons", "The chevrons page through tabs; selecting a hidden tab brings it into view.", |ui| {
        if let Some(index) = Tabs::new(Id::new("catalog_tabs_overflow_buttons"), &overflow_tabs, Some(state.tab_overflow_buttons))
            .overflow(TabOverflow::Buttons)
            .width(ControlWidth::Fixed(280.0))
            .show(ui)
        {
            state.tab_overflow_buttons = index;
        }
    });
    example(ui, "Overflow · fade", "Scroll horizontally or use the keyboard to reach tabs beyond the edges.", |ui| {
        if let Some(index) = Tabs::new(Id::new("catalog_tabs_overflow_fade"), &overflow_tabs, Some(state.tab_overflow_fade))
            .overflow(TabOverflow::Fade)
            .width(ControlWidth::Fixed(280.0))
            .show(ui)
        {
            state.tab_overflow_fade = index;
        }
    });
}

fn sliders(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{ControlWidth, Slider, SliderKind, SliderVariant};

    example(ui, "Types · with text", "Five slider types with editable values. Stepper shows its selected range and step marks.", |ui| {
        let labels = ["Range", "Hue", "Opacity", "Delta", "Stepper"];
        let kinds = [SliderKind::Range, SliderKind::Hue, SliderKind::Opacity, SliderKind::Delta, SliderKind::Stepper];
        for (index, (label, kind)) in labels.into_iter().zip(kinds).enumerate() {
            ui.horizontal(|ui| {
                ui.add_sized([72.0, 24.0], egui::Label::new(label));
                let mut slider = Slider::new(Id::new(("catalog_slider_kind", index)), label)
                    .kind(kind)
                    .value(Some(state.slider_variants[index]))
                    .width(ControlWidth::Fixed(248.0));
                if kind == SliderKind::Opacity {
                    slider = slider.color(Color32::from_rgb(0xff, 0x42, 0x3c)).units("%");
                }
                if kind == SliderKind::Delta {
                    slider = slider.range(-5.0, 5.0).step(0.25).reference(0.0).precision(2);
                }
                if let Some((next, _)) = slider.show(ui).change {
                    state.slider_variants[index] = next;
                }
            });
        }
    });
    example(ui, "Delta · with text", "The reference tick marks zero. The blue range extends in either direction; values snap to 0.25.", |ui| {
        if let Some((next, _)) = Slider::new(Id::new("catalog_slider_delta_text"), "Delta")
            .kind(SliderKind::Delta)
            .range(-5.0, 5.0)
            .step(0.25)
            .reference(0.0)
            .value(Some(state.slider_variants[3]))
            .precision(2)
            .width(ControlWidth::Fixed(320.0))
            .show(ui)
            .change
        {
            state.slider_variants[3] = next;
        }
    });
    example(
        ui,
        "Classic · delta with text",
        "The thumb keeps one size. It is solid white at zero, whether zero is centered or at the left edge.",
        |ui| {
            ui.horizontal(|ui| {
                ui.add_sized([80.0, 24.0], egui::Label::new("Moved"));
                if let Some((next, _)) = Slider::new(Id::new("catalog_slider_classic_delta"), "Classic delta")
                    .kind(SliderKind::Delta)
                    .variant(SliderVariant::Classic)
                    .range(-5.0, 5.0)
                    .step(0.25)
                    .reference(0.0)
                    .value(Some(state.slider_classic_delta))
                    .width(ControlWidth::Fixed(240.0))
                    .show(ui)
                    .change
                {
                    state.slider_classic_delta = next;
                }
            });
            ui.horizontal(|ui| {
                ui.add_sized([80.0, 24.0], egui::Label::new("Centered"));
                if let Some((next, _)) = Slider::new(Id::new("catalog_slider_classic_default"), "Classic at center")
                    .kind(SliderKind::Delta)
                    .variant(SliderVariant::Classic)
                    .range(-5.0, 5.0)
                    .step(0.25)
                    .reference(0.0)
                    .value(Some(state.slider_classic_default))
                    .width(ControlWidth::Fixed(240.0))
                    .show(ui)
                    .change
                {
                    state.slider_classic_default = next;
                }
            });
            ui.horizontal(|ui| {
                ui.add_sized([80.0, 24.0], egui::Label::new("Zero at left"));
                if let Some((next, _)) = Slider::new(Id::new("catalog_slider_classic_left"), "Classic with zero at left")
                    .kind(SliderKind::Delta)
                    .variant(SliderVariant::Classic)
                    .range(0.0, 10.0)
                    .step(0.25)
                    .reference(0.0)
                    .value(Some(state.slider_classic_left))
                    .width(ControlWidth::Fixed(240.0))
                    .show(ui)
                    .change
                {
                    state.slider_classic_left = next;
                }
            });
        },
    );
    example(ui, "Props and states", "Fixed, fill, mixed, reference, custom ticks, transform, precision, units and disabled.", |ui| {
        ui.set_max_width(360.0);
        for (label, sizing) in [("Hug", ControlWidth::Hug), ("Fixed", ControlWidth::Fixed(200.0)), ("Fill", ControlWidth::Fill)] {
            ui.horizontal(|ui| {
                ui.add_sized([48.0, 24.0], egui::Label::new(label));
                if let Some((next, _)) =
                    Slider::new(Id::new(("catalog_slider_width", label)), label).value(Some(state.slider)).width(sizing).show(ui).change
                {
                    state.slider = next;
                }
            });
        }
        ui.label("Mixed value");
        Slider::new(Id::new("catalog_slider_mixed"), "Mixed").value(None).placeholder("##").width(ControlWidth::Fixed(240.0)).show(ui);
        ui.label("Reference + custom ticks");
        Slider::new(Id::new("catalog_slider_ticks"), "Ticks")
            .value(Some(state.slider))
            .reference(50.0)
            .ticks(&[0.0, 25.0, 50.0, 75.0, 100.0])
            .width(ControlWidth::Fixed(240.0))
            .show(ui);
        ui.label("Transform ×100 · precision 1 · units %");
        Slider::new(Id::new("catalog_slider_transform"), "Transformed")
            .range(0.0, 1.0)
            .value(Some(state.slider / 100.0))
            .step(0.001)
            .transform(100.0)
            .precision(1)
            .units("%")
            .width(ControlWidth::Fixed(240.0))
            .show(ui);
        ui.label("Disabled");
        Slider::new(Id::new("catalog_slider_disabled"), "Disabled").value(Some(40.0)).disabled(true).width(ControlWidth::Fixed(240.0)).show(ui);
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

fn shimmer(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{Button, ButtonVariant, ControlWidth, ShimmerDirection, ShimmerText};

    example(ui, "Default", "1.5 s horizontal sweep through text, as in FigUI3.", |ui| {
        ShimmerText::new("Testing...").show(ui);
    });
    example(ui, "Diagonal · 0.6 s", "The settings from the linked reference. Toggle playback to compare the static state.", |ui| {
        ui.horizontal(|ui| {
            ShimmerText::new("Generating your design...")
                .direction(ShimmerDirection::Diagonal)
                .duration(0.6)
                .playing(state.shimmer_playing)
                .size(14.0)
                .show(ui);
            ui.add_space(14.0);
            if Button::new(if state.shimmer_playing { "Pause" } else { "Play" }).variant(ButtonVariant::Secondary).show(ui).clicked() {
                state.shimmer_playing = !state.shimmer_playing;
            }
        });
    });
    example(ui, "Directions", "Horizontal, vertical, and diagonal sweeps share the same text treatment.", |ui| {
        for (label, direction) in
            [("Horizontal", ShimmerDirection::Horizontal), ("Vertical", ShimmerDirection::Vertical), ("Diagonal", ShimmerDirection::Diagonal)]
        {
            ui.horizontal(|ui| {
                ui.add_sized([90.0, 22.0], egui::Label::new(label));
                ShimmerText::new("Loading content").direction(direction).duration(1.5).show(ui);
            });
        }
    });
    example(ui, "Paused and wrapped", "A fixed width wraps the text; paused shimmer renders as ordinary secondary text.", |ui| {
        ShimmerText::new("Preparing a preview of the selected components").width(ControlWidth::Fixed(220.0)).playing(false).show(ui);
    });
}

fn surfaces(ui: &mut Ui) {
    use lizard_ui_egui::primitives::MenuItem;
    use workbench_ui::{Surface, SurfaceTheme};

    let tokens = Tokens::get(ui.ctx());
    let theme = SurfaceTheme { dark: tokens.dark, panel: tokens.panel };

    example(ui, "Menu surface", "Shared dark popup chrome and top inset highlights.", |ui| {
        Surface::menu().show(ui, theme, |ui| {
            ui.set_width(220.0);
            MenuItem::new("Copy").shortcut("⌘C").show(ui);
            MenuItem::new("Paste").shortcut("⌘V").show(ui);
        });
    });
    example(ui, "Palette surface", "The Actions palette's 552 px shell with two outer and two inner shadows.", |ui| {
        Surface::palette().show(ui, theme, |ui| {
            ui.set_width(536.0);
            ui.set_min_height(90.0);
            ui.label("Search, tabs and action rows compose inside this surface.");
        });
    });
}

fn menus(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{Menu, MenuItem};

    example(ui, "Default menu", "The shared menu surface and item treatment used by property popovers.", |ui| {
        let response = lizard_ui_egui::primitives::Select::trigger(ui, "Open menu", 170.0, None);
        Menu::popup(&response).show(|ui| {
            if MenuItem::new("Add item").shortcut("⌘N").show(ui).clicked() {
                ui.close();
            }
            if MenuItem::new("Duplicate").shortcut("⌘D").show(ui).clicked() {
                ui.close();
            }
            Menu::separator(ui, None);
            for (index, label) in ["First choice", "Second choice"].iter().enumerate() {
                if MenuItem::new(label).checked(state.menu_choice == index).show(ui).clicked() {
                    state.menu_choice = index;
                    ui.close();
                }
            }
            Menu::separator(ui, Some("More"));
            MenuItem::new("Unavailable action").enabled(false).show(ui);
            if MenuItem::new("Manage choices").show(ui).clicked() {
                ui.close();
            }
        });
    });
    example(ui, "Icons and subtle hover", "Leading and trailing icons, keyboard hints, and a secondary hover variant.", |ui| {
        let response = lizard_ui_egui::primitives::Select::trigger(ui, "More actions", 170.0, None);
        Menu::popup(&response).show(|ui| {
            if MenuItem::new("Open").leading(Icon::Frame).shortcut("⌘O").show(ui).clicked() {
                ui.close();
            }
            if MenuItem::new("Export").leading(Icon::Export).trailing(Icon::ChevronRight).show(ui).clicked() {
                ui.close();
            }
            if MenuItem::new("Preferences").subtle(true).show(ui).clicked() {
                ui.close();
            }
        });
    });
    example(ui, "Context menu", "Right-click the area to open the same menu at the pointer.", |ui| {
        let (rect, response) = ui.allocate_exact_size(vec2(170.0, 48.0), Sense::click());
        let t = Tokens::get(ui.ctx());
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Right-click here", theme::medium(11.0), t.text_secondary);
        Menu::context(&response).show(|ui| {
            if MenuItem::new("Copy").shortcut("⌘C").show(ui).clicked() {
                ui.close();
            }
            if MenuItem::new("Paste").shortcut("⌘V").show(ui).clicked() {
                ui.close();
            }
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
    example(ui, "General icons", "One glyph per drawing. Hover to see its name and any legacy aliases.", |ui| {
        ui.horizontal_wrapped(|ui| {
            for icon in Icon::ALL.iter().filter(|icon| icon.canonical() == **icon) {
                let (rect, response) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                if response.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(5), t.hover);
                }
                icons::paint(ui.painter(), rect.shrink(7.0), *icon, t.icon);
                let aliases: Vec<_> =
                    Icon::ALL.iter().filter(|other| **other != *icon && other.canonical() == *icon).map(|other| format!("{other:?}")).collect();
                let caption = if aliases.is_empty() { format!("{icon:?}") } else { format!("{icon:?} · aliases: {}", aliases.join(", ")) };
                response.lizard_tooltip(caption);
            }
        });
    });
    example(ui, "Toolbar", "All 12 tool and mode icons shown at their normal toolbar scale.", |ui| {
        let names =
            ["Move", "Frame", "Rectangle", "Pen", "Text", "Comment", "Actions", "Chevron", "Draw mode", "Design mode", "Prototype mode", "Dev mode"];
        ui.horizontal_wrapped(|ui| {
            for (index, name) in names.into_iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                icons::paint_toolbar(ui.painter(), rect.shrink(7.0), index, t.icon);
                response.lizard_tooltip(name);
            }
        });
    });
    specialized_icons(ui);
    example(ui, "Alignment and document tab", "Production alignment glyphs and the document tab icon.", |ui| {
        ui.horizontal_wrapped(|ui| {
            for horizontal in [true, false] {
                for position in 0..3 {
                    let (rect, response) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                    icons::paint_alignment(ui.painter(), rect.shrink(7.0), horizontal, position);
                    response.lizard_tooltip(format!("{} alignment {position}", if horizontal { "Horizontal" } else { "Vertical" }));
                }
            }
            let (rect, response) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            icons::paint_tab_document(ui.painter(), rect.shrink(7.0), t.icon);
            response.lizard_tooltip("Document tab");
        });
    });
}

fn specialized_icons(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    example(ui, "Panel and navigation icons", "Dedicated SVG families used in the header, properties, layers, navigation, and Agent UI.", |ui| {
        let mut row = |name: &str, icons: usize, paint: &dyn Fn(&egui::Painter, Rect, usize), label: &dyn Fn(usize) -> String| {
            ui.label(egui::RichText::new(name).size(11.0).color(t.text_secondary));
            ui.horizontal_wrapped(|ui| {
                for index in 0..icons {
                    let (rect, response) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                    if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(5), t.hover);
                    }
                    paint(ui.painter(), rect.shrink(7.0), index);
                    response.lizard_tooltip(format!("{name} · {}", label(index)));
                }
            });
        };
        let header = [
            HeaderIcon::CollapsePanel,
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
        row("Header", header.len(), &|p, r, i| icons::paint_header(p, r, header[i], t.icon), &|i| format!("{:?}", header[i]));
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
        row("Position", position.len(), &|p, r, i| icons::paint_position(p, r, position[i], t.icon, t.icon_secondary), &|i| {
            format!("{:?}", position[i])
        });
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
        row("Layout", layout.len(), &|p, r, i| icons::paint_layout(p, r, layout[i], t.icon), &|i| format!("{:?}", layout[i]));
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
        row("Appearance", appearance.len(), &|p, r, i| icons::paint_appearance(p, r, appearance[i], t.icon), &|i| format!("{:?}", appearance[i]));
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
        row("Sizing", sizing.len(), &|p, r, i| icons::paint_sizing(p, r, sizing[i], t.icon), &|i| format!("{:?}", sizing[i]));
        let layer = [
            LayerIcon::Frame,
            LayerIcon::LayoutH,
            LayerIcon::LayoutV,
            LayerIcon::LayoutGrid,
            LayerIcon::Text,
            LayerIcon::Absolute,
            LayerIcon::ChevronDown,
            LayerIcon::ChevronRight,
            LayerIcon::Mask,
            LayerIcon::Ellipse,
            LayerIcon::EllipseMask,
            LayerIcon::Component,
            LayerIcon::Instance,
            LayerIcon::Vector,
            LayerIcon::Image,
            LayerIcon::Star,
            LayerIcon::Polygon,
            LayerIcon::Arrow,
            LayerIcon::Line,
            LayerIcon::Rectangle,
            LayerIcon::Group,
            LayerIcon::CollapseAll,
        ];
        row("Layers", layer.len(), &|p, r, i| icons::paint_layer(p, r, layer[i], t.icon), &|i| format!("{:?}", layer[i]));
        let nav = [NavIcon::Logo, NavIcon::File, NavIcon::Agent, NavIcon::Assets, NavIcon::Variables];
        row("Navigation", nav.len(), &|p, r, i| icons::paint_nav(p, r, nav[i], t.icon), &|i| format!("{:?}", nav[i]));
        let agent = [
            AgentIcon::Back,
            AgentIcon::More,
            AgentIcon::Emblem,
            AgentIcon::Polish,
            AgentIcon::Plugins,
            AgentIcon::Chevron,
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
        row("Agent", agent.len(), &|p, r, i| icons::paint_agent(p, r, agent[i], t.icon), &|i| format!("{:?}", agent[i]));
        let color_picker_names = [
            "Solid",
            "Gradient",
            "Pattern",
            "Image",
            "Video",
            "Shader",
            "Contrast",
            "Chevron",
            "Add style",
            "Close",
            "Eyedropper",
            "Flip gradient",
            "Gradient orientation",
            "Remove gradient stop",
            "Pattern source",
            "Pattern scale",
            "Create image",
        ];
        row("Color picker", color_picker_names.len(), &|p, r, i| icons::paint_colorpicker(p, r, i, t.icon), &|i| color_picker_names[i].into());
        let property_names = ["Styles", "Stroke weight", "Stroke settings", "Stroke sides"];
        row("Properties", property_names.len(), &|p, r, i| icons::paint_property(p, r, i, t.icon), &|i| property_names[i].into());
        let stroke_names = ["All sides", "Top", "Bottom", "Left", "Right"];
        row("Stroke sides", stroke_names.len(), &|p, r, i| icons::paint_stroke_side(p, r, i, t.icon), &|i| stroke_names[i].into());
        let panel_icons = icons::panel_reference_catalog();
        row("Panel reference", panel_icons.len(), &|p, r, i| icons::paint_panel_reference(p, r, panel_icons[i].0, t.icon), &|i| {
            panel_icons[i].1.clone()
        });
    });
}

fn theme_preview(ui: &mut Ui, state: &mut State) {
    use lizard_ui_egui::primitives::{Button, ButtonVariant, ControlWidth};

    ui.label("Semantic colors drive controls in both modes. The editor keeps its own panel and canvas surfaces.");
    ui.add_space(12.0);
    ui.columns(2, |columns| {
        for (column, dark) in columns.iter_mut().zip([false, true]) {
            let palette = if dark { Tokens::dark() } else { Tokens::light() };
            theme::with_theme(column, palette, |ui| {
                egui::Frame::NONE
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_min_width(300.0);
                        ui.label(egui::RichText::new(if dark { "Dark mode" } else { "Light mode" }).size(16.0).strong().color(palette.text));
                        ui.label(
                            egui::RichText::new(if dark { "Panel  #2B2B2B" } else { "Panel  #FFFFFF" }).size(11.0).color(palette.text_secondary),
                        );
                        ui.add_space(15.0);
                        ui.label(egui::RichText::new("Primary text").color(palette.text));
                        ui.label(egui::RichText::new("Supporting text and descriptions").color(palette.text_secondary));
                        ui.label(egui::RichText::new("Less prominent details").color(palette.text_tertiary));
                        ui.add_space(12.0);
                        ui.add(egui::TextEdit::singleline(&mut state.theme_preview_text).desired_width(230.0));
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            Button::new("Primary").variant(ButtonVariant::Primary).width(ControlWidth::Fixed(90.0)).show(ui);
                            Button::new("Secondary").variant(ButtonVariant::Secondary).width(ControlWidth::Fixed(100.0)).show(ui);
                        });
                        ui.add_space(5.0);
                        Button::new("Danger").variant(ButtonVariant::Danger).width(ControlWidth::Fixed(90.0)).show(ui);
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            for (label, color) in [("Selected", palette.selected_row), ("Component", palette.component_row)] {
                                egui::Frame::NONE.fill(color).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::symmetric(8, 5)).show(
                                    ui,
                                    |ui| {
                                        ui.label(egui::RichText::new(label).size(11.0).color(palette.text));
                                    },
                                );
                            }
                        });
                    });
            });
        }
    });
    ui.add_space(12.0);
    example(ui, "Lizard surface decisions", "These product colors intentionally differ from the generic palette.", |ui| {
        for (name, light, dark) in [
            ("Editor panel", "#FFFFFF", "#2B2B2B"),
            ("Canvas", "#F5F5F5", "#1E1E1E"),
            ("Text selection", "#255681 + white", "#255681 + white"),
            ("Menu shell", "#1E1E1E", "#1E1E1E"),
        ] {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(name).strong());
                ui.label(format!("Light {light}  ·  Dark {dark}"));
            });
        }
    });
}

fn preview_role_color(role: ColorRole, dark: bool, recipe: ThemeRecipe) -> Color32 {
    if recipe == ThemeRecipe::default() {
        return role.value(dark);
    }
    let t = Tokens::from_recipe(dark, recipe);
    if recipe.primary == ThemeRecipe::default().primary {
        return match role {
            ColorRole::Bg => t.bg,
            ColorRole::BgSecondary => t.field,
            ColorRole::BgTertiary | ColorRole::BgSwitch => t.surface_tertiary,
            ColorRole::BgHover => t.hover,
            ColorRole::BgPressed | ColorRole::BgActive => t.pressed,
            ColorRole::BgMenu => t.menu,
            ColorRole::Border => t.border,
            _ => role.value(dark),
        };
    }
    let shades = recipe.primary_scale(dark, t.panel);
    match role {
        ColorRole::Bg => t.bg,
        ColorRole::BgSecondary => t.field,
        ColorRole::BgTertiary | ColorRole::BgSwitch => t.surface_tertiary,
        ColorRole::BgHover => t.hover,
        ColorRole::BgPressed | ColorRole::BgActive => t.pressed,
        ColorRole::BgMenu => t.menu,
        ColorRole::BgMenuHover => t.menu_hover,
        ColorRole::BgBrand | ColorRole::BgSelectedStrong | ColorRole::IconSelectedSecondary => t.accent,
        ColorRole::BgBrandHover => t.accent_hover,
        ColorRole::BgBrandPressed => t.accent_pressed,
        ColorRole::BgBrandSecondary => shades.secondary,
        ColorRole::BgBrandTertiary => shades.subtle,
        ColorRole::BgSelected | ColorRole::BgSelectedPressed => t.selected_row,
        ColorRole::BgSelectedHover | ColorRole::BgOnSelected => shades.selected_hover,
        ColorRole::BgSelectedSecondary | ColorRole::BgSelectedTertiary => t.selected_row_child,
        ColorRole::TextBrand | ColorRole::TextSelected | ColorRole::IconBrand | ColorRole::IconSelected => t.accent_text,
        ColorRole::TextOnBrand => t.accent_on,
        ColorRole::Border => t.border,
        ColorRole::BorderSelected => t.focus,
        ColorRole::BorderSelectedStrong | ColorRole::BorderBrandStrong => t.accent_text,
        ColorRole::BorderBrand => shades.secondary,
        ColorRole::IconPressed => t.accent_hover,
        _ => role.value(dark),
    }
}

fn token_value(ui: &mut Ui, role: ColorRole, dark: bool, recipe: ThemeRecipe) {
    let background = preview_role_color(ColorRole::Bg, dark, recipe);
    let border = preview_role_color(ColorRole::Border, dark, recipe);
    let color = preview_role_color(role, dark, recipe);
    let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), background);
    ui.painter().rect_filled(rect.shrink(2.0), CornerRadius::same(2), color);
    ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, border), StrokeKind::Inside);
    let [red, green, blue, alpha] = color.to_array();
    let hex = if alpha == 255 { format!("#{red:02X}{green:02X}{blue:02X}") } else { format!("#{red:02X}{green:02X}{blue:02X}{alpha:02X}") };
    if ui.small_button(&hex).lizard_tooltip("Copy color value").clicked() {
        ui.ctx().copy_text(hex);
    }
}

fn tokens(ui: &mut Ui, state: &mut State) {
    ui.label("Lizard semantic roles · click a value to copy its hex code.");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.add(egui::TextEdit::singleline(&mut state.token_search).hint_text("Name, group, or usage").desired_width(250.0));
        if ui.small_button("Clear").clicked() {
            state.token_search.clear();
        }
    });
    ui.add_space(12.0);
    let query = state.token_search.trim().to_lowercase();
    let recipe = state.theme_recipe;
    for group in ["Surfaces", "Brand and status", "Selection", "Text", "Borders", "Icons"] {
        let roles: Vec<_> = ALL_COLOR_ROLES
            .iter()
            .copied()
            .filter(|role| {
                role.group() == group
                    && (query.is_empty()
                        || role.name().to_lowercase().contains(&query)
                        || role.group().to_lowercase().contains(&query)
                        || role.usage().to_lowercase().contains(&query))
            })
            .collect();
        if roles.is_empty() {
            continue;
        }
        example(ui, group, "Semantic role · light value · dark value · intended use", |ui| {
            egui::Grid::new(("color_roles", group)).spacing(vec2(12.0, 5.0)).striped(true).show(ui, |ui| {
                ui.strong("Role");
                ui.strong("Light");
                ui.strong("Dark");
                ui.strong("Use");
                ui.end_row();
                for role in roles {
                    ui.label(egui::RichText::new(role.name()).monospace().size(10.0));
                    ui.horizontal(|ui| token_value(ui, role, false, recipe));
                    ui.horizontal(|ui| token_value(ui, role, true, recipe));
                    ui.label(egui::RichText::new(role.usage()).size(10.0));
                    ui.end_row();
                }
            });
        });
    }
}

fn canvas_feedback(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    example(ui, "Insertion target", "The line drawn while a layer is dragged into the tree.", |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(320.0, 45.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
        widgets::insertion_line(ui.painter(), rect.left() + 12.0, rect.right() - 12.0, rect.center().y);
    });
    example(ui, "Canvas banner", "The overlay container used for canvas notices.", |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(320.0, 75.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.canvas);
        widgets::canvas_banner(ui.ctx(), Id::new("catalog_canvas_banner"), rect, |ui| {
            ui.label("Selection updated");
        });
    });
    example(ui, "Drag ghost", "The floating label follows a dragged item.", |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(320.0, 65.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.canvas);
        let galley = ui.painter().layout_no_wrap("Example layer".into(), theme::regular(11.0), t.text);
        widgets::drag_ghost(ui.ctx(), rect.center(), galley);
    });
}
