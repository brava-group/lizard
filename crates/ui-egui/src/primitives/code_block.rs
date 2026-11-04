//! Reusable code editor with an integrated, scrolling line-number gutter.

use egui::{Align2, Color32, CornerRadius, FontId, Id, Layout, Response, Sense, Stroke, Ui, vec2};

use crate::theme::{self, Tokens};

/// Syntax rules used by a code block.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum CodeLanguage {
    #[default]
    Plain,
    Wgsl,
    Css,
    Html,
    React,
    Leptos,
    SwiftUi,
    Compose,
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
    let normal_color = if dark { Color32::from_rgb(225, 225, 225) } else { Color32::from_rgb(38, 38, 42) };
    let format = |color| TextFormat { font_id: font.clone(), color, ..Default::default() };
    let mut previous = 0;
    for token in syntax_tokens(source, language) {
        if previous < token.start {
            job.append(&source[previous..token.start], 0.0, format(normal_color));
        }
        job.append(&source[token.start..token.end], 0.0, format(syntax_color(token.kind, dark)));
        previous = token.end;
    }
    if previous < source.len() || source.is_empty() {
        job.append(&source[previous..], 0.0, format(normal_color));
    }
    job
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyntaxKind {
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Tag,
    Attribute,
    Property,
    Value,
    Selector,
}

#[derive(Clone, Copy)]
struct SyntaxToken {
    start: usize,
    end: usize,
    kind: SyntaxKind,
}

fn syntax_color(kind: SyntaxKind, dark: bool) -> Color32 {
    let (dark_rgb, light_rgb) = match kind {
        SyntaxKind::Comment => ((116, 157, 119), (83, 127, 87)),
        SyntaxKind::String => ((221, 174, 126), (151, 92, 43)),
        SyntaxKind::Number => ((220, 151, 93), (154, 93, 43)),
        SyntaxKind::Keyword => ((190, 149, 232), (113, 67, 171)),
        SyntaxKind::Type => ((123, 181, 227), (35, 104, 165)),
        SyntaxKind::Tag => ((123, 194, 214), (28, 110, 150)),
        SyntaxKind::Attribute => ((220, 175, 111), (152, 101, 37)),
        SyntaxKind::Property => ((159, 195, 219), (41, 113, 153)),
        SyntaxKind::Value => ((218, 130, 185), (159, 64, 120)),
        SyntaxKind::Selector => ((184, 181, 227), (84, 78, 151)),
    };
    let (r, g, b) = if dark { dark_rgb } else { light_rgb };
    Color32::from_rgb(r, g, b)
}

