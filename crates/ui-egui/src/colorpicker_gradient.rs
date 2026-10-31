//! Compact gradient editor, laid out on the 240 × 329 reference dialog.

use super::custom::{BORDER, FIELD, MUTED, PANEL, WIDTH, at, field, label_tab, paint_kind_button, vector_button};
use super::*;

const BASE_HEIGHT: f32 = 329.0;
const SELECTED_ROW: Color32 = Color32::from_rgb(0x39, 0x43, 0x60);
const SELECTED_MARKER: Color32 = Color32::from_rgb(0x0c, 0x8c, 0xe9);
const MAX_STOPS: usize = 32;

fn entry(ui: &mut Ui, rect: Rect, id: Id, value: String) -> Option<String> {
    let focused = ui.memory(|m| m.has_focus(id));
    let mut text: String = if focused { ui.data(|d| d.get_temp(id)).unwrap_or(value) } else { value };
    let response = ui.put(
        rect,
        egui::TextEdit::singleline(&mut text)
            .id(id)
            .font(regular(11.0))
            .text_color(Color32::WHITE)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO),
    );
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    response.changed().then_some(text)
}

fn kind_label(kind: GradientKind) -> &'static str {
    match kind {
        GradientKind::Linear => "Linear",
        GradientKind::Radial => "Radial",
        GradientKind::Angular => "Angular",
        GradientKind::Diamond => "Diamond",
    }
}

fn switch_gradient_kind(gradient: &mut Gradient, kind: GradientKind) {
    if gradient.kind == kind {
        return;
    }
    let base = gradient.stops.first().map_or(Color::BLACK, |stop| stop.color);
    let defaults = Gradient::default_for(kind, base);
    gradient.kind = kind;
    gradient.from = defaults.from;
    gradient.to = defaults.to;
    gradient.width = defaults.width;
}

fn add_stop(gradient: &mut Gradient, selected: &mut usize, requested: Option<f32>) -> bool {
    if gradient.stops.len() >= MAX_STOPS {
        return false;
    }
    let position = requested
        .unwrap_or_else(|| {
            let sorted = gradient.sorted_stops();
            let mut best = (0.0, 0.0);
            for pair in sorted.windows(2) {
                if pair[1].position - pair[0].position > best.1 - best.0 {
                    best = (pair[0].position, pair[1].position);
                }
            }
            if best.0 == best.1 {
                best = (0.0, 1.0);
            }
            (best.0 + best.1) * 0.5
        })
        .clamp(0.0, 1.0);
    let color = gradient.sample(position);
    gradient.stops.push(ColorStop { position, color });
    gradient.stops.sort_by(|a, b| a.position.total_cmp(&b.position));
    *selected = gradient.stops.iter().position(|stop| stop.position == position && stop.color == color).unwrap_or(0);
    true
}

