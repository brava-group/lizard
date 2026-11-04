//! Reusable code editor with an integrated, scrolling line-number gutter.

use egui::{Align2, Color32, CornerRadius, FontId, Id, Layout, Response, Sense, Stroke, Ui, vec2};

use crate::theme::{self, Tokens};

/// Syntax rules used by a code block.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum CodeLanguage {
    #[default]
    Plain,
    Wgsl,
}

/// An editable or read-only IDE-style code surface. The caller owns the source text.
pub struct CodeBlock {
    id: Id,
    width: f32,
    height: f32,
    language: CodeLanguage,
    editable: bool,
}

impl CodeBlock {
    pub fn new(id: Id) -> Self {
        Self { id, width: 360.0, height: 280.0, language: CodeLanguage::Plain, editable: true }
    }

    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width.max(120.0);
        self.height = height.max(64.0);
        self
    }

    pub fn language(mut self, language: CodeLanguage) -> Self {
        self.language = language;
        self
    }

    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    pub fn show(self, ui: &mut Ui, source: &mut String) -> Response {
        let tokens = Tokens::get(ui.ctx());
        let background = if tokens.dark { Color32::from_rgb(35, 35, 35) } else { tokens.field };
        let frame = egui::Frame::new()
            .fill(background)
            .stroke(Stroke::new(1.0, tokens.border))
            .corner_radius(CornerRadius::same(7))
            .inner_margin(egui::Margin::same(8));
        ui.allocate_ui_with_layout(vec2(self.width, self.height), Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(self.width);
            frame
                .show(ui, |ui| {
                    let content_width = self.width - 18.0;
                    let content_height = self.height - 18.0;
                    ui.set_width(content_width);
                    egui::ScrollArea::vertical()
                        .id_salt(self.id.with("vertical"))
                        .auto_shrink([false, false])
                        .max_height(content_height)
                        .show(ui, |ui| {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                let gutter_width = 26.0;
                                let lines = source.lines().count() + usize::from(source.ends_with('\n'));
                                let editor_height = (lines.max(1) as f32 * 17.0 + 8.0).max(content_height);
                                let (gutter, _) = ui.allocate_exact_size(vec2(gutter_width, editor_height), Sense::hover());
                                let longest = source.lines().map(str::len).max().unwrap_or(0);
                                let viewport_width = content_width - gutter_width - 10.0;
                                let editor_width = (longest as f32 * 7.0 + 20.0).max(viewport_width);
                                let language = self.language;
                                let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
                                    ui.fonts_mut(|fonts| fonts.layout_job(code_layout_job(text.as_str(), language, tokens.dark)))
                                };
                                let output = egui::ScrollArea::horizontal()
                                    .id_salt(self.id.with("horizontal"))
                                    .auto_shrink([false, false])
                                    .max_width(viewport_width)
                                    .show(ui, |ui| {
                                        egui::TextEdit::multiline(source)
                                            .id(self.id.with("editor"))
                                            .code_editor()
                                            .frame(egui::Frame::NONE)
                                            .interactive(self.editable)
                                            .desired_width(editor_width)
                                            .min_size(vec2(editor_width, editor_height))
                                            .layouter(&mut layouter)
                                            .show(ui)
                                    })
                                    .inner;
                                let painter = ui.painter().with_clip_rect(gutter.intersect(ui.clip_rect()));
                                painter.vline(gutter.right() + 5.0, gutter.y_range(), Stroke::new(1.0, tokens.border));
                                for (index, row) in output.galley.rows.iter().enumerate() {
                                    let y = output.galley_pos.y + row.rect().center().y;
                                    painter.text(
                                        egui::pos2(gutter.right() - 2.0, y),
                                        Align2::RIGHT_CENTER,
                                        (index + 1).to_string(),
                                        FontId::monospace(theme::ui_font_size(11.0)),
                                        tokens.text_tertiary,
                                    );
                                }
                                output.response.response
                            })
                            .inner
                        })
                        .inner
                })
                .inner
        })
        .inner
    }
}

fn code_layout_job(source: &str, language: CodeLanguage, dark: bool) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};

    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let font = FontId::monospace(theme::ui_font_size(11.0));
    let format = |color| TextFormat { font_id: font.clone(), color, ..Default::default() };
    let normal_color = if dark { Color32::from_rgb(225, 225, 225) } else { Color32::from_rgb(38, 38, 42) };
    let normal = format(normal_color);
    if language == CodeLanguage::Plain {
        job.append(source, 0.0, normal);
        return job;
    }
    let mut cursor = 0;
    let mut plain = 0;
    while cursor < source.len() {
        let rest = &source[cursor..];
        let token = if rest.starts_with("//") {
            let end = rest.find('\n').unwrap_or(rest.len());
            Some((cursor + end, Color32::from_rgb(100, 155, 110)))
        } else if let Some(first) = rest.chars().next() {
            if first.is_ascii_digit() {
                let end = rest.bytes().take_while(|byte| byte.is_ascii_digit() || *byte == b'.').count();
                Some((cursor + end, Color32::from_rgb(215, 143, 83)))
            } else if first.is_ascii_alphabetic() || first == '_' || first == '@' {
                let end = rest.bytes().take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'@').count();
                let word = &rest[..end];
                let color = if word.starts_with('@') {
                    Color32::from_rgb(220, 163, 94)
                } else if matches!(word, "fn" | "let" | "var" | "return" | "for" | "if" | "else" | "struct" | "loop" | "break" | "continue") {
                    Color32::from_rgb(185, 143, 230)
                } else if matches!(word, "f32" | "i32" | "u32" | "bool" | "vec2f" | "vec3f" | "vec4f" | "mat2x2f" | "mat3x3f" | "mat4x4f") {
                    Color32::from_rgb(110, 175, 225)
                } else {
                    normal_color
                };
                Some((cursor + end, color))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((end, color)) = token {
            if plain < cursor {
                job.append(&source[plain..cursor], 0.0, normal.clone());
            }
            job.append(&source[cursor..end], 0.0, format(color));
            cursor = end;
            plain = end;
        } else if let Some(ch) = rest.chars().next() {
            cursor += ch.len_utf8();
        } else {
            break;
        }
    }
    if plain < source.len() {
        job.append(&source[plain..], 0.0, normal);
    }
    job
}
