//! Compact image editor, laid out on the 240 × 577 reference dialog.

use super::custom::{BORDER, FIELD, MUTED, PANEL, WIDTH, at, field, label_tab, paint_kind_button, vector_button};
use super::*;

const HEIGHT: f32 = 577.0;
const BLUE: Color32 = Color32::from_rgb(0x0c, 0x8c, 0xe9);
const SLIDER_BLUE: Color32 = Color32::from_rgb(0x12, 0x81, 0xe1);
const SLIDER_TRACK: Color32 = Color32::from_rgb(0x31, 0x31, 0x31);
const SLIDER_BORDER: Color32 = Color32::from_rgb(0x5a, 0x5a, 0x5a);
const FILTER_NAMES: [&str; 7] = ["Exposure", "Contrast", "Saturation", "Temperature", "Tint", "Highlights", "Shadows"];
const FILTER_IDS: [&str; 7] = [
    "cp_image_exposure",
    "cp_image_contrast",
    "cp_image_saturation",
    "cp_image_temperature",
    "cp_image_tint",
    "cp_image_highlights",
    "cp_image_shadows",
];

fn preview_texture(app: &FigApp, ctx: &egui::Context, image: &figcraft_color::ImagePaint) -> Option<egui::TextureHandle> {
    if image.image.is_empty() {
        return None;
    }
    let doc = app.session.doc().ok()?;
    let profile = doc.color_profile;
    let cache = Id::new("cp_image_preview_texture");
    if let Some((hash, filters, cached_profile, texture)) =
        ctx.data(|data| data.get_temp::<(String, ImageFilters, figcraft_color::ColorProfile, egui::TextureHandle)>(cache))
        && hash == image.image
        && filters == image.filters
        && cached_profile == profile
    {
        return Some(texture);
    }
    let pixels = figcraft_render::images::get(&image.image, doc.images.get(&image.image), &image.filters, profile)?;
    let color_image = crate::render_worker::screen_image(&pixels, profile);
    let texture = ctx.load_texture(format!("cp-image-{}", image.image), color_image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(cache, (image.image.clone(), image.filters, profile, texture.clone())));
    Some(texture)
}

fn upload(app: &mut FigApp, target: &PickerTarget) {
    let target = target.clone();
    app.pick_open(tr("Choose image"), &["png", "jpg", "jpeg", "webp", "gif", "tif", "tiff"], move |app, path| {
        let Ok(bytes) = std::fs::read(&path) else { return };
        let Some(hash) = app.session.doc_mut().ok().and_then(|doc| figcraft_format_add(doc, bytes)) else { return };
        if let Some(mut paint) = current_paint(app, &target)
            && let PaintKind::Image(ref mut image) = paint.kind
        {
            image.image = hash;
            write(app, &target, &paint, false);
        }
    });
}

fn filter_value(filters: &ImageFilters, index: usize) -> f32 {
    match index {
        0 => filters.exposure,
        1 => filters.contrast,
        2 => filters.saturation,
        3 => filters.temperature,
        4 => filters.tint,
        5 => filters.highlights,
        _ => filters.shadows,
    }
}

fn set_filter(filters: &mut ImageFilters, index: usize, value: f32) {
    match index {
        0 => filters.exposure = value,
        1 => filters.contrast = value,
        2 => filters.saturation = value,
        3 => filters.temperature = value,
        4 => filters.tint = value,
        5 => filters.highlights = value,
        _ => filters.shadows = value,
    }
}

