//! Lizard semantic light and dark design tokens, product surfaces, and fonts.

mod colors;

pub use colors::{ALL_COLOR_ROLES, ColorRole};

use std::sync::Arc;

use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Stroke, TextStyle, Visuals};
use workbench_ui::ThemeRecipe;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    /// The color picker can follow the host application's panel color.
    pub picker_surface: Color32,
    pub canvas: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    /// Dim text for disabled controls, placeholders and hints.
    pub text_tertiary: Color32,
    pub icon: Color32,
    pub icon_secondary: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub field: Color32,
    pub surface_tertiary: Color32,
    pub field_hover_border: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub accent_on: Color32,
    pub accent_text: Color32,
    pub canvas_accent: Color32,
    pub text_selection_bg: Color32,
    pub selected_row: Color32,
    pub selected_row_child: Color32,
    pub component: Color32,
    pub component_row: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub guide: Color32,
    pub tab_bar: Color32,
    pub tab_active: Color32,
    pub shadow: Color32,
    pub menu: Color32,
    pub menu_text: Color32,
    pub menu_hover: Color32,
    pub menu_dim: Color32,
    /// The ring around the control with the keyboard (keyboard navigation): at least 3:1 against
    /// the panels and the canvas.
    pub focus: Color32,
}

/// Keep the branch's blue accent across controls and canvas overlays.
pub const ACCENT: Color32 = Color32::from_rgb(0x0D, 0x99, 0xFF);
pub const ACCENT_ON: Color32 = Color32::WHITE;
/// Text selection in every editor and input, including the component catalog.
pub const TEXT_SELECTION_BG: Color32 = Color32::from_rgb(0x25, 0x56, 0x81);
/// Canvas selection colour adjusted to the visible page background, when one is painted.
pub fn canvas_accent(ctx: &Context) -> Color32 {
    ctx.data(|data| data.get_temp(canvas_accent_id())).unwrap_or_else(|| Tokens::get(ctx).canvas_accent)
}

fn canvas_accent_id() -> egui::Id {
    egui::Id::new("lizard_canvas_accent")
}

/// Keep selection outlines readable on custom page backgrounds without changing the theme hue.
pub fn set_canvas_background(ctx: &Context, background: Color32) {
    let base = Tokens::get(ctx).canvas_accent;
    ctx.data_mut(|data| data.insert_temp(canvas_accent_id(), contrasted_canvas_accent(base, background)));
}

fn contrasted_canvas_accent(base: Color32, background: Color32) -> Color32 {
    let bg = lizard_color::Color::from_rgb8_const(background.r(), background.g(), background.b());
    let contrast = |color: Color32| lizard_color::Color::from_rgb8_const(color.r(), color.g(), color.b()).contrast(bg);
    const MIN_CONTRAST: f32 = 4.0;
    if contrast(base) >= MIN_CONTRAST {
        return base;
    }
    let blend = |toward: Color32, amount: f32| {
        let channel = |a: u8, b: u8| (f32::from(a) * (1.0 - amount) + f32::from(b) * amount).round() as u8;
        Color32::from_rgb(channel(base.r(), toward.r()), channel(base.g(), toward.g()), channel(base.b(), toward.b()))
    };
    let fit = |toward: Color32| {
        if contrast(toward) < MIN_CONTRAST {
            return None;
        }
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..10 {
            let middle = (low + high) / 2.0;
            if contrast(blend(toward, middle)) >= MIN_CONTRAST {
                high = middle;
            } else {
                low = middle;
            }
        }
        Some((blend(toward, high), high))
    };
    match (fit(Color32::WHITE), fit(Color32::BLACK)) {
        (Some(light), Some(dark)) => if light.1 < dark.1 { light.0 } else { dark.0 },
        (Some(light), None) => light.0,
        (None, Some(dark)) => dark.0,
        (None, None) => base,
    }
}

/// Preserve the branch's blue fill under white text.
pub fn under_white(c: Color32) -> Color32 {
    c
}

