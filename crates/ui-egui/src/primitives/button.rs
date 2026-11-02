//! Composable button styles. Callers own commands, menus, and toggle state.

use crate::primitives::TooltipResponseExt as _;
use egui::{Align2, Color32, CornerRadius, Id, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::{CONTROL_CHEVRON_SIZE, ControlWidth};
use crate::a11y;
use crate::icons::{self, HeaderIcon, Icon, LayoutIcon, VectorPointIcon};
use crate::theme::{ColorRole, Tokens, medium};
use crate::widgets;

/// Visual treatment, independent of what clicking the button does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Secondary,
    Danger,
    DangerSecondary,
    DangerGhost,
    DangerLink,
    Ghost,
    /// Sidebar navigation: a solid accent when selected.
    Navigation,
    Link,
    Input,
    Property,
    Overlay,
}

/// What assistive technology should hear. The caller owns the associated action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonBehavior {
    #[default]
    Action,
    Toggle {
        selected: bool,
    },
    Select {
        expanded: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonAlign {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    #[default]
    Regular,
    Large,
}

/// A button may hold text, a leading or trailing icon, or an icon alone. `show` lays it out;
/// `show_at` paints into a rectangle owned by an existing composition.
pub struct Button<'a> {
    label: &'a str,
    variant: ButtonVariant,
    behavior: ButtonBehavior,
    align: ButtonAlign,
    size: ButtonSize,
    width: ControlWidth,
    leading: Option<Icon>,
    header_icon: Option<HeaderIcon>,
    layout_icon: Option<LayoutIcon>,
    vector_point_icon: Option<VectorPointIcon>,
    trailing: Option<Icon>,
    trailing_at_end: bool,
    icon_only: bool,
    icon_size: Option<f32>,
    enabled: bool,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            variant: ButtonVariant::Primary,
            behavior: ButtonBehavior::Action,
            align: ButtonAlign::Center,
            size: ButtonSize::Regular,
            width: ControlWidth::Hug,
            leading: None,
            header_icon: None,
            layout_icon: None,
            vector_point_icon: None,
            trailing: None,
            trailing_at_end: false,
            icon_only: false,
            icon_size: None,
            enabled: true,
        }
    }

    pub fn icon(icon: Icon, accessible_name: &'a str) -> Self {
        Self::new(accessible_name).leading(icon).icon_only(true).variant(ButtonVariant::Ghost)
    }

    /// A header SVG with the same button interaction and states as the standard icon button.
    pub fn header_icon(icon: HeaderIcon, accessible_name: &'a str) -> Self {
        let mut button = Self::new(accessible_name).icon_only(true).variant(ButtonVariant::Ghost);
        button.header_icon = Some(icon);
        button
    }

    pub fn layout_icon(icon: LayoutIcon, accessible_name: &'a str) -> Self {
        let mut button = Self::new(accessible_name).icon_only(true).variant(ButtonVariant::Ghost);
        button.layout_icon = Some(icon);
        button
    }

    pub fn vector_point_icon(icon: VectorPointIcon, accessible_name: &'a str) -> Self {
        let mut button = Self::new(accessible_name).icon_only(true).variant(ButtonVariant::Ghost);
        button.vector_point_icon = Some(icon);
        button
    }

    pub fn icon_size(mut self, size: f32) -> Self {
        self.icon_size = Some(size);
        self
    }

    pub fn variant(mut self, value: ButtonVariant) -> Self {
        self.variant = value;
        if value == ButtonVariant::Property {
            self.align = ButtonAlign::Start;
            self.width = ControlWidth::Fill;
        }
        self
    }

    pub fn behavior(mut self, value: ButtonBehavior) -> Self {
        self.behavior = value;
        self
    }

    pub fn align(mut self, value: ButtonAlign) -> Self {
        self.align = value;
        self
    }

    pub fn size(mut self, value: ButtonSize) -> Self {
        self.size = value;
        self
    }

    pub fn width(mut self, value: ControlWidth) -> Self {
        self.width = value;
        self
    }

    pub fn leading(mut self, value: Icon) -> Self {
        self.leading = Some(value);
        self
    }

    pub fn trailing(mut self, value: Icon) -> Self {
        self.trailing = Some(value);
        self
    }

    pub fn trailing_at_end(mut self, value: bool) -> Self {
        self.trailing_at_end = value;
        self
    }

    pub fn icon_only(mut self, value: bool) -> Self {
        self.icon_only = value;
        self
    }

    pub fn enabled(mut self, value: bool) -> Self {
        self.enabled = value;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        if !self.enabled {
            return ui.add_enabled_ui(false, |ui| self.show_enabled(ui)).inner;
        }
        self.show_enabled(ui)
    }

    fn show_enabled(self, ui: &mut Ui) -> Response {
        let width = self.width.resolve(ui, self.intrinsic_width(ui));
        let (rect, response) = ui.allocate_exact_size(vec2(width, self.height()), Sense::click());
        self.paint(ui, rect, response)
    }

    pub fn show_at(self, ui: &mut Ui, id: Id, rect: Rect) -> Response {
        if !self.enabled {
            return ui.add_enabled_ui(false, |ui| self.show_at_enabled(ui, id, rect)).inner;
        }
        self.show_at_enabled(ui, id, rect)
    }

    fn show_at_enabled(self, ui: &mut Ui, id: Id, rect: Rect) -> Response {
        let response = ui.interact(rect, id, Sense::click());
        self.paint(ui, rect, response)
    }

    fn height(&self) -> f32 {
        match self.size {
            ButtonSize::Regular => 24.0,
            ButtonSize::Large => 32.0,
        }
    }

    fn intrinsic_width(&self, ui: &Ui) -> f32 {
        if self.icon_only {
            return self.height();
        }
        let label = ui.painter().layout_no_wrap(self.label.to_owned(), medium(11.0), Color32::WHITE).size().x;
        let leading = if self.leading.is_some() || self.header_icon.is_some() || self.layout_icon.is_some() || self.vector_point_icon.is_some() {
            18.0
        } else {
            0.0
        };
        let trailing = if self.trailing.is_some() || matches!(self.behavior, ButtonBehavior::Select { .. }) { 18.0 } else { 0.0 };
        label + leading + trailing + if self.size == ButtonSize::Large { 24.0 } else { 16.0 }
    }

    fn paint(self, ui: &mut Ui, rect: Rect, response: Response) -> Response {
        let t = Tokens::get(ui.ctx());
        match self.behavior {
            ButtonBehavior::Action => a11y::named_button(&response, self.label),
            ButtonBehavior::Toggle { selected } => a11y::toggle(&response, self.label, selected),
            ButtonBehavior::Select { expanded } => a11y::combo(&response, self.label, self.label, expanded || a11y::menu_open(&response)),
        }
        let selected = matches!(self.behavior, ButtonBehavior::Toggle { selected: true });
        let palette = palette(t, self.variant, self.enabled, selected);
        let animation = ui.style().animation_time.min(0.08);
        let hover = if self.enabled { ui.ctx().animate_bool_with_time(response.id, response.hovered(), animation) } else { 0.0 };
        let pressed = self.enabled && response.is_pointer_button_down_on();
        let fill = if pressed { palette.pressed } else { blend(palette.normal, palette.hover, hover) };
        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
        if palette.border != Color32::TRANSPARENT {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, palette.border), StrokeKind::Inside);
        } else if self.variant == ButtonVariant::Input && hover > 0.0 {
            let border = translucent_border(t).gamma_multiply(hover);
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, border), StrokeKind::Inside);
        }
        if response.has_focus() && self.enabled {
            ui.painter().rect_stroke(rect.expand(1.0), CornerRadius::same(6), Stroke::new(1.0, t.focus), StrokeKind::Outside);
        }

        let content = rect.shrink2(vec2(if self.size == ButtonSize::Large { 12.0 } else { 8.0 }, 0.0));
        let ink = if !self.enabled {
            palette.disabled_ink
        } else if pressed {
            palette.pressed_ink
        } else {
            palette.ink
        };
        if self.icon_only {
            let size = self.icon_size.unwrap_or(if self.size == ButtonSize::Large { 18.0 } else { 16.0 }).min(rect.width()).min(rect.height());
            if let Some(icon) = self.header_icon {
                icons::paint_header(ui.painter(), Rect::from_center_size(rect.center(), vec2(size, size)), icon, ink);
            } else if let Some(icon) = self.layout_icon {
                icons::paint_layout(ui.painter(), Rect::from_center_size(rect.center(), vec2(size, size)), icon, ink);
            } else if let Some(icon) = self.vector_point_icon {
                icons::paint_vector_point(ui.painter(), Rect::from_center_size(rect.center(), vec2(size, size)), icon, ink);
            } else if let Some(icon) = self.leading {
                clip_icon(ui.painter(), icon, rect.center(), ink, size);
            }
            return response.lizard_tooltip(self.label);
        }
        let trailing = self.trailing.or(if matches!(self.behavior, ButtonBehavior::Select { .. }) { Some(Icon::Chevron) } else { None });
        let select = matches!(self.behavior, ButtonBehavior::Select { .. });
        let trailing_at_end = select || self.trailing_at_end;
        let icon_w = if self.size == ButtonSize::Large { 16.0 } else { 14.0 };
        let gap = 4.0;
        let leading_w = if self.leading.is_some() { icon_w + gap } else { 0.0 };
        let trailing_w = if trailing.is_some() { icon_w + gap } else { 0.0 };
        let text_area = if trailing_at_end {
            Rect::from_min_max(content.min, pos2((content.max.x - trailing_w).max(content.min.x), content.max.y))
        } else {
            content
        };
        let available_text = (text_area.width() - leading_w - if trailing_at_end { 0.0 } else { trailing_w }).max(0.0);
        let shown = widgets::elide_font(ui, self.label, available_text, medium(11.0));
        let text_w = ui.painter().layout_no_wrap(shown.clone(), medium(11.0), ink).size().x;
        let group_w = text_w + leading_w + if trailing_at_end { 0.0 } else { trailing_w };
        let start = match self.align {
            ButtonAlign::Start => text_area.min.x,
            ButtonAlign::Center => text_area.center().x - group_w / 2.0,
            ButtonAlign::End => text_area.max.x - group_w,
        };
        let clip = ui.painter().with_clip_rect(content);
        if let Some(icon) = self.leading {
            clip_icon(&clip, icon, pos2(start + icon_w / 2.0, rect.center().y), ink, icon_w);
        }
        clip.text(pos2(start + leading_w, rect.center().y), Align2::LEFT_CENTER, shown, medium(11.0), ink);
        if let Some(icon) = trailing {
            let x = if trailing_at_end { content.max.x - icon_w / 2.0 } else { start + leading_w + text_w + gap + icon_w / 2.0 };
            clip_icon(&clip, icon, pos2(x, rect.center().y), ink, if icon == Icon::Chevron { CONTROL_CHEVRON_SIZE } else { icon_w });
        }
        response
    }
}