fn filter_slider(ui: &mut Ui, origin: Pos2, index: usize, value: f32) -> (Option<f32>, bool) {
    let row_y = 344.0 + index as f32 * 32.0;
    ui.painter().text(pos2(origin.x + 16.0, origin.y + row_y + 16.0), Align2::LEFT_CENTER, FILTER_NAMES[index], regular(11.0), MUTED);
    let track = at(origin, 104.0, row_y + 8.0, 120.0, 16.0);
    let response = ui.interact(track, Id::new(FILTER_IDS[index]), Sense::click_and_drag()).tip(FILTER_NAMES[index]);
    let changed = if response.double_clicked() {
        Some(0.0)
    } else if response.clicked() || response.dragged() {
        ui.input(|input| input.pointer.interact_pos()).map(|pointer| ((pointer.x - track.left() - 8.0) / 104.0 * 2.0 - 1.0).clamp(-1.0, 1.0))
    } else {
        None
    };
    ui.painter().rect_filled(track, CornerRadius::same(8), SLIDER_TRACK);
    ui.painter().rect_stroke(track, CornerRadius::same(8), Stroke::new(1.0, SLIDER_BORDER), StrokeKind::Inside);
    let current = changed.unwrap_or(value);
    let center_x = track.center().x;
    let knob_x = center_x + current.clamp(-1.0, 1.0) * 52.0;
    if (knob_x - center_x).abs() > 0.5 {
        let (left, right) = if knob_x < center_x { (knob_x, center_x + 8.0) } else { (center_x - 8.0, knob_x) };
        let fill = Rect::from_min_max(pos2(left, track.top()), pos2(right, track.bottom()));
        ui.painter().rect_filled(fill, CornerRadius::same(8), SLIDER_BLUE);
        ui.painter().circle_filled(pos2(center_x, track.center().y), 2.0, Color32::WHITE);
    }
    let knob = pos2(knob_x, track.center().y);
    ui.painter().circle_filled(knob + vec2(0.0, 0.5), 8.6, Color32::from_black_alpha(130));
    ui.painter().circle_filled(knob, 8.0, Color32::WHITE);
    if (knob_x - center_x).abs() > 0.5 {
        ui.painter().circle_filled(knob, 4.0, SLIDER_BLUE);
    }
    (changed.filter(|next| (next - value).abs() > f32::EPSILON), response.drag_stopped() || response.double_clicked())
}