pub const COMPONENT: Color32 = Color32::from_rgb(0xF7, 0x25, 0x85);
/// Palette exported from the Workbench UI theme editor for the Lizard application.
pub const LIZARD_THEME: ThemeRecipe = ThemeRecipe { darkness: 80, primary: [0x73, 0x5D, 0xF7] };
/// Handoff status badges (white text on these, in both themes).
pub const DEV_READY: Color32 = Color32::from_rgb(0x14, 0xAE, 0x5C);
pub const DEV_CHANGED: Color32 = Color32::from_rgb(0xD9, 0x7A, 0x06);
pub const DEV_COMPLETED: Color32 = Color32::from_rgb(0x6E, 0x6E, 0x73);

impl Tokens {
    pub fn lizard(dark: bool) -> Self {
        let mut tokens = Self::from_recipe(dark, LIZARD_THEME);
        tokens.picker_surface = tokens.panel;
        tokens
    }

    pub fn light() -> Self {
        let color = |role: ColorRole| role.value(false);
        Self {
            dark: false,
            bg: color(ColorRole::Bg),
            panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            picker_surface: Color32::from_rgb(0x2C, 0x2C, 0x2C),
            canvas: Color32::from_rgb(0xF5, 0xF5, 0xF5),
            border: color(ColorRole::Border),
            border_strong: Color32::from_rgb(0xD9, 0xD9, 0xD9),
            text: color(ColorRole::Text),
            text_secondary: color(ColorRole::TextSecondary),
            text_tertiary: color(ColorRole::TextTertiary),
            icon: color(ColorRole::Icon),
            icon_secondary: color(ColorRole::IconSecondary),
            hover: color(ColorRole::BgHover),
            pressed: color(ColorRole::BgPressed),
            field: color(ColorRole::BgSecondary),
            surface_tertiary: color(ColorRole::BgTertiary),
            field_hover_border: Color32::from_rgb(0xE6, 0xE6, 0xE6),
            accent: color(ColorRole::BgBrand),
            accent_hover: color(ColorRole::BgBrandHover),
            accent_pressed: color(ColorRole::BgBrandPressed),
            accent_on: ACCENT_ON,
            accent_text: color(ColorRole::TextBrand),
            canvas_accent: ACCENT,
            text_selection_bg: TEXT_SELECTION_BG,
            selected_row: color(ColorRole::BgSelected),
            selected_row_child: color(ColorRole::BgSelectedSecondary),
            component: color(ColorRole::BgComponent),
            component_row: color(ColorRole::BgComponentTertiary),
            danger: color(ColorRole::BgDanger),
            success: color(ColorRole::BgSuccess),
            warning: color(ColorRole::BgWarning),
            guide: Color32::from_rgb(0xF2, 0x48, 0x22),
            tab_bar: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            tab_active: color(ColorRole::BgSecondary),
            shadow: Color32::from_black_alpha(40),
            menu: Color32::from_rgb(0x1E, 0x1E, 0x1E),
            menu_text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            menu_hover: color(ColorRole::BgMenuHover),
            menu_dim: Color32::from_rgba_unmultiplied(255, 255, 255, 178),
            focus: color(ColorRole::BorderSelected),
        }
    }

