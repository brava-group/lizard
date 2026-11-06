//! The swatch, value, and opacity control shared by paint rows and effect settings.

use crate::primitives::TooltipResponseExt as _;
use egui::{Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use lizard_color::{Color, Hsb, Hsl, Paint, PaintKind};

use super::ControlWidth;
use crate::i18n::tr;
use crate::icons;
use crate::theme::{Tokens, medium};
use crate::widgets::{self, FieldEdit};

/// What appears beside the swatch. The container decides how an edit changes its document.
pub enum ColorValue<'a> {
    EditableHex(&'a str),
    Label(&'a str),
    /// A saved paint style: the entire control opens the style browser.
    StyleName(&'a str),
    BoundVariable(&'a str),
    /// Compact fields for the colour picker's Hex, RGB, HSL, HSB and CSS modes.
    Model(ColorModel),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorModel {
    Hex,
    Rgb,
    Hsl,
    Hsb,
    Css,
}

/// Intent produced by the control. No document command is dispatched here.
#[derive(Default)]
pub struct ColorControlResponse {
    pub open_picker: bool,
    pub edited_hex: Option<String>,
    pub opacity_edit: Option<FieldEdit>,
    pub detach_variable: bool,
    /// A colour edit from a model field, plus whether a scrub has finished.
    pub edited_color: Option<(Color, bool)>,
}

/// A reusable, single-line colour control. Its label, visibility and removal actions belong to
/// the containing row; this primitive only owns the rounded swatch/value/opacity field.
pub struct ColorControl<'a> {
    id: Id,
    paint: &'a Paint,
    value: ColorValue<'a>,
    opacity_percent: f64,
    swatch_id: Option<Id>,
    inline: bool,
    opacity_editable: bool,
    label_opens_picker: bool,
}

impl<'a> ColorControl<'a> {
    pub fn new(id: Id, paint: &'a Paint, value: ColorValue<'a>, opacity_percent: f64) -> Self {
        Self { id, paint, value, opacity_percent, swatch_id: None, inline: false, opacity_editable: true, label_opens_picker: false }
    }

    /// Keep an existing swatch's egui identity when migrating a production row.
    pub fn swatch_id(mut self, id: Id) -> Self {
        self.swatch_id = Some(id);
        self
    }

    /// Display a colour as an unboxed swatch and value inside a table cell.
    pub fn inline(mut self) -> Self {
        self.inline = true;
        self
    }

    /// Show a read-only alpha value for colours that cannot store transparency.
    pub fn opacity_editable(mut self, editable: bool) -> Self {
        self.opacity_editable = editable;
        self
    }

    /// Let a read-only value open the same picker as its swatch.
    pub fn label_opens_picker(mut self) -> Self {
        self.label_opens_picker = true;
        self
    }

    /// Allocate the control in normal layout. Use [`Self::show_at`] inside an existing row.
    pub fn show(self, ui: &mut Ui, width: f32) -> ColorControlResponse {
        self.show_sized(ui, ControlWidth::Fixed(width.max(96.0)))
    }

    /// Allocate with the parent's available width, an explicit width, or content width.
    pub fn show_sized(self, ui: &mut Ui, width: ControlWidth) -> ColorControlResponse {
        let resolved = width.resolve(ui, self.intrinsic_width(ui));
        let (rect, _) = ui.allocate_exact_size(vec2(resolved, 24.0), Sense::hover());
        self.show_at(ui, rect)
    }

    fn intrinsic_width(&self, ui: &Ui) -> f32 {
        let (label, trailing) = match self.value {
            ColorValue::EditableHex(value) | ColorValue::Label(value) => (value, 52.0),
            ColorValue::StyleName(name) => (name, 12.0),
            // Reserve the unlink action even when it is hidden so hover does not resize the field.
            ColorValue::BoundVariable(name) => (name, 32.0),
            ColorValue::Model(_) => return 144.0,
        };
        let label_width = ui.painter().layout_no_wrap(label.to_owned(), medium(10.0), Color32::WHITE).size().x;
        (28.0 + label_width + trailing).max(96.0)
    }

    pub fn show_at(self, ui: &mut Ui, rect: Rect) -> ColorControlResponse {
        let t = Tokens::get(ui.ctx());
        if self.inline {
            let swatch = Rect::from_center_size(pos2(rect.min.x + 8.0, rect.center().y), vec2(16.0, 16.0));
            widgets::paint_swatch(ui.painter(), swatch, self.paint);
            let open_picker = ui
                .interact(swatch.expand(2.0), self.swatch_id.unwrap_or_else(|| self.id.with("swatch")), Sense::click())
                .lizard_tooltip(tr("Edit colour"))
                .clicked();
            let value_rect = Rect::from_min_max(pos2(swatch.right() + 4.0, rect.top()), rect.max);
            let edited_hex = match self.value {
                ColorValue::EditableHex(hex) => widgets::text_field_inline(ui, self.id.with("hex"), hex, value_rect, medium(10.0)),
                ColorValue::Label(label) | ColorValue::StyleName(label) | ColorValue::BoundVariable(label) => {
                    ui.painter().with_clip_rect(value_rect).text(value_rect.left_center(), Align2::LEFT_CENTER, label, medium(10.0), t.text);
                    None
                }
                ColorValue::Model(_) => None,
            };
            return ColorControlResponse { open_picker, edited_hex, ..Default::default() };
        }
        if let ColorValue::BoundVariable(name) = &self.value {
            return show_bound_variable(ui, rect, self.id, self.paint, name, t);
        }
        if let ColorValue::StyleName(name) = &self.value {
            return show_style_name(ui, rect, self.id, self.paint, name, t);
        }
        if let ColorValue::Model(model) = self.value {
            let color = self.paint.solid_color().unwrap_or(Color::BLACK).with_alpha((self.opacity_percent / 100.0).clamp(0.0, 1.0) as f32);
            return show_model(ui, rect, self.id, color, model, self.opacity_editable, t);
        }

        let hovered = ui.is_enabled() && ui.input(|i| i.pointer.hover_pos().is_some_and(|p| rect.contains(p)));
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);

        let swatch = Rect::from_min_size(pos2(rect.min.x + 4.0, rect.center().y - 7.0), vec2(14.0, 14.0));
        widgets::paint_swatch(ui.painter(), swatch, self.paint);
        let value_left = swatch.max.x + 5.0;
        let opacity_left = (rect.max.x - 52.0).max(value_left);
        let value_rect = Rect::from_min_max(pos2(value_left, rect.min.y), pos2(opacity_left, rect.max.y));
        let opacity_rect = Rect::from_min_max(pos2(opacity_left, rect.min.y), rect.max);
        ui.painter().line_segment(
            [pos2(opacity_rect.min.x, rect.min.y), pos2(opacity_rect.min.x, rect.max.y)],
            Stroke::new(1.0, if hovered { t.border } else { t.bg }),
        );

        let open_picker = ui
            .interact(swatch.expand(2.0), self.swatch_id.unwrap_or_else(|| self.id.with("swatch")), Sense::click())
            .lizard_tooltip(if matches!(&self.paint.kind, PaintKind::Shader(_)) { tr("Edit shader") } else { tr("Edit colour") })
            .clicked();
        let mut result = ColorControlResponse { open_picker, ..Default::default() };
        if matches!(&self.paint.kind, PaintKind::Shader(_)) {
            result.open_picker |= ui.interact(value_rect, self.id.with("shader_name"), Sense::click()).lizard_tooltip(tr("Edit shader")).clicked();
        }

        match self.value {
            ColorValue::EditableHex(hex) => {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(value_rect));
                result.edited_hex =
                    widgets::text_field_inline_without_ring(&mut child, self.id.with("hex"), hex, value_rect.shrink2(vec2(0.0, 4.0)), medium(10.0));
            }
            ColorValue::Label(label) => {
                ui.painter().with_clip_rect(value_rect).text(
                    pos2(value_rect.min.x, rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    medium(10.0),
                    t.text,
                );
                if self.label_opens_picker {
                    result.open_picker |= ui.interact(value_rect, self.id.with("label"), Sense::click()).lizard_tooltip(tr("Edit colour")).clicked();
                }
            }
            ColorValue::StyleName(_) | ColorValue::BoundVariable(_) => {}
            ColorValue::Model(_) => {}
        }

        // Keep the numeric editor and scrub handle separate. The percent sign stays pinned to
        // the trailing edge while the user edits the number, even in a narrow property row.
        let suffix_width = 18.0;
        let number_width = (opacity_rect.width() - suffix_width).max(0.0);
        let number_rect =
            Rect::from_min_size(pos2(opacity_rect.min.x + 1.0, opacity_rect.min.y), vec2((number_width - 1.0).max(0.0), opacity_rect.height()));
        let mut opacity_ui = ui.new_child(egui::UiBuilder::new().max_rect(number_rect));
        let opacity_id = self.id.with("opacity");
        result.opacity_edit = widgets::number_field_without_ring_with_padding_and_font(
            &mut opacity_ui,
            opacity_id,
            self.opacity_percent.round(),
            number_width,
            7.0,
            0.0,
            10.0,
        );
        let suffix = Rect::from_min_max(pos2(number_rect.max.x, opacity_rect.min.y), opacity_rect.max);
        let suffix_response = ui.interact(suffix, self.id.with("opacity_suffix"), Sense::CLICK | Sense::DRAG);
        if suffix_response.hovered() || suffix_response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if let Some(edit) = widgets::scrub(ui, self.id.with("opacity_suffix"), &suffix_response, Some(self.opacity_percent), 1.0) {
            result.opacity_edit = Some(edit);
        }
        let suffix_color = if t.dark { Color32::from_white_alpha(150) } else { Color32::from_black_alpha(130) };
        ui.painter().text(pos2(suffix.right() - 7.0, suffix.center().y), Align2::RIGHT_CENTER, "%", medium(9.5), suffix_color);

        let focused = ui
            .memory(|m| m.has_focus(self.id.with("hex").with("te")) || m.has_focus(opacity_id.with("te")) || m.has_focus(opacity_id.with("field")))
            || suffix_response.has_focus()
            || suffix_response.dragged();
        if focused {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Inside);
        } else if hovered {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.border), StrokeKind::Inside);
        }
        result
    }
}