fn syntax_tokens(source: &str, language: CodeLanguage) -> Vec<SyntaxToken> {
    if language == CodeLanguage::Plain {
        return Vec::new();
    }
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut css = language == CodeLanguage::Css;
    let mut css_rule = false;
    let mut css_value = false;
    while index < bytes.len() {
        let rest = &source[index..];
        if css && language == CodeLanguage::Html && rest.starts_with("</style") {
            css = false;
        }
        if rest.starts_with("<!--") {
            let end = index + rest.find("-->").map_or(rest.len(), |offset| offset + 3);
            tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::Comment });
            index = end;
            continue;
        }
        if rest.starts_with("/*") {
            let end = index + rest.find("*/").map_or(rest.len(), |offset| offset + 2);
            tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::Comment });
            if matches!(language, CodeLanguage::React | CodeLanguage::Leptos) && source[index..end].contains(".css") {
                css = true;
            }
            index = end;
            continue;
        }
        if !css && rest.starts_with("//") {
            let end = index + rest.find('\n').unwrap_or(rest.len());
            tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::Comment });
            index = end;
            continue;
        }
        if matches!(language, CodeLanguage::Html | CodeLanguage::React | CodeLanguage::Leptos)
            && bytes[index] == b'<'
            && is_markup_start(bytes, index)
        {
            let (end, style, closing) = markup_tag(source, index, &mut tokens);
            if language == CodeLanguage::Html && style {
                css = !closing;
            }
            index = end;
            continue;
        }
        if bytes[index] == b'"' || bytes[index] == b'\'' || bytes[index] == b'`' {
            let end = quoted_end(bytes, index);
            tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::String });
            index = end;
            continue;
        }
        if !css && bytes[index] == b'#' && bytes.get(index + 1) == Some(&b'[') {
            let end = index + rest.find(']').map_or(rest.len(), |offset| offset + 1);
            tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::Attribute });
            index = end;
            continue;
        }
        if bytes[index] == b'@' && bytes.get(index + 1).is_some_and(u8::is_ascii_alphabetic) {
            let end = identifier_end(bytes, index + 1);
            tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::Attribute });
            index = end;
            continue;
        }
        if bytes[index].is_ascii_digit() {
            let mut end = index + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'.' | b'_')) {
                end += 1;
            }
            tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::Number });
            index = end;
            continue;
        }
        if is_identifier_start(bytes[index]) {
            let end = identifier_end(bytes, index);
            let word = &source[index..end];
            let kind = if css {
                if css_rule && !css_value && source[end..].trim_start().starts_with(':') {
                    SyntaxKind::Property
                } else if css_value {
                    SyntaxKind::Value
                } else {
                    SyntaxKind::Selector
                }
            } else if is_keyword(language, word) || word == "true" || word == "false" || word == "null" || word == "nil" {
                SyntaxKind::Keyword
            } else if word.as_bytes().first().is_some_and(u8::is_ascii_uppercase) || is_builtin_type(word) {
                SyntaxKind::Type
            } else if bytes.get(end) == Some(&b'!') && language == CodeLanguage::Leptos {
                SyntaxKind::Keyword
            } else {
                index = end;
                continue;
            };
            tokens.push(SyntaxToken { start: index, end, kind });
            index = end;
            continue;
        }
        if css {
            match bytes[index] {
                b'{' => {
                    css_rule = true;
                    css_value = false;
                }
                b'}' => {
                    css_rule = false;
                    css_value = false;
                }
                b':' if css_rule => css_value = true,
                b';' => css_value = false,
                _ => {}
            }
        }
        index += source[index..].chars().next().map_or(1, char::len_utf8);
    }
    tokens
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn identifier_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    end
}

fn quoted_end(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut end = start + 1;
    while end < bytes.len() {
        if bytes[end] == b'\\' {
            end = (end + 2).min(bytes.len());
        } else if bytes[end] == quote {
            return end + 1;
        } else {
            end += 1;
        }
    }
    end
}

fn is_markup_start(bytes: &[u8], start: usize) -> bool {
    let mut next = start + 1;
    if bytes.get(next) == Some(&b'/') {
        next += 1;
    }
    bytes.get(next).is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'!')
}

