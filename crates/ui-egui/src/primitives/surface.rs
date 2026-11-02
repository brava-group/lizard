//! Lizard theme adapter for the reusable Workbench UI surface.

use crate::theme::Tokens;
use egui::{Frame, InnerResponse, Painter, Rect, Ui};
use workbench_ui::SurfaceTheme;

pub struct Surface(workbench_ui::Surface);

impl Surface {
    pub fn menu() -> Self {
        Self(workbench_ui::Surface::menu())
    }
    pub fn palette() -> Self {
        Self(workbench_ui::Surface::palette())
    }
    pub fn frame(&self, tokens: Tokens) -> Frame {
        self.0.frame(SurfaceTheme { dark: tokens.dark, panel: tokens.panel })
    }
    pub fn show<R>(&self, ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let tokens = Tokens::get(ui.ctx());
        self.0.show(ui, SurfaceTheme { dark: tokens.dark, panel: tokens.panel }, content)
    }
    pub fn paint_at(&self, painter: &Painter, rect: Rect, tokens: Tokens) {
        self.0.paint_at(painter, rect, SurfaceTheme { dark: tokens.dark, panel: tokens.panel });
    }
    pub fn paint_close_shadow_outside(&self, painter: &Painter, rect: Rect) {
        self.0.paint_close_shadow_outside(painter, rect);
    }
    pub fn paint_top_highlight(&self, painter: &Painter, rect: Rect) {
        self.0.paint_top_highlight(painter, rect);
    }
}
