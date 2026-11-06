//! A compact keyboard shortcut badge shared by menus, tooltips, and help surfaces.

use egui::{Color32, CornerRadius, FontFamily, FontId, Response, Sense, Ui, pos2, vec2};

use crate::theme::Tokens;

const FONT_SIZE: f32 = 10.0;
const HORIZONTAL_PADDING: f32 = 5.0;
const HEIGHT: f32 = 18.0;

/// Display-ready key sequence (for example `⌘V` or `Ctrl+V`).
pub struct Shortcut<'a> {
    text: &'a str,
    ink: Option<Color32>,
    background: Option<Color32>,
}

impl<'a> Shortcut<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text, ink: None, background: None }
    }

    pub fn ink(mut self, ink: Color32) -> Self {
        self.ink = Some(ink);
        self
    }

    /// Override the badge background when it sits on a surface with an inverted theme.
    pub fn background(mut self, background: Color32) -> Self {
        self.background = Some(background);
        self
    }

    fn font() -> FontId {
        FontId::new(FONT_SIZE, FontFamily::Name("shortcuts".into()))
    }

    fn display_text(&self) -> String {
        // The font's named keys are ligatures. egui does not shape GSUB, so the
        // bundled font exposes their outlines at these private-use codepoints.
        const NAMES: [(&str, &str); 22] = [
            ("Ctrl", "\u{E000}"),
            ("Shift", "\u{E001}"),
            ("Alt", "\u{E002}"),
            ("Enter", "\u{E003}"),
            ("Esc", "\u{E004}"),
            ("Tab", "\u{E005}"),
            ("Backspace", "\u{E006}"),
            ("Delete", "\u{E007}"),
            ("PageUp", "\u{E008}"),
            ("PageDown", "\u{E009}"),
            ("F10", "\u{E013}"),
            ("F11", "\u{E014}"),
            ("F12", "\u{E015}"),
            ("F1", "\u{E00A}"),
            ("F2", "\u{E00B}"),
            ("F3", "\u{E00C}"),
            ("F4", "\u{E00D}"),
            ("F5", "\u{E00E}"),
            ("F6", "\u{E00F}"),
            ("F7", "\u{E010}"),
            ("F8", "\u{E011}"),
            ("F9", "\u{E012}"),
        ];
        let mut text =
            self.text.replace('↵', "⏎").replace("Escape", "Esc").replace("Return", "Enter").replace("PgUp", "PageUp").replace("PgDn", "PageDown");
        for (name, glyph) in NAMES {
            text = text.replace(name, glyph);
        }
        text
    }

    pub fn width(&self, ui: &Ui) -> f32 {
        ui.painter().layout_no_wrap(self.display_text(), Self::font(), Color32::WHITE).size().x + HORIZONTAL_PADDING * 2.0
    }

    /// Paint centered vertically, with its right edge at `right`; return its left edge.
    pub fn paint_right(&self, ui: &Ui, right: f32, center_y: f32) -> f32 {
        let t = Tokens::get(ui.ctx());
        let ink = self.ink.unwrap_or(t.text_secondary).gamma_multiply(0.78);
        let galley = ui.painter().layout_no_wrap(self.display_text(), Self::font(), ink);
        let width = galley.size().x + HORIZONTAL_PADDING * 2.0;
        let rect = egui::Rect::from_center_size(pos2(right - width / 2.0, center_y), vec2(width, HEIGHT));
        let background = self.background.unwrap_or_else(|| if t.dark { Color32::from_white_alpha(26) } else { Color32::from_black_alpha(13) });
        ui.painter().rect_filled(rect, CornerRadius::same(4), background);
        ui.painter().galley(pos2(rect.left() + HORIZONTAL_PADDING, center_y - galley.size().y / 2.0), galley, ink);
        rect.left()
    }

    pub fn show(&self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(vec2(self.width(ui), HEIGHT), Sense::hover());
        self.paint_right(ui, rect.right(), rect.center().y);
        response
    }
}