    pub fn dark() -> Self {
        let color = |role: ColorRole| role.value(true);
        Self {
            dark: true,
            bg: color(ColorRole::Bg),
            panel: Color32::from_rgb(0x2B, 0x2B, 0x2B),
            picker_surface: Color32::from_rgb(0x2C, 0x2C, 0x2C),
            canvas: Color32::from_rgb(0x1E, 0x1E, 0x1E),
            border: color(ColorRole::Border),
            border_strong: Color32::from_rgb(0x55, 0x55, 0x55),
            text: color(ColorRole::Text),
            text_secondary: color(ColorRole::TextSecondary),
            text_tertiary: color(ColorRole::TextTertiary),
            icon: color(ColorRole::Icon),
            icon_secondary: color(ColorRole::IconSecondary),
            hover: color(ColorRole::BgHover),
            pressed: color(ColorRole::BgPressed),
            field: color(ColorRole::BgSecondary),
            surface_tertiary: color(ColorRole::BgTertiary),
            field_hover_border: Color32::from_rgb(0x4D, 0x4D, 0x4D),
            accent: color(ColorRole::BgBrand),
            accent_hover: color(ColorRole::BgBrandHover),
            accent_pressed: color(ColorRole::BgBrandPressed),
            accent_on: ACCENT_ON,
            accent_text: color(ColorRole::TextBrand),
            canvas_accent: ACCENT,
            text_selection_bg: TEXT_SELECTION_BG,
            selected_row: color(ColorRole::BgSelected),
            selected_row_child: color(ColorRole::BgSelectedSecondary),
            component: color(ColorRole::BgComponent),
            component_row: color(ColorRole::BgComponentTertiary),
            danger: color(ColorRole::BgDanger),
            success: color(ColorRole::BgSuccess),
            warning: color(ColorRole::BgWarning),
            guide: Color32::from_rgb(0xF2, 0x48, 0x22),
            tab_bar: Color32::from_rgb(0x2B, 0x2B, 0x2B),
            tab_active: color(ColorRole::BgSecondary),
            shadow: Color32::from_black_alpha(90),
            menu: Color32::from_rgb(0x1E, 0x1E, 0x1E),
            menu_text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            menu_hover: color(ColorRole::BgMenuHover),
            menu_dim: Color32::from_rgba_unmultiplied(255, 255, 255, 178),
            focus: color(ColorRole::BorderSelected),
        }
    }

    /// Preview a portable Workbench recipe without changing the editor's saved theme.
    pub fn from_recipe(dark: bool, recipe: ThemeRecipe) -> Self {
        let mut t = if dark { Self::dark() } else { Self::light() };
        if dark && recipe.darkness != 50 {
            for neutral in [
                &mut t.bg,
                &mut t.panel,
                &mut t.canvas,
                &mut t.border,
                &mut t.border_strong,
                &mut t.hover,
                &mut t.pressed,
                &mut t.field,
                &mut t.surface_tertiary,
                &mut t.field_hover_border,
                &mut t.tab_bar,
                &mut t.tab_active,
                &mut t.menu,
            ] {
                *neutral = recipe.dark_neutral(*neutral);
            }
        }
        if recipe.primary != ThemeRecipe::default().primary {
            let shades = recipe.primary_scale(dark, t.panel);
            t.accent = shades.base;
            t.accent_hover = shades.hover;
            t.accent_pressed = shades.pressed;
            t.accent_on = shades.on_primary;
            t.accent_text = shades.text;
            t.canvas_accent = shades.base;
            t.text_selection_bg = shades.base;
            t.selected_row = shades.selected;
            t.selected_row_child = shades.selected_child;
            t.menu_hover = shades.base;
            t.focus = shades.base;
        }
        t
    }

    /// Stable semantic names and their resolved colors for export by a host app.
    pub fn named_colors(self) -> [(&'static str, Color32); 38] {
        [
            ("bg", self.bg),
            ("panel", self.panel),
            ("canvas", self.canvas),
            ("border", self.border),
            ("border-strong", self.border_strong),
            ("text", self.text),
            ("text-secondary", self.text_secondary),
            ("text-tertiary", self.text_tertiary),
            ("icon", self.icon),
            ("icon-secondary", self.icon_secondary),
            ("hover", self.hover),
            ("pressed", self.pressed),
            ("field", self.field),
            ("surface-tertiary", self.surface_tertiary),
            ("field-hover-border", self.field_hover_border),
            ("accent", self.accent),
            ("accent-hover", self.accent_hover),
            ("accent-pressed", self.accent_pressed),
            ("accent-on", self.accent_on),
            ("accent-text", self.accent_text),
            ("canvas-accent", self.canvas_accent),
            ("text-selection-bg", self.text_selection_bg),
            ("selected-row", self.selected_row),
            ("selected-row-child", self.selected_row_child),
            ("component", self.component),
            ("component-row", self.component_row),
            ("danger", self.danger),
            ("success", self.success),
            ("warning", self.warning),
            ("guide", self.guide),
            ("tab-bar", self.tab_bar),
            ("tab-active", self.tab_active),
            ("shadow", self.shadow),
            ("menu", self.menu),
            ("menu-text", self.menu_text),
            ("menu-hover", self.menu_hover),
            ("menu-dim", self.menu_dim),
            ("focus", self.focus),
        ]
    }

    pub fn get(ctx: &Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(tokens_id())).unwrap_or_else(Tokens::light)
    }
}

