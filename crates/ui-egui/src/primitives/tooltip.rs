//! Shared tooltip content and surface for controls in the editor and Workbench UI.

use egui::{Frame, InnerResponse, Margin, Response, RichText, Stroke, Ui};

use crate::theme::{Tokens, regular, semibold};

pub struct Tooltip<'a> {
    title: &'a str,
    shortcut: Option<&'a str>,
    description: Option<&'a str>,
}

impl<'a> Tooltip<'a> {
    pub fn new(title: &'a str) -> Self {
        Self { title, shortcut: None, description: None }
    }

    pub fn shortcut(mut self, shortcut: &'a str) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    pub fn description(mut self, description: &'a str) -> Self {
        self.description = Some(description);
        self
    }

    fn frame(t: Tokens) -> Frame {
        Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).corner_radius(8).inner_margin(Margin::symmetric(10, 7)).shadow(egui::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: t.shadow,
        })
    }

    fn content(&self, ui: &mut Ui) {
        let t = Tokens::get(ui.ctx());
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.title).font(semibold(11.0)).color(t.text));
            if let Some(shortcut) = self.shortcut {
                ui.add_space(8.0);
                ui.label(RichText::new(shortcut).font(regular(10.0)).color(t.text_secondary));
            }
        });
        if let Some(description) = self.description {
            ui.label(RichText::new(description).font(regular(10.0)).color(t.text_secondary));
        }
    }

    pub fn show_for(self, response: &Response) {
        let frame = Self::frame(Tokens::get(&response.ctx));
        egui::Tooltip::for_enabled(response).popup.frame(frame).show(|ui| self.content(ui));
    }

    /// An always visible specimen for tuning the same content and surface in Workbench UI.
    pub fn preview(self, ui: &mut Ui) -> InnerResponse<()> {
        let frame = Self::frame(Tokens::get(ui.ctx()));
        frame.show(ui, |ui| self.content(ui))
    }
}