fn markup_tag(source: &str, start: usize, tokens: &mut Vec<SyntaxToken>) -> (usize, bool, bool) {
    let bytes = source.as_bytes();
    let closing = bytes.get(start + 1) == Some(&b'/');
    let name_start = start + if closing { 2 } else { 1 };
    let mut index = name_start;
    while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'-' | b':' | b'_')) {
        index += 1;
    }
    let style = source[name_start..index].eq_ignore_ascii_case("style");
    tokens.push(SyntaxToken { start, end: index, kind: SyntaxKind::Tag });
    while index < bytes.len() {
        if bytes[index] == b'>' {
            tokens.push(SyntaxToken { start: index, end: index + 1, kind: SyntaxKind::Tag });
            return (index + 1, style, closing);
        }
        if bytes[index] == b'"' || bytes[index] == b'\'' {
            let end = quoted_end(bytes, index);
            tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::String });
            index = end;
            continue;
        }
        if is_identifier_start(bytes[index]) {
            let end = identifier_end(bytes, index);
            if source[end..].trim_start().starts_with('=') {
                tokens.push(SyntaxToken { start: index, end, kind: SyntaxKind::Attribute });
            }
            index = end;
        } else {
            index += source[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    (index, style, closing)
}

fn is_builtin_type(word: &str) -> bool {
    matches!(word, "f32" | "f16" | "i32" | "u32" | "bool" | "str" | "usize" | "vec2f" | "vec3f" | "vec4f" | "mat2x2f" | "mat3x3f" | "mat4x4f")
}

fn is_keyword(language: CodeLanguage, word: &str) -> bool {
    match language {
        CodeLanguage::Wgsl => {
            matches!(word, "fn" | "let" | "var" | "return" | "for" | "if" | "else" | "struct" | "loop" | "break" | "continue" | "const" | "override")
        }
        CodeLanguage::React => matches!(
            word,
            "import"
                | "from"
                | "export"
                | "default"
                | "function"
                | "const"
                | "let"
                | "return"
                | "interface"
                | "type"
                | "extends"
                | "if"
                | "else"
                | "new"
                | "as"
        ),
        CodeLanguage::Leptos => matches!(
            word,
            "use" | "pub" | "fn" | "let" | "mut" | "move" | "impl" | "struct" | "enum" | "if" | "else" | "match" | "Some" | "None" | "return"
        ),
        CodeLanguage::SwiftUi => matches!(
            word,
            "import" | "struct" | "var" | "let" | "some" | "func" | "private" | "public" | "if" | "else" | "return" | "enum" | "case" | "in"
        ),
        CodeLanguage::Compose => matches!(
            word,
            "package"
                | "import"
                | "fun"
                | "val"
                | "var"
                | "private"
                | "public"
                | "if"
                | "else"
                | "return"
                | "object"
                | "data"
                | "class"
                | "enum"
                | "when"
                | "is"
        ),
        CodeLanguage::Plain | CodeLanguage::Css | CodeLanguage::Html => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{CodeLanguage, SyntaxKind, SyntaxToken, syntax_tokens};

    fn kind_of(source: &str, tokens: &[SyntaxToken], fragment: &str) -> Option<SyntaxKind> {
        let at = source.find(fragment)?;
        tokens.iter().find(|token| token.start <= at && at < token.end).map(|token| token.kind)
    }

    #[test]
    fn every_inspect_language_colors_its_native_syntax() {
        let cases = [
            (CodeLanguage::Css, ".card { width: 12px; color: red; }", "width", SyntaxKind::Property, "red", SyntaxKind::Value),
            (CodeLanguage::Html, "<div class=\"card\"></div>", "<div", SyntaxKind::Tag, "class", SyntaxKind::Attribute),
            (
                CodeLanguage::React,
                "export function Card() { return <div className=\"card\" />; }",
                "export",
                SyntaxKind::Keyword,
                "<div",
                SyntaxKind::Tag,
            ),
            (
                CodeLanguage::Leptos,
                "#[component]\npub fn Card() -> impl IntoView { view! { <div /> } }",
                "#[component]",
                SyntaxKind::Attribute,
                "pub",
                SyntaxKind::Keyword,
            ),
            (
                CodeLanguage::SwiftUi,
                "import SwiftUI\nstruct Card: View { var body: some View { Text(\"Hi\") } }",
                "import",
                SyntaxKind::Keyword,
                "SwiftUI",
                SyntaxKind::Type,
            ),
            (
                CodeLanguage::Compose,
                "@Composable\nfun Card() { Text(\"Hi\", color = Color.Red) }",
                "@Composable",
                SyntaxKind::Attribute,
                "fun",
                SyntaxKind::Keyword,
            ),
        ];
        for (language, source, first, first_kind, second, second_kind) in cases {
            let tokens = syntax_tokens(source, language);
            assert_eq!(kind_of(source, &tokens, first), Some(first_kind), "{first} in {source}");
            assert_eq!(kind_of(source, &tokens, second), Some(second_kind), "{second} in {source}");
        }
    }

    #[test]
    fn embedded_css_and_unicode_strings_keep_valid_non_overlapping_ranges() {
        for (language, source) in [
            (CodeLanguage::Html, "<p title=\"Olá\">Oi</p><style>.card { color: red; }</style>"),
            (CodeLanguage::React, "export const Card = () => <div>Olá</div>;\n/* card.css */\n.card { color: red; }"),
            (CodeLanguage::Leptos, "pub fn card() { view! { <div>Olá</div> } }\n/* card.css */\n.card { color: red; }"),
        ] {
            let tokens = syntax_tokens(source, language);
            assert_eq!(kind_of(source, &tokens, "color",), Some(SyntaxKind::Property));
            assert_eq!(kind_of(source, &tokens, "red"), Some(SyntaxKind::Value));
            let mut end = 0;
            for token in tokens {
                assert!(token.start >= end && token.end <= source.len());
                assert!(source.is_char_boundary(token.start) && source.is_char_boundary(token.end));
                end = token.end;
            }
        }
    }
}