fn clip_icon(painter: &egui::Painter, icon: Icon, center: egui::Pos2, color: Color32, size: f32) {
    if icon == Icon::Chevron {
        icons::paint_compact_chevron(painter, Rect::from_center_size(center, vec2(size, size)), color);
    } else {
        icons::paint(painter, Rect::from_center_size(center, vec2(size, size)), icon, color);
    }
}

fn blend(a: Color32, b: Color32, amount: f32) -> Color32 {
    Color32::from(egui::lerp(egui::Rgba::from(a)..=egui::Rgba::from(b), amount))
}

fn translucent_border(t: Tokens) -> Color32 {
    ColorRole::BorderTranslucent.value(t.dark)
}

fn translucent_press(t: Tokens) -> Color32 {
    ColorRole::BgTransparentPressed.value(t.dark)
}

struct Palette {
    normal: Color32,
    hover: Color32,
    pressed: Color32,
    border: Color32,
    ink: Color32,
    pressed_ink: Color32,
    disabled_ink: Color32,
}

fn palette(t: Tokens, variant: ButtonVariant, enabled: bool, selected: bool) -> Palette {
    let clear = Color32::TRANSPARENT;
    let border = translucent_border(t);
    let color = |role: ColorRole| role.value(t.dark);
    let danger_ink = color(ColorRole::TextDanger);
    let danger_soft = color(ColorRole::BgDangerTertiary);
    let disabled_ink = color(ColorRole::TextDisabled);
    let (normal, hover, pressed, outline, ink) = match variant {
        ButtonVariant::Primary => (t.accent, t.accent_hover, t.accent_pressed, clear, t.accent_on),
        ButtonVariant::Secondary | ButtonVariant::Property => (clear, t.hover, translucent_press(t), border, t.text),
        ButtonVariant::Danger => {
            (color(ColorRole::BgDanger), color(ColorRole::BgDangerHover), color(ColorRole::BgDangerHover), clear, color(ColorRole::TextOnDanger))
        }
        ButtonVariant::DangerSecondary => (clear, danger_soft, danger_soft, color(ColorRole::BorderDanger), danger_ink),
        ButtonVariant::DangerGhost | ButtonVariant::DangerLink => {
            (clear, if variant == ButtonVariant::DangerGhost { danger_soft } else { clear }, danger_soft, clear, danger_ink)
        }
        ButtonVariant::Ghost => (clear, color(ColorRole::BgTransparentHover), translucent_press(t), clear, t.text),
        ButtonVariant::Navigation => (clear, t.hover, translucent_press(t), clear, t.text),
        ButtonVariant::Link => (clear, clear, t.selected_row, clear, t.accent_text),
        ButtonVariant::Input => (t.field, clear, translucent_press(t), clear, t.text),
        ButtonVariant::Overlay => (color(ColorRole::Bg), color(ColorRole::Bg), t.field, border, t.text),
    };
    if !enabled {
        let filled = matches!(variant, ButtonVariant::Primary | ButtonVariant::Danger | ButtonVariant::Input | ButtonVariant::Overlay);
        let background = if filled { color(ColorRole::BgDisabled) } else { clear };
        return Palette {
            normal: background,
            hover: background,
            pressed: background,
            border: clear,
            ink: disabled_ink,
            pressed_ink: disabled_ink,
            disabled_ink,
        };
    }
    if selected {
        if variant == ButtonVariant::Navigation {
            return Palette {
                normal: t.accent,
                hover: t.accent_hover,
                pressed: t.accent_pressed,
                border: clear,
                ink: t.accent_on,
                pressed_ink: t.accent_on,
                disabled_ink,
            };
        }
        let fill = t.selected_row;
        let ink = t.accent_text;
        return Palette { normal: fill, hover: fill, pressed: fill, border: clear, ink, pressed_ink: ink, disabled_ink };
    }
    Palette { normal, hover, pressed, border: outline, ink, pressed_ink: ink, disabled_ink }
}

#[cfg(test)]
mod theme_recipe_tests {
    use super::*;
    use workbench_ui::ThemeRecipe;

    #[test]
    fn primary_button_uses_the_edited_theme_colors() {
        let recipe = ThemeRecipe { primary: [0xc5, 0x68, 0xff], ..ThemeRecipe::default() };
        let tokens = Tokens::from_recipe(true, recipe);
        let primary = palette(tokens, ButtonVariant::Primary, true, false);
        assert_eq!(primary.normal, tokens.accent);
        assert_eq!(primary.hover, tokens.accent_hover);
        assert_eq!(primary.pressed, tokens.accent_pressed);
        assert_eq!(primary.ink, tokens.accent_on);
    }
}