fn preview(ui: &mut Ui, app: &mut FigApp, ctx: &egui::Context, target: &PickerTarget, image: &figcraft_color::ImagePaint, origin: Pos2) {
    let rect = at(origin, 16.0, 128.0, 208.0, 208.0);
    ui.painter().rect_filled(rect, CornerRadius::same(5), Color32::from_gray(0xe6));
    widgets::checker(&ui.painter().with_clip_rect(rect), rect, 8.0);
    if let Some(texture) = preview_texture(app, ctx, image) {
        let size = texture.size_vec2();
        let fit = (208.0 / size.x.max(1.0)).min(208.0 / size.y.max(1.0));
        let display = Rect::from_center_size(rect.center(), size * fit);
        ui.painter().with_clip_rect(rect).image(texture.id(), display, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    ui.painter().rect_filled(rect, CornerRadius::same(5), Color32::from_black_alpha(128));
    mask_round_corners(ui, rect, 5.0);

    let upload_rect = at(origin, 36.0, 204.0, 168.0, 24.0);
    ui.painter().rect_filled(upload_rect, CornerRadius::same(5), BLUE);
    ui.painter().text(upload_rect.center(), Align2::CENTER_CENTER, "Upload from computer", regular(11.0), Color32::WHITE);
    if ui.interact(upload_rect, Id::new("cp_image_upload"), Sense::click()).tip("Upload from computer").clicked() {
        upload(app, target);
    }

    let create_rect = at(origin, 36.0, 236.0, 168.0, 24.0);
    ui.painter().rect_filled(create_rect, CornerRadius::same(5), PANEL);
    icons::paint_colorpicker(ui.painter(), at(origin, 62.0, 241.0, 13.939, 13.939), 16, Color32::WHITE);
    ui.painter().text(pos2(origin.x + 83.0, origin.y + 248.0), Align2::LEFT_CENTER, "Create an image", regular(11.0), Color32::WHITE);
    if ui.interact(create_rect, Id::new("cp_image_create"), Sense::click()).tip("Create an image").clicked() {
        app.toast("Image generation is not available yet");
    }
}

pub(super) fn show(app: &mut FigApp, ctx: &egui::Context, target: &PickerTarget, mut paint: Paint) {
    let PaintKind::Image(mut image) = paint.kind.clone() else { return };
    let screen = ctx.content_rect();
    let beside = match target {
        PickerTarget::PluginField { plugin, .. } => {
            crate::plugins_ui::field_window(app, ctx, plugin).map(|rect| pos2(rect.min.x - 248.0, rect.min.y))
        }
        _ => None,
    };
    let pos = beside.unwrap_or(pos2(screen.max.x - app.ui.right_width - 260.0, 140.0));
    let mut open = true;
    let mut close_now = false;
    let mut switched_mode = false;
    egui::Window::new("color_picker")
        .title_bar(false)
        .fixed_pos(pos)
        .fixed_size(vec2(WIDTH, HEIGHT))
        .resizable(false)
        .frame(
            egui::Frame::window(&ctx.global_style())
                .fill(PANEL)
                .corner_radius(CornerRadius::same(13))
                .shadow(egui::Shadow { offset: [0, 10], blur: 14, spread: 0, color: Color32::from_black_alpha(89) })
                .inner_margin(egui::Margin::ZERO),
        )
        .open(&mut open)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let (dialog, _) = ui.allocate_exact_size(vec2(WIDTH, HEIGHT), Sense::hover());
            let origin = dialog.min;
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
                let Some(response) = paint_kind_button(ui, origin, index, index == 3) else { continue };
                if response.clicked() && index != 3 {
                    custom::pick_kind(app, target, &mut paint, index);
                    switched_mode = true;
                    return;
                }
            }
            let blend_rect = at(origin, 208.0, 48.0, 24.0, 24.0);
            let blend = ui.interact(blend_rect, Id::new("cp_image_blend_menu"), Sense::click()).tip("Blend mode");
            ui.painter().rect_filled(blend_rect, CornerRadius::same(5), if blend.hovered() { FIELD } else { Color32::TRANSPARENT });
            icons::paint_appearance(ui.painter(), blend_rect, icons::AppearanceIcon::Blend, Color32::WHITE);
            crate::right::menu_popup(&blend).show(|ui| {
                if let Some(mode) = crate::right::blend_menu(ui, false) {
                    paint.blend = mode;
                    paint.kind = PaintKind::Image(image.clone());
                    write(app, target, &paint, false);
                }
            });
            ui.painter().hline(origin.x..=origin.x + WIDTH, origin.y + 79.5, Stroke::new(1.0, BORDER));

            let mode_rect = at(origin, 16.0, 92.0, 96.0, 24.0);
            field(ui, mode_rect, CornerRadius::same(5), true);
            let mode_name = match image.scale_mode {
                ImageScaleMode::Fill => "Fill",
                ImageScaleMode::Fit => "Fit",
                ImageScaleMode::Crop => "Crop",
                ImageScaleMode::Tile => "Tile",
            };
            ui.painter().text(pos2(origin.x + 23.0, origin.y + 104.0), Align2::LEFT_CENTER, mode_name, regular(10.0), Color32::WHITE);
            icons::paint_colorpicker(ui.painter(), at(origin, 99.0, 102.5, 5.002, 3.002), 7, Color32::WHITE);
            let mode = ui.interact(mode_rect, Id::new("cp_image_scale_mode"), Sense::click());
            crate::right::menu_popup(&mode).show(|ui| {
                for (label, value) in
                    [("Fill", ImageScaleMode::Fill), ("Fit", ImageScaleMode::Fit), ("Crop", ImageScaleMode::Crop), ("Tile", ImageScaleMode::Tile)]
                {
                    if crate::right::menu_item(ui, label, "") {
                        image.scale_mode = value;
                        paint.kind = PaintKind::Image(image.clone());
                        write(app, target, &paint, false);
                        ui.close();
                    }
                }
            });
            if vector_button(ui, at(origin, 208.0, 92.0, 24.0, 24.0), "cp_image_rotate", 12, false, "Rotate image 90°").clicked() {
                image.rotation = (image.rotation + 1).rem_euclid(4);
                paint.kind = PaintKind::Image(image.clone());
                write(app, target, &paint, false);
            }

            preview(ui, app, ctx, target, &image, origin);
            for index in 0..7 {
                let value = filter_value(&image.filters, index);
                let (next, stopped) = filter_slider(ui, origin, index, value);
                if let Some(next) = next {
                    set_filter(&mut image.filters, index, next);
                    paint.kind = PaintKind::Image(image.clone());
                    write(app, target, &paint, !stopped);
                }
                if stopped {
                    finish(app);
                }
            }
        });
    if switched_mode {
        return;
    }
    if !open || close_now || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        finish(app);
        app.ui.picker = None;
    }
}