fn tokens_id() -> egui::Id {
    egui::Id::new("lizard_tokens")
}

/// egui's default for how long hover fades and openings animate.
const ANIMATION_TIME: f32 = 0.2;

/// Install tokens and egui visuals.
pub fn apply(ctx: &Context, t: Tokens) {
    apply_with(ctx, t, false);
}

/// [`apply`], with egui's own animations (hover fades, openings, smooth scrolling to a target) off
/// when the system or the preferences ask for reduced motion. Prototype transitions still play.
pub fn apply_with(ctx: &Context, t: Tokens, reduce_motion: bool) {
    ctx.data_mut(|d| {
        d.insert_temp(tokens_id(), t);
        d.remove::<Color32>(canvas_accent_id());
    });
    ctx.set_visuals(visuals(t));
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size = egui::vec2(24.0, 24.0);
        s.spacing.menu_margin = egui::Margin::same(8);
        s.spacing.window_margin = egui::Margin::same(12);
        s.text_styles = [
            (TextStyle::Small, FontId::new(ui_font_size(10.0), FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(ui_font_size(11.0), FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(ui_font_size(11.0), FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(ui_font_size(13.0), FontFamily::Name("semibold".into()))),
            (TextStyle::Monospace, FontId::new(ui_font_size(11.0), FontFamily::Monospace)),
        ]
        .into();
        s.interaction.tooltip_delay = 0.4;
        s.animation_time = if reduce_motion { 0.0 } else { ANIMATION_TIME };
        s.scroll_animation = if reduce_motion { egui::style::ScrollAnimation::none() } else { egui::style::ScrollAnimation::default() };
    });
}

/// egui visuals for tokens `t`.
fn visuals(t: Tokens) -> Visuals {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.panel;
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.hover;
    v.override_text_color = Some(t.text);
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_corner_radius = egui::CornerRadius::same(13);
    v.menu_corner_radius = egui::CornerRadius::same(13);
    v.selection.bg_fill = t.text_selection_bg;
    v.selection.stroke = Stroke::new(1.0, t.accent_on);
    v.hyperlink_color = t.accent_text;
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open, &mut v.widgets.noninteractive] {
        w.corner_radius = egui::CornerRadius::same(5);
        w.fg_stroke = Stroke::new(1.0, t.text);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.bg_fill = t.panel;
    v.widgets.inactive.bg_fill = t.field;
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.field_hover_border);
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.bg_stroke = Stroke::new(1.0, t.accent);
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: t.shadow };
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: t.shadow };
    v.text_cursor.stroke = Stroke::new(1.0, t.accent);
    v
}

/// Draw a preview with a specified Lizard theme without changing the surrounding window.
pub fn with_theme<R>(ui: &mut egui::Ui, theme: Tokens, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    let saved = Tokens::get(&ctx);
    ctx.data_mut(|d| d.insert_temp(tokens_id(), theme));
    let r = ui
        .scope(|ui| {
            *ui.visuals_mut() = visuals(theme);
            f(ui)
        })
        .inner;
    ctx.data_mut(|d| d.insert_temp(tokens_id(), saved));
    r
}

/// Draw `f` with the dark tokens and visuals: controls on dark popovers whichever theme the app shows.
pub fn on_dark<R>(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    with_theme(ui, Tokens::dark(), f)
}