fn show_style_name(ui: &mut Ui, rect: Rect, id: Id, paint: &Paint, name: &str, t: Tokens) -> ColorControlResponse {
    let response = ui.interact(rect, id.with("style"), Sense::click()).lizard_tooltip(name);
    ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
    let swatch = Rect::from_center_size(pos2(rect.left() + 12.0, rect.center().y), vec2(16.0, 16.0));
    if paint.solid_color().is_some() {
        ui.painter().circle_filled(swatch.center(), 8.0, crate::theme::doc_c32(ui.ctx(), paint.preview_color()));
    } else {
        widgets::paint_swatch(ui.painter(), swatch, paint);
    }
    let label_rect = Rect::from_min_max(pos2(swatch.right() + 7.0, rect.top()), pos2(rect.right() - 8.0, rect.bottom()));
    let label = widgets::elide_font(ui, name, label_rect.width().max(0.0), medium(10.0));
    ui.painter().with_clip_rect(label_rect).text(label_rect.left_center(), Align2::LEFT_CENTER, label, medium(10.0), t.text);
    if response.has_focus() {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Inside);
    } else if response.hovered() {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.field_hover_border), StrokeKind::Inside);
    }
    ColorControlResponse { open_picker: response.clicked(), ..Default::default() }
}

fn show_model(ui: &mut Ui, rect: Rect, id: Id, color: Color, model: ColorModel, opacity_editable: bool, t: Tokens) -> ColorControlResponse {
    const OPACITY_WIDTH: f32 = 52.0;
    let hovered = ui.is_enabled() && ui.input(|i| i.pointer.hover_pos().is_some_and(|p| rect.contains(p)));
    ui.painter().rect_filled(rect, CornerRadius::same(5), t.field);
    let value_right = (rect.right() - OPACITY_WIDTH).max(rect.left() + 24.0);
    let value_rect = Rect::from_min_max(rect.min, pos2(value_right, rect.bottom()));
    let mut result = ColorControlResponse::default();

    match model {
        ColorModel::Hex | ColorModel::Css => {
            let value = if model == ColorModel::Hex {
                color.to_hex()
            } else {
                let [r, g, b, _] = color.to_rgba8();
                let alpha = format!("{:.2}", color.a);
                format!("rgba({r}, {g}, {b}, {})", alpha.trim_end_matches('0').trim_end_matches('.'))
            };
            let edit_rect =
                Rect::from_min_max(pos2(value_rect.left() + 8.0, value_rect.top() + 4.0), pos2(value_rect.right() - 4.0, value_rect.bottom() - 4.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(value_rect));
            if let Some(text) = widgets::text_field_inline_without_ring(&mut child, id.with("model_value"), &value, edit_rect, medium(10.0)) {
                result.edited_color = if model == ColorModel::Hex {
                    Color::from_hex_or_name(&text).map(|next| (next.with_alpha(if next.a < 1.0 { next.a } else { color.a }), true))
                } else {
                    Color::parse_css(&text).map(|next| (next, true))
                };
            }
        }
        ColorModel::Rgb | ColorModel::Hsl | ColorModel::Hsb => {
            let mut values = match model {
                ColorModel::Rgb => {
                    let [r, g, b, _] = color.to_rgba8();
                    [f64::from(r), f64::from(g), f64::from(b)]
                }
                ColorModel::Hsl => {
                    let hsl = Hsl::from_color(color);
                    [f64::from(hsl.h), f64::from(hsl.s) * 100.0, f64::from(hsl.l) * 100.0]
                }
                ColorModel::Hsb => {
                    let hsb = Hsb::from_color(color);
                    [f64::from(hsb.h), f64::from(hsb.s) * 100.0, f64::from(hsb.b) * 100.0]
                }
                ColorModel::Hex | ColorModel::Css => [0.0; 3],
            };
            let segment_width = value_rect.width() / 3.0;
            for index in 0..3 {
                let segment = Rect::from_min_max(
                    pos2(value_rect.left() + index as f32 * segment_width, rect.top()),
                    pos2(value_rect.left() + (index + 1) as f32 * segment_width, rect.bottom()),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(segment));
                let channel_id = id.with(("channel", index));
                let (leading_padding, trailing_padding) = if index == 0 { (6.0, 4.0) } else { (4.0, 1.0) };
                if let Some(edit) = widgets::number_field_without_ring_with_padding_and_font(
                    &mut child,
                    channel_id,
                    values[index].round(),
                    segment_width,
                    leading_padding,
                    trailing_padding,
                    10.0,
                ) {
                    let (value, done) = match edit {
                        FieldEdit::Set(value) => (value, true),
                        FieldEdit::Scrub(value, done) => (value, done),
                    };
                    values[index] = if model == ColorModel::Rgb {
                        value.clamp(0.0, 255.0)
                    } else if index == 0 {
                        value.rem_euclid(360.0)
                    } else {
                        value.clamp(0.0, 100.0)
                    };
                    let next = match model {
                        ColorModel::Rgb => Color::new(values[0] as f32 / 255.0, values[1] as f32 / 255.0, values[2] as f32 / 255.0, color.a),
                        ColorModel::Hsl => {
                            Hsl { h: values[0] as f32, s: values[1] as f32 / 100.0, l: values[2] as f32 / 100.0 }.to_color().with_alpha(color.a)
                        }
                        ColorModel::Hsb => {
                            Hsb { h: values[0] as f32, s: values[1] as f32 / 100.0, b: values[2] as f32 / 100.0 }.to_color().with_alpha(color.a)
                        }
                        ColorModel::Hex | ColorModel::Css => color,
                    };
                    result.edited_color = Some((next, done));
                }
            }
            for index in 1..3 {
                let x = value_rect.left() + index as f32 * segment_width;
                let separator = if t.dark { t.panel } else { t.border };
                ui.painter().line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, separator));
            }
        }
    }

    let suffix_active = {
        let opacity_rect = Rect::from_min_max(pos2(value_right, rect.top()), rect.max);
        let number_width = (opacity_rect.width() - 18.0).max(0.0);
        let number_rect = Rect::from_min_size(pos2(opacity_rect.left() + 1.0, rect.top()), vec2((number_width - 1.0).max(0.0), rect.height()));
        let opacity_id = id.with("model_opacity");
        if opacity_editable {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(number_rect));
            result.opacity_edit = widgets::number_field_without_ring_with_padding_and_font(
                &mut child,
                opacity_id,
                f64::from(color.a) * 100.0,
                number_width,
                7.0,
                0.0,
                10.0,
            );
        } else {
            ui.painter().text(number_rect.center(), Align2::CENTER_CENTER, format!("{:.0}", color.a * 100.0), medium(10.0), t.text_secondary);
        }
        let suffix = Rect::from_min_max(pos2(number_rect.right(), rect.top()), rect.max);
        let suffix_response =
            ui.interact(suffix, id.with("model_percent"), if opacity_editable { Sense::CLICK | Sense::DRAG } else { Sense::hover() });
        let suffix_active = suffix_response.has_focus() || suffix_response.dragged();
        if suffix_response.hovered() || suffix_response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if opacity_editable && let Some(edit) = widgets::scrub(ui, id.with("model_percent"), &suffix_response, Some(f64::from(color.a) * 100.0), 1.0)
        {
            result.opacity_edit = Some(edit);
        }
        let suffix_color = if t.dark { Color32::from_white_alpha(150) } else { Color32::from_black_alpha(130) };
        ui.painter().text(pos2(suffix.right() - 7.0, suffix.center().y), Align2::RIGHT_CENTER, "%", medium(9.5), suffix_color);
        let separator =
            if matches!(model, ColorModel::Rgb | ColorModel::Hsl | ColorModel::Hsb) { if t.dark { t.panel } else { t.border } } else { t.bg };
        ui.painter().line_segment([pos2(value_right, rect.top()), pos2(value_right, rect.bottom())], Stroke::new(1.0, separator));
        suffix_active
    };

    let focused = ui.memory(|memory| {
        memory.has_focus(id.with("model_value").with("te"))
            || memory.has_focus(id.with("model_opacity").with("te"))
            || memory.has_focus(id.with("model_opacity").with("field"))
            || (0..3).any(|index| {
                let channel = id.with(("channel", index));
                memory.has_focus(channel.with("te")) || memory.has_focus(channel.with("field"))
            })
    });
    if focused || suffix_active {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.focus), StrokeKind::Inside);
    } else if hovered {
        ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.border), StrokeKind::Inside);
    }
    result
}

