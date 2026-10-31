//! Two portable theme controls: neutral darkness and a derived primary color scale.
//! Host applications map the resulting colors to their own semantic tokens.

use egui::Color32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeRecipe {
    /// 0 is lighter, 100 is darker, and 50 preserves the host's base dark surfaces.
    pub darkness: u8,
    /// The one primary color from which interactive shades are derived.
    pub primary: [u8; 3],
}

impl Default for ThemeRecipe {
    fn default() -> Self {
        Self { darkness: 50, primary: [0x0D, 0x99, 0xFF] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimaryScale {
    pub base: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub secondary: Color32,
    pub subtle: Color32,
    pub text: Color32,
    pub selected: Color32,
    pub selected_hover: Color32,
    pub selected_child: Color32,
    pub on_primary: Color32,
}

impl ThemeRecipe {
    /// Shift a dark neutral without changing its alpha or its relative step in the palette.
    pub fn dark_neutral(self, color: Color32) -> Color32 {
        let delta = (50.0 - f32::from(self.darkness.min(100))) * 0.5;
        let [r, g, b, a] = color.to_array();
        let shift = |v: u8| (f32::from(v) + delta).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgba_unmultiplied(shift(r), shift(g), shift(b), a)
    }

    pub fn primary_scale(self, dark: bool, surface: Color32) -> PrimaryScale {
        let [r, g, b] = self.primary;
        let chosen = Color32::from_rgb(r, g, b);
        let base = if dark { mix(chosen, Color32::BLACK, 0.08) } else { chosen };
        let text = if dark { mix(base, Color32::WHITE, 0.45) } else { mix(base, Color32::BLACK, 0.18) };
        let on_primary = if luminance(base) > 0.179 { Color32::BLACK } else { Color32::WHITE };
        PrimaryScale {
            base,
            hover: mix(base, Color32::BLACK, 0.13),
            pressed: mix(base, Color32::BLACK, 0.27),
            secondary: mix(base, Color32::BLACK, 0.20),
            subtle: mix(surface, base, if dark { 0.17 } else { 0.10 }),
            text,
            selected: mix(surface, base, if dark { 0.19 } else { 0.12 }),
            selected_hover: mix(surface, base, if dark { 0.28 } else { 0.22 }),
            selected_child: mix(surface, base, if dark { 0.13 } else { 0.07 }),
            on_primary,
        }
    }
}

fn mix(a: Color32, b: Color32, fraction: f32) -> Color32 {
    let [ar, ag, ab, _] = a.to_array();
    let [br, bg, bb, _] = b.to_array();
    let channel = |a: u8, b: u8| (f32::from(a) * (1.0 - fraction) + f32::from(b) * fraction).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(channel(ar, br), channel(ag, bg), channel(ab, bb))
}

fn luminance(color: Color32) -> f32 {
    let [r, g, b, _] = color.to_array();
    let linear = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_slider_preserves_neutral_order_and_alpha() {
        let base = Color32::from_rgba_unmultiplied(44, 44, 44, 180);
        let lighter = ThemeRecipe { darkness: 0, ..ThemeRecipe::default() }.dark_neutral(base);
        let darker = ThemeRecipe { darkness: 100, ..ThemeRecipe::default() }.dark_neutral(base);
        assert!(lighter.r() > base.r() && darker.r() < base.r());
        assert_eq!(lighter.a(), 180);
        assert_eq!(darker.a(), 180);
    }

    #[test]
    fn primary_scale_tracks_selected_hue() {
        let surface = Color32::from_rgb(43, 43, 43);
        let red = ThemeRecipe { primary: [220, 40, 35], ..ThemeRecipe::default() }.primary_scale(true, surface);
        let green = ThemeRecipe { primary: [35, 200, 80], ..ThemeRecipe::default() }.primary_scale(true, surface);
        assert!(red.base.r() > red.base.g());
        assert!(red.selected.r() > red.selected.g());
        assert!(green.base.g() > green.base.r());
        assert!(green.selected.g() > green.selected.r());
    }
}