/// UI fonts: bundled NV Legible Next for Latin text, with system and optional craft-fonts
/// fallbacks for scripts and symbols it does not cover. Document text keeps its own font choices.
pub fn install_fonts(ctx: &Context, system: bool, cjk: u8) {
    let mut fonts = FontDefinitions::default();
    // NV Legible Next's visible glyphs sit high inside its line box. Move the ink down
    // without changing layout metrics, so labels center in every control at every UI size.
    let ui_font_tweak = FontTweak { y_offset_factor: 0.20, ..Default::default() };
    fonts.font_data.insert(
        "nv-legible-next-regular".into(),
        Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/nv-legible-next/NV_Legible_Next-Regular.ttf")).tweak(ui_font_tweak.clone())),
    );
    fonts.font_data.insert(
        "nv-legible-next-bold".into(),
        Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/nv-legible-next/NV_Legible_Next-Bold.ttf")).tweak(ui_font_tweak)),
    );
    if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        list.insert(0, "nv-legible-next-regular".into());
    }
    let proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.insert(FontFamily::Name("medium".into()), proportional.clone());
    let mut semi = vec!["nv-legible-next-bold".into()];
    semi.extend(proportional);
    fonts.families.insert(FontFamily::Name("semibold".into()), semi);
    // CJK and other scripts from craft-fonts as fallbacks (Noto Sans CJK only with LIZARD_EMBED_CJK).
    const BUILT_IN: [&str; 4] = ["BIZ UDPGothic", "Noto Sans CJK SC", "Noto Sans Arabic", "Noto Sans Georgian"];
    let lang = crate::i18n::current().map_or("en", |l| l.code);
    // A Chinese UI draws Han in a Chinese face: the built-in Japanese one goes after the system faces.
    let late = |family: &str| family == "BIZ UDPGothic" && crate::cjk::language(lang) == crate::cjk::HAN;
    let craft = |last: bool| {
        lizard_text::CRAFT_FONTS
            .iter()
            .filter(move |f| f.style == "Regular" && BUILT_IN.contains(&f.family) && late(f.family) == last)
            .map(|f| (format!("craft-{}", f.family), FontData::from_static(f.bytes)))
    };
    let mut extra: Vec<(String, FontData)> = craft(false).collect();
    if system {
        // What the built-in faces above already draw.
        let embedded = |c: char| {
            BUILT_IN
                .into_iter()
                .filter(|f| !late(f) && lizard_text::CRAFT_FONTS.iter().any(|cf| cf.family == *f))
                .filter_map(|f| lizard_text::find_face(f, "Regular"))
                .any(|x| x.has_char(c))
        };
        let mut taken = Vec::new();
        extra.extend(crate::bidi::script_fallbacks(&embedded, &mut taken));
        extra.extend(key_symbol_fallbacks(&embedded, &mut taken));
        extra.extend(crate::cjk::faces(cjk, lang, &embedded).iter().map(|f| (format!("system-{}-{}", f.family, f.style), font_data(f))));
    }
    extra.extend(craft(true));
    for (key, data) in extra {
        fonts.font_data.insert(key.clone(), Arc::new(data));
        for fam in [FontFamily::Proportional, FontFamily::Name("medium".into()), FontFamily::Name("semibold".into())] {
            if let Some(list) = fonts.families.get_mut(&fam) {
                list.push(key.clone());
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// The glyphs macOS shortcuts are written with ([`lizard_engine::keymap::display`]).
const KEY_SYMBOLS: &str = "⌃⌥⇧⌘⌫↵⇥⇞⇟←→↑↓";
/// System families that have them, in order of preference.
const KEY_FAMILIES: [&str; 6] = ["Lucida Grande", "Apple Symbols", "Segoe UI Symbol", "DejaVu Sans", "Noto Sans Symbols 2", "FreeSans"];

/// System faces for the shortcut glyphs the UI's own fonts lack (egui's default face has ⌘ but
/// not ⌥ or ↵); `taken` as for [`system_fallbacks`].
fn key_symbol_fallbacks(have: &dyn Fn(char) -> bool, taken: &mut Vec<Arc<lizard_text::Face>>) -> Vec<(String, FontData)> {
    system_fallbacks(KEY_SYMBOLS.chars(), &KEY_FAMILIES, have, taken)
}

/// System faces, the first of `families` that has each, for the `samples` neither the UI's own
/// fonts (`have`) nor the faces already `taken` draw. Each face found joins `taken`, so a face that
/// several lists name is added once.
pub(crate) fn system_fallbacks(
    samples: impl IntoIterator<Item = char>,
    families: &[&str],
    have: &dyn Fn(char) -> bool,
    taken: &mut Vec<Arc<lizard_text::Face>>,
) -> Vec<(String, FontData)> {
    let mut out = Vec::new();
    for c in samples {
        if have(c) || taken.iter().any(|f| f.has_char(c)) {
            continue;
        }
        let Some(face) = lizard_text::face_with_char(c, families) else { continue };
        out.push((format!("system-{}-{}", face.family, face.style), font_data(&face)));
        taken.push(face);
    }
    out
}

/// egui font data over a face's bytes, without copying them (CJK faces are 10–20 MB). Font
/// database faces are never dropped, so a system face's shared bytes are kept for good: leaking
/// one more reference to them costs a pointer and gives egui the `'static` slice it needs.
pub fn font_data(face: &lizard_text::Face) -> FontData {
    let bytes: &'static [u8] = match &face.bytes {
        lizard_text::fontdb::FontBytes::Static(b) => b,
        lizard_text::fontdb::FontBytes::Shared(v) => Box::leak(Box::new(v.clone())).as_slice(),
    };
    let mut data = FontData::from_static(bytes);
    data.index = face.index;
    data
}

/// Scale the interface type ramp so its smallest fixed size, 8.5, renders at 10 logical points.
pub fn ui_font_size(size: f32) -> f32 {
    (size * (10.0 / 8.5)).max(10.0)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(ui_font_size(size), FontFamily::Name("semibold".into()))
}

pub fn medium(size: f32) -> FontId {
    FontId::new(ui_font_size(size), FontFamily::Name("medium".into()))
}

/// Shared typography for property-panel section headings.
pub fn property_section_title() -> FontId {
    medium(10.5)
}

pub fn regular(size: f32) -> FontId {
    FontId::new(ui_font_size(size), FontFamily::Proportional)
}

pub fn c32(c: lizard_color::Color) -> Color32 {
    let [r, g, b, a] = c.to_rgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn screen_conversion_id() -> egui::Id {
    egui::Id::new("lizard_screen_conversion")
}

/// Remember the colour profile of the document on screen (once per frame): document colours drawn
/// by the UI ([`doc_c32`]) go from it to the screen's sRGB.
pub fn set_doc_profile(ctx: &Context, profile: lizard_color::ColorProfile) {
    let conv = profile.conversion_to(lizard_color::ColorProfile::Srgb);
    ctx.data_mut(|d| d.insert_temp(screen_conversion_id(), conv));
}

/// A document colour (a paint, swatch, background) for the screen: converted from the document's
/// colour profile to sRGB. UI chrome colours use [`c32`].
pub fn doc_c32(ctx: &Context, c: lizard_color::Color) -> Color32 {
    let conv = ctx.data(|d| d.get_temp::<lizard_color::ProfileConversion>(screen_conversion_id()));
    c32(conv.map_or(c, |k| k.apply(c)))
}

#[cfg(test)]
mod ui_font_tests {
    use super::*;

    #[test]
    fn lizard_picker_follows_properties_without_changing_catalog_surface() {
        let lizard = Tokens::lizard(true);
        assert_eq!(lizard.picker_surface, lizard.panel);
        assert_eq!(Tokens::dark().picker_surface, crate::primitives::FloatingPanel::DARK_SURFACE);
        assert_eq!(Tokens::light().picker_surface, crate::primitives::FloatingPanel::DARK_SURFACE);
    }

    #[test]
    fn available_ui_fonts_render_antialiased_glyphs() {
        let ctx = Context::default();
        install_fonts(&ctx, false, 0);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Ag 123");
        });
        output.textures_delta.clear();
        ctx.fonts(|fonts| {
            let defs = fonts.definitions();
            assert!(!defs.families[&FontFamily::Proportional].is_empty());
            assert!(!defs.families[&FontFamily::Name("semibold".into())].is_empty());
            assert!(fonts.image().pixels.iter().any(|p| (1..255).contains(&p.a())));
        });
    }
}
