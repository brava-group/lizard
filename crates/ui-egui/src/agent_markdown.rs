//! Render agent replies as Markdown without interpreting embedded HTML.

use egui::text::{ByteIndex, LayoutJob, TextFormat};
use egui::{Color32, FontId, Stroke, Ui};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme::{regular, semibold};

#[derive(Default)]
struct Style {
    strong: usize,
    emphasis: usize,
    strike: usize,
    link: usize,
    code_block: usize,
    heading: Option<HeadingLevel>,
}

fn format(style: &Style, color: Color32, accent: Color32, inline_code: bool) -> TextFormat {
    let size = match style.heading {
        Some(HeadingLevel::H1) => 18.0,
        Some(HeadingLevel::H2) => 16.0,
        Some(_) => 14.0,
        None => 14.0,
    };
    let code = inline_code || style.code_block > 0;
    TextFormat {
        font_id: if code {
            FontId::monospace(12.0)
        } else if style.strong > 0 || style.heading.is_some() {
            semibold(size)
        } else {
            regular(size)
        },
        color: if style.link > 0 { accent } else { color },
        background: if code && style.code_block == 0 {
            if u16::from(color.r()) + u16::from(color.g()) + u16::from(color.b()) < 384 {
                Color32::from_rgb(232, 232, 232)
            } else {
                Color32::from_rgb(55, 55, 55)
            }
        } else {
            Color32::TRANSPARENT
        },
        italics: style.emphasis > 0,
        underline: if style.link > 0 { Stroke::new(1.0, accent) } else { Stroke::NONE },
        strikethrough: if style.strike > 0 { Stroke::new(1.0, color) } else { Stroke::NONE },
        ..Default::default()
    }
}

fn append(job: &mut LayoutJob, value: &str, style: &Style, color: Color32, accent: Color32, code: bool) {
    if !value.is_empty() {
        job.append(value, 0.0, format(style, color, accent, code));
    }
}

fn newline(job: &mut LayoutJob, style: &Style, color: Color32, accent: Color32, blank: bool) {
    if !job.text.is_empty() && !job.text.ends_with('\n') {
        append(job, "\n", style, color, accent, false);
    }
    if blank && !job.text.is_empty() && !job.text.ends_with("\n\n") {
        append(job, "\n", style, color, accent, false);
    }
}

/// Parse the model's untrusted text as CommonMark into egui's native text layout.
fn layout_job(markdown: &str, color: Color32, accent: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut style = Style::default();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item_depth = 0_usize;
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Strong => style.strong += 1,
                Tag::Emphasis => style.emphasis += 1,
                Tag::Strikethrough => style.strike += 1,
                Tag::Link { .. } => style.link += 1,
                Tag::Heading { level, .. } => {
                    newline(&mut job, &style, color, accent, false);
                    style.heading = Some(level);
                }
                Tag::CodeBlock(_) => {
                    newline(&mut job, &style, color, accent, false);
                    style.code_block += 1;
                }
                Tag::List(start) => lists.push(start),
                Tag::Item => {
                    newline(&mut job, &style, color, accent, false);
                    item_depth += 1;
                    let indent = "  ".repeat(lists.len().saturating_sub(1).min(8));
                    let marker = match lists.last_mut() {
                        Some(Some(number)) => {
                            let marker = format!("{number}. ");
                            *number = number.saturating_add(1);
                            marker
                        }
                        _ => "• ".to_string(),
                    };
                    append(&mut job, &format!("{indent}{marker}"), &style, color, accent, false);
                }
                Tag::BlockQuote(_) => {
                    newline(&mut job, &style, color, accent, false);
                    append(&mut job, "│  ", &style, accent, accent, false);
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Strong => style.strong = style.strong.saturating_sub(1),
                TagEnd::Emphasis => style.emphasis = style.emphasis.saturating_sub(1),
                TagEnd::Strikethrough => style.strike = style.strike.saturating_sub(1),
                TagEnd::Link => style.link = style.link.saturating_sub(1),
                TagEnd::Heading(_) => {
                    style.heading = None;
                    newline(&mut job, &style, color, accent, true);
                }
                TagEnd::CodeBlock => {
                    style.code_block = style.code_block.saturating_sub(1);
                    newline(&mut job, &style, color, accent, true);
                }
                TagEnd::Paragraph => {
                    if item_depth == 0 {
                        newline(&mut job, &style, color, accent, true);
                    }
                }
                TagEnd::Item => {
                    item_depth = item_depth.saturating_sub(1);
                    newline(&mut job, &style, color, accent, false);
                }
                TagEnd::List(_) => {
                    lists.pop();
                    if lists.is_empty() {
                        newline(&mut job, &style, color, accent, true);
                    }
                }
                _ => {}
            },
            Event::Text(value) | Event::Html(value) | Event::InlineHtml(value) => {
                append(&mut job, &value, &style, color, accent, false);
            }
            Event::Code(value) => append(&mut job, &value, &style, color, accent, true),
            Event::SoftBreak => append(&mut job, " ", &style, color, accent, false),
            Event::HardBreak => append(&mut job, "\n", &style, color, accent, false),
            Event::Rule => {
                newline(&mut job, &style, color, accent, false);
                append(&mut job, "────────", &style, color, accent, false);
                newline(&mut job, &style, color, accent, true);
            }
            Event::TaskListMarker(done) => append(&mut job, if done { "☑ " } else { "☐ " }, &style, color, accent, false),
            _ => {}
        }
    }
    let end = job.text.trim_end_matches('\n').len();
    job.text.truncate(end);
    job.sections.retain(|section| section.byte_range.start < ByteIndex(end));
    if let Some(section) = job.sections.last_mut() {
        section.byte_range.end = section.byte_range.end.min(ByteIndex(end));
    }
    job
}

pub fn show(ui: &mut Ui, markdown: &str, color: Color32, accent: Color32) {
    ui.add(egui::Label::new(layout_job(markdown, color, accent)).wrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_agent_reply_without_markdown_delimiters() {
        let job = layout_job("É um **360 × 240 px** com `get_node` e *texto*.", Color32::WHITE, Color32::LIGHT_BLUE);
        assert!(job.text.contains("360 × 240 px"));
        assert!(job.text.contains("get_node"));
        assert!(!job.text.contains("**"));
        assert!(!job.text.contains('`'));
        assert!(!job.text.ends_with('\n'));
        assert!(job.sections.iter().any(|section| section.format.font_id == semibold(14.0)));
        assert!(job.sections.iter().any(|section| section.format.font_id == FontId::monospace(12.0)));
    }

    #[test]
    fn formats_lists_and_fenced_code_without_losing_text() {
        let job = layout_job("## Resultado\n\n1. Primeiro\n2. Segundo\n\n```json\n{\"ok\":true}\n```", Color32::WHITE, Color32::LIGHT_BLUE);
        assert!(job.text.contains("1. Primeiro"));
        assert!(job.text.contains("2. Segundo"));
        assert!(job.text.contains("{\"ok\":true}"));
        assert!(!job.text.contains("```"));
    }
}