fn preview(ui: &mut Ui, origin: Pos2, gradient: &mut Gradient, selected: &mut usize, changed: &mut bool, done: &mut bool) {
    let bar = at(origin, 16.0, 144.0, 208.0, 32.0);
    // Rounded end columns keep the gradient inside the five-point canvas curve.
    let painter = ui.painter().with_clip_rect(bar);
    if gradient.stops.iter().any(|stop| stop.color.a < 0.999) {
        painter.rect_filled(bar, CornerRadius::same(5), Color32::WHITE);
        widgets::checker(&painter, bar.shrink2(vec2(5.0, 0.0)), 4.0);
    }
    for i in 0..208 {
        let x = bar.left() + i as f32;
        let edge = (i.min(207 - i) as f32).min(5.0);
        let inset = if edge < 5.0 { 5.0 - (25.0 - (5.0 - edge).powi(2)).sqrt() } else { 0.0 };
        let column = Rect::from_min_max(pos2(x, bar.top() + inset), pos2(x + 1.05, bar.bottom() - inset));
        painter.rect_filled(column, CornerRadius::ZERO, doc_c32(ui.ctx(), gradient.sample((i as f32 + 0.5) / 208.0)));
    }
    ui.painter().rect_stroke(bar, CornerRadius::same(5), Stroke::new(1.0, Color32::from_white_alpha(26)), StrokeKind::Inside);

    // Create new stops from the bar, while markers handle their own pointer interactions.
    let bar_response = ui.interact(bar, Id::new("cp_gradient_preview"), Sense::click());
    let stop_snapshot: Vec<_> = gradient.stops.iter().take(MAX_STOPS).copied().collect();
    for (index, stop) in stop_snapshot.iter().enumerate() {
        let marker = Rect::from_center_size(pos2(bar.left() + stop.position.clamp(0.0, 1.0) * bar.width(), origin.y + 140.0), vec2(24.0, 24.0));
        let response = ui
            .interact(marker, Id::new(("cp_gradient_marker", index)), Sense::click_and_drag())
            .on_hover_text(format!("{}%", (stop.position * 100.0).round()));
        ui.painter().rect_filled(marker, CornerRadius::same(5), if index == *selected { SELECTED_MARKER } else { FIELD });
        let swatch = Rect::from_center_size(marker.center(), vec2(14.0, 14.0));
        widgets::paint_swatch(ui.painter(), swatch, &Paint::solid(stop.color));
        if response.clicked() || response.drag_started() {
            *selected = index;
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(current) = gradient.stops.get_mut(index)
        {
            current.position = ((pointer.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
            *selected = index;
            *changed = true;
        }
        if response.drag_stopped() {
            *done = true;
        }
    }
    if let Some(stop) = stop_snapshot.get(*selected) {
        let marker = Rect::from_center_size(pos2(bar.left() + stop.position.clamp(0.0, 1.0) * bar.width(), origin.y + 140.0), vec2(24.0, 24.0));
        ui.painter().rect_filled(marker, CornerRadius::same(5), SELECTED_MARKER);
        widgets::paint_swatch(ui.painter(), Rect::from_center_size(marker.center(), vec2(14.0, 14.0)), &Paint::solid(stop.color));
    }
    if bar_response.clicked()
        && let Some(pointer) = bar_response.interact_pointer_pos()
        && !stop_snapshot.iter().any(|stop| (pointer.x - (bar.left() + stop.position * bar.width())).abs() <= 12.0 && pointer.y <= origin.y + 152.0)
    {
        *changed |= add_stop(gradient, selected, Some((pointer.x - bar.left()) / bar.width()));
        *done = true;
    }
}

fn stop_rows(ui: &mut Ui, origin: Pos2, gradient: &mut Gradient, selected: &mut usize, changed: &mut bool, done: &mut bool) {
    let snapshot: Vec<_> = gradient.stops.iter().take(MAX_STOPS).copied().collect();
    let mut remove = None;
    for (index, stop) in snapshot.iter().enumerate() {
        let y = 220.0 + index as f32 * 32.0;
        let row = at(origin, 0.0, y, WIDTH, 32.0);
        if index == *selected {
            ui.painter().rect_filled(row, CornerRadius::ZERO, SELECTED_ROW);
        }
        if ui.interact(row, Id::new(("cp_gradient_row", index)), Sense::click()).clicked() {
            *selected = index;
        }
        let position = at(origin, 16.0, y + 4.0, 48.0, 24.0);
        let color = at(origin, 72.0, y + 4.0, 128.0, 24.0);
        field(ui, position, CornerRadius::same(5), false);
        field(ui, color, CornerRadius::same(5), false);
        if let Some(text) = entry(
            ui,
            at(origin, 23.0, y + 8.0, 35.0, 16.0),
            Id::new(("cp_gradient_position", index)),
            format!("{}%", (stop.position * 100.0).round()),
        ) && let Ok(value) = text.trim().trim_end_matches('%').parse::<f32>()
            && value.is_finite()
            && let Some(current) = gradient.stops.get_mut(index)
        {
            current.position = (value / 100.0).clamp(0.0, 1.0);
            *selected = index;
            *changed = true;
            *done = true;
        }
        let swatch = at(origin, 77.0, y + 8.0, 16.0, 16.0);
        widgets::paint_swatch(ui.painter(), swatch, &Paint::solid(stop.color));
        if ui.interact(swatch, Id::new(("cp_gradient_swatch", index)), Sense::click()).on_hover_text("Edit stop color").clicked() {
            *selected = index;
            ui.data_mut(|data| data.insert_temp(Id::new("cp_gradient_stop_popup"), index));
        }
        if let Some(text) = entry(
            ui,
            at(origin, 97.0, y + 8.0, 55.0, 16.0),
            Id::new(("cp_gradient_hex", index)),
            stop.color.to_hex().trim_start_matches('#').to_uppercase(),
        ) && let Some(color) = Color::from_hex_or_name(text.trim())
            && let Some(current) = gradient.stops.get_mut(index)
        {
            current.color = color.with_alpha(current.color.a);
            *selected = index;
            *changed = true;
            *done = true;
        }
        if let Some(text) =
            entry(ui, at(origin, 157.0, y + 8.0, 22.0, 16.0), Id::new(("cp_gradient_alpha", index)), format!("{:.0}", stop.color.a * 100.0))
            && let Ok(value) = text.trim().trim_end_matches('%').parse::<f32>()
            && value.is_finite()
            && let Some(current) = gradient.stops.get_mut(index)
        {
            current.color = current.color.with_alpha((value / 100.0).clamp(0.0, 1.0));
            *selected = index;
            *changed = true;
            *done = true;
        }
        ui.painter().text(pos2(origin.x + 189.0, origin.y + y + 16.0), Align2::CENTER_CENTER, "%", regular(11.0), MUTED);
        let remove_rect = at(origin, 208.0, y + 4.0, 24.0, 24.0);
        let remove_response = ui.interact(remove_rect, Id::new(("cp_gradient_remove_button", index)), Sense::click()).tip("Remove stop");
        ui.painter().rect_filled(
            remove_rect,
            CornerRadius::same(5),
            if remove_response.hovered() { Color32::from_rgb(64, 64, 64) } else { Color32::TRANSPARENT },
        );
        icons::paint_colorpicker(ui.painter(), Rect::from_center_size(remove_rect.center(), vec2(12.0, 1.0)), 13, Color32::WHITE);
        if remove_response.clicked() && gradient.stops.len() > 2 {
            remove = Some(index);
        }
    }
    if let Some(index) = remove {
        gradient.stops.remove(index);
        *selected = (*selected).min(gradient.stops.len().saturating_sub(1));
        ui.data_mut(|data| data.remove::<usize>(Id::new("cp_gradient_stop_popup")));
        *changed = true;
        *done = true;
    }
}

pub(super) fn show(app: &mut LizardApp, ctx: &egui::Context, target: &PickerTarget, mut paint: Paint) {
    let PaintKind::Gradient(mut gradient) = paint.kind.clone() else { return };
    let pos = super::initial_position(app, ctx, target);
    let visible_stops = gradient.stops.len().min(MAX_STOPS);
    let height = BASE_HEIGHT + 32.0 * visible_stops.saturating_sub(3) as f32;
    let mut open = true;
    let mut close_now = false;
    let mut switched_mode = false;
    crate::primitives::FloatingPanel::new(Id::new("color_picker"), pos)
        .width(crate::primitives::FloatingPanelWidth::Compact)
        .height(crate::primitives::FloatingPanelHeight::Fixed(height))
        .fill(PANEL)
        .open(&mut open)
        .show_with_header(ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let dialog = Rect::from_min_size(header.min, vec2(WIDTH, height));
            ui.allocate_exact_size(vec2(WIDTH, height - 40.0), Sense::hover());
            let origin = dialog.min;
            let mut changed = false;
            let mut done = false;
            let mut selection: usize = ui.data(|d| d.get_temp(stop_id())).unwrap_or(0).min(visible_stops.saturating_sub(1));
            label_tab(ui, at(origin, 8.0, 8.0, 91.609, 24.0), "cp_custom_tab", "Custom", true);
            if label_tab(ui, at(origin, 103.609, 8.0, 74.914, 24.0), "cp_libraries_tab", "Libraries", false) {
                library_tab::activate(ctx);
            }
            if vector_button(ui, at(origin, 180.0, 8.0, 28.0, 24.0), "cp_new_style", 8, false, "New style").clicked() {
                library_tab::create(ctx, paint.clone());
                library_tab::activate(ctx);
            }
            if vector_button(ui, at(origin, 208.0, 8.0, 24.0, 24.0), "cp_close", 9, false, "Close").clicked() {
                close_now = true;
            }

            for index in 0..6 {
                let Some(response) = paint_kind_button(ui, origin, index, index == 1) else { continue };
                if index == 5 && response.clicked() {
                    if super::shader_tab::supports(target) {
                        super::shader_tab::activate(ctx);
                    } else {
                        app.toast("Shaders are available for fills");
                    }
                    continue;
                }
                if response.clicked() && index != 1 {
                    custom::pick_kind(app, target, &mut paint, index);
                    switched_mode = true;
                    return;
                }
            }
            let blend_rect = at(origin, 208.0, 48.0, 24.0, 24.0);
            let blend = ui.interact(blend_rect, Id::new("cp_gradient_blend_menu"), Sense::click()).tip("Blend mode");
            ui.painter().rect_filled(blend_rect, CornerRadius::same(5), if blend.hovered() { FIELD } else { Color32::TRANSPARENT });
            custom::paint_blend_icon(ui, blend_rect);
            crate::right::menu_popup(&blend).show(|ui| {
                if let Some(mode) = crate::right::blend_menu(ui, false) {
                    paint.blend = mode;
                    paint.kind = PaintKind::Gradient(gradient.clone());
                    write(app, target, &paint, false);
                }
            });
            ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 79.5, Stroke::new(1.0, BORDER));

            let type_field = at(origin, 16.0, 92.0, 96.0, 24.0);
            field(ui, type_field, CornerRadius::same(5), true);
            ui.painter().text(
                type_field.left_center() + vec2(7.0, 0.0),
                Align2::LEFT_CENTER,
                kind_label(gradient.kind),
                regular(11.0),
                Color32::WHITE,
            );
            icons::paint_colorpicker(ui.painter(), at(origin, 99.0, 102.5, 5.002, 3.002), 7, Color32::WHITE);
            let type_response = ui.interact(type_field, Id::new("cp_gradient_type"), Sense::click());
            crate::right::menu_popup(&type_response).show(|ui| {
                for kind in [GradientKind::Linear, GradientKind::Radial, GradientKind::Angular, GradientKind::Diamond] {
                    if crate::right::menu_item(ui, kind_label(kind), "") {
                        switch_gradient_kind(&mut gradient, kind);
                        changed = true;
                        done = true;
                        ui.close();
                    }
                }
            });
            if vector_button(ui, at(origin, 172.0, 92.0, 24.0, 24.0), "cp_gradient_flip", 11, false, "Flip gradient").clicked() {
                for stop in &mut gradient.stops {
                    stop.position = 1.0 - stop.position;
                }
                gradient.stops.reverse();
                selection = gradient.stops.len().saturating_sub(1).saturating_sub(selection);
                changed = true;
                done = true;
            }
            if vector_button(ui, at(origin, 200.0, 92.0, 24.0, 24.0), "cp_gradient_orientation", 12, false, "Rotate gradient 90°").clicked() {
                let (fx, fy) = gradient.from;
                let rotate = |(x, y): (f64, f64)| (fx - (y - fy), fy + (x - fx));
                gradient.to = rotate(gradient.to);
                gradient.width = rotate(gradient.width);
                changed = true;
                done = true;
            }

            preview(ui, origin, &mut gradient, &mut selection, &mut changed, &mut done);
            ui.painter().text(pos2(origin.x + 16.0, origin.y + 204.0), Align2::LEFT_CENTER, "Stops", semibold(11.0), Color32::WHITE);
            if vector_button(ui, at(origin, 208.0, 192.0, 24.0, 24.0), "cp_gradient_add", 8, false, "Add stop").clicked() {
                changed |= add_stop(&mut gradient, &mut selection, None);
                ui.data_mut(|data| data.remove::<usize>(Id::new("cp_gradient_stop_popup")));
                done = true;
            }
            stop_rows(ui, origin, &mut gradient, &mut selection, &mut changed, &mut done);
            ui.data_mut(|d| d.insert_temp(stop_id(), selection));
            if changed {
                paint.kind = PaintKind::Gradient(gradient);
                write(app, target, &paint, !done);
            }
            if done {
                finish(app);
            }
        });
    if switched_mode {
        ctx.data_mut(|data| data.remove::<usize>(Id::new("cp_gradient_stop_popup")));
        return;
    }
    let stop_popup = ctx.data(|data| data.get_temp::<usize>(Id::new("cp_gradient_stop_popup")));
    if !open || close_now || (stop_popup.is_none() && ctx.input(|input| input.key_pressed(egui::Key::Escape))) {
        finish(app);
        app.ui.picker = None;
        ctx.data_mut(|data| data.remove::<usize>(Id::new("cp_gradient_stop_popup")));
    } else if let Some(index) = stop_popup {
        let current = current_paint(app, target).unwrap_or(paint);
        if let PaintKind::Gradient(ref gradient) = current.kind
            && index < gradient.stops.len()
        {
            custom::show_stop(app, ctx, target, current, index, pos);
        } else {
            ctx.data_mut(|data| data.remove::<usize>(Id::new("cp_gradient_stop_popup")));
        }
    }
}