fn show_bound_variable(ui: &mut Ui, rect: Rect, id: Id, paint: &Paint, name: &str, t: Tokens) -> ColorControlResponse {
    let hovered = ui.is_enabled() && ui.input(|i| i.pointer.hover_pos().is_some_and(|p| rect.contains(p)));
    let show_detach = hovered || ui.memory(|m| m.has_focus(id.with("bound")) || m.has_focus(id.with("detach")));
    let button_width = if show_detach { (rect.width() - 28.0).max(24.0) } else { rect.width() };
    let button = Rect::from_min_size(rect.min, vec2(button_width, rect.height()));
    ui.painter().rect_stroke(button, CornerRadius::same(5), Stroke::new(1.0, t.field), StrokeKind::Inside);
    let swatch = Rect::from_min_size(pos2(button.min.x + 5.0, button.center().y - 7.0), vec2(14.0, 14.0));
    widgets::paint_swatch(ui.painter(), swatch, paint);
    let label_rect = Rect::from_min_max(pos2(swatch.max.x + 5.0, button.min.y), pos2(button.max.x - 4.0, button.max.y));
    let label = widgets::elide_font(ui, name, label_rect.width().max(0.0), medium(10.0));
    ui.painter().with_clip_rect(label_rect).text(pos2(label_rect.min.x, label_rect.center().y), Align2::LEFT_CENTER, label, medium(10.0), t.text);
    let open_picker = ui.interact(button, id.with("bound"), Sense::click()).lizard_tooltip(name).clicked();
    let mut response = ColorControlResponse { open_picker, ..Default::default() };
    if show_detach && rect.width() >= 80.0 {
        let detach = Rect::from_min_size(pos2(button.max.x + 4.0, rect.min.y), vec2(24.0, 24.0));
        let detach_response = ui.interact(detach, id.with("detach"), Sense::click()).lizard_tooltip(tr("Detach variable"));
        if detach_response.hovered() {
            ui.painter().rect_filled(detach, CornerRadius::same(5), t.hover);
        }
        icons::paint_color_detach(ui.painter(), Rect::from_center_size(detach.center(), vec2(12.0, 12.0)), t.text);
        response.detach_variable = detach_response.clicked();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use lizard_color::Color;

    #[test]
    fn variants_render_even_when_the_parent_row_is_narrow() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx, true, 0);
        theme::apply(&ctx, Tokens::dark());
        let paint = Paint::solid(Color::new(0.047, 0.549, 0.914, 1.0));
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            for (index, width) in [64.0, 96.0, 208.0].into_iter().enumerate() {
                let rect = Rect::from_min_size(pos2(0.0, index as f32 * 32.0), vec2(width, 24.0));
                ColorControl::new(Id::new(("hex", index)), &paint, ColorValue::EditableHex("0C8CE9"), 100.0).show_at(ui, rect);
                ColorControl::new(Id::new(("bound", index)), &paint, ColorValue::BoundVariable("Theme/brand"), 100.0)
                    .show_at(ui, rect.translate(vec2(0.0, 100.0)));
                ColorControl::new(Id::new(("style", index)), &paint, ColorValue::StyleName("Brand/Primary"), 100.0)
                    .show_at(ui, rect.translate(vec2(0.0, 200.0)));
            }
        });
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }

    #[test]
    fn focus_on_either_editor_outlines_the_whole_control() {
        let paint = Paint::solid(Color::new(0.047, 0.549, 0.914, 1.0));
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(208.0, 24.0));
        for focus in ["hex", "opacity"] {
            let ctx = egui::Context::default();
            theme::install_fonts(&ctx, true, 0);
            theme::apply(&ctx, Tokens::dark());
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))), ..Default::default() };
            let id = Id::new("color_focus_test");
            let mut output = ctx.run_ui(input, |ui| {
                ui.memory_mut(|m| m.request_focus(id.with(focus).with("te")));
                ColorControl::new(id, &paint, ColorValue::EditableHex("0C8CE9"), 100.0).show_at(ui, rect);
            });
            assert!(output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Rect(shape) if shape.rect == rect && shape.stroke.color == Tokens::dark().focus)
            }));
            output.textures_delta.clear();
        }
    }
}
