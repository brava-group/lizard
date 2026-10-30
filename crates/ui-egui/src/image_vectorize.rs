//! Non-destructive VTracer preview. Only the checkmark commits an engine command.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use egui::{Color32, Rect as UiRect, TextureHandle, Ui, pos2, vec2};
use figcraft_color::PaintKind;
use figcraft_doc::{Document, Node, NodeId};
use figcraft_geom::Rect;
use serde_json::json;

use crate::FigApp;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::primitives::{Button, ButtonSize, ButtonVariant, ControlWidth, Menu, Slider, Surface};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Color,
    Grayscale,
    BlackWhite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings {
    mode: Mode,
    colors: u8,
    threshold: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self { mode: Mode::Color, colors: 5, threshold: 128 }
    }
}

struct Preview {
    settings: Settings,
    svg: String,
    image: TextureHandle,
}

struct ResultImage {
    settings: Settings,
    svg: String,
    image: egui::ColorImage,
    background: Option<egui::ColorImage>,
}

pub(crate) struct Pending {
    doc_uid: u64,
    id: NodeId,
    index: usize,
    source: Node,
    bounds: Rect,
    png: Arc<Vec<u8>>,
    settings: Settings,
    changed_at: Instant,
    rx: Option<Receiver<Result<ResultImage, String>>>,
    preview: Option<Preview>,
    background: Option<TextureHandle>,
    document: Arc<Document>,
    page: NodeId,
}

pub(crate) fn start(app: &mut FigApp, ctx: &egui::Context, id: NodeId, index: usize) {
    if app.pending_image_vectorize.is_some() {
        app.toast(tr("Image vectorization is already running"));
        return;
    }
    let Some((source, bounds, document, page, doc_uid)) = app.session.ds().ok().and_then(|ds| {
        let node = ds.doc.node(id)?;
        let PaintKind::Image(_) = &node.fills.get(index)?.kind else { return None };
        Some((node.clone(), ds.doc.world_bounds(id)?, ds.doc.clone(), ds.page, ds.uid))
    }) else {
        app.show_error(tr("Selected image data is unavailable").to_string());
        return;
    };
    if !source.children.is_empty() {
        app.show_error(tr("Vectorization requires an image layer without children").to_string());
        return;
    }
    let longest = bounds.width().max(bounds.height());
    if !longest.is_finite() || longest <= 0.0 || longest > 102_400.0 {
        app.show_error(tr("Image is too large to vectorize").to_string());
        return;
    }
    let scale = (1024.0 / longest).min(1.0);
    let Some(exported) = app.run("file.export", json!({"ids": [id], "format": "png", "includeBoundingBox": true, "scale": scale})) else {
        return;
    };
    let Some(encoded) = exported.get("data").and_then(serde_json::Value::as_str) else {
        app.show_error(tr("Could not read the image export").to_string());
        return;
    };
    let Ok(png) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        app.show_error(tr("Could not read the image export").to_string());
        return;
    };
    let mut pending = Pending {
        doc_uid,
        id,
        index,
        source,
        bounds,
        png: Arc::new(png),
        settings: Settings::default(),
        changed_at: Instant::now(),
        rx: None,
        preview: None,
        background: None,
        document,
        page,
    };
    if let Err(error) = launch(&mut pending, ctx) {
        app.show_error(format!("VTracer: {error}"));
        return;
    }
    app.pending_image_vectorize = Some(pending);
    app.toast(tr("Vectorizing image…"));
}

fn launch(pending: &mut Pending, ctx: &egui::Context) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let png = pending.png.clone();
    let settings = pending.settings;
    let background = pending.background.is_none();
    let document = pending.document.clone();
    let page = pending.page;
    let id = pending.id;
    let bounds = pending.bounds;
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("vtracer-preview".into())
        .spawn(move || {
            let result = trace(&png, settings).map(|(svg, image)| {
                let background = if background { render_background(&document, page, id, bounds, image.size) } else { None };
                ResultImage { settings, svg, image, background }
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        })
        .map_err(|error| error.to_string())?;
    pending.rx = Some(rx);
    Ok(())
}

fn source_unchanged(app: &FigApp, pending: &Pending) -> bool {
    app.session.ds().ok().is_some_and(|ds| {
        ds.uid == pending.doc_uid
            && ds.selection.as_slice() == [pending.id]
            && ds.doc.node(pending.id) == Some(&pending.source)
            && ds.doc.world_bounds(pending.id) == Some(pending.bounds)
    })
}

pub(crate) fn poll(app: &mut FigApp, ctx: &egui::Context) {
    let Some(mut pending) = app.pending_image_vectorize.take() else { return };
    if !source_unchanged(app, &pending) {
        app.toast(tr("Vectorization canceled because the image changed"));
        return;
    }
    if let Some(rx) = pending.rx.take() {
        match rx.try_recv() {
            Err(TryRecvError::Empty) => pending.rx = Some(rx),
            Err(TryRecvError::Disconnected) => {
                app.show_error(tr("Image vectorization stopped unexpectedly").to_string());
                return;
            }
            Ok(Err(error)) => {
                app.show_error(format!("VTracer: {error}"));
                return;
            }
            Ok(Ok(result)) => {
                if let Some(background) = result.background {
                    pending.background = Some(ctx.load_texture("vtracer-background", background, egui::TextureOptions::LINEAR));
                }
                let image = ctx.load_texture("vtracer-preview", result.image, egui::TextureOptions::LINEAR);
                pending.preview = Some(Preview { settings: result.settings, svg: result.svg, image });
            }
        }
    }
    if pending.rx.is_none() && pending.preview.as_ref().is_none_or(|preview| preview.settings != pending.settings) {
        let remaining = Duration::from_millis(120).saturating_sub(pending.changed_at.elapsed());
        if remaining.is_zero() {
            if let Err(error) = launch(&mut pending, ctx) {
                app.show_error(format!("VTracer: {error}"));
                return;
            }
        } else {
            ctx.request_repaint_after(remaining);
        }
    }
    if pending.rx.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    app.pending_image_vectorize = Some(pending);
}

/// Draw the trace over a rendering of the page without the source image. This preserves other
/// layers showing through transparent regions while the actual document stays untouched.
pub(crate) fn paint_preview(app: &FigApp, painter: &egui::Painter, canvas_background: Color32) {
    let Some(pending) = app.pending_image_vectorize.as_ref() else { return };
    let Some(preview) = pending.preview.as_ref() else { return };
    let min = app.to_screen(pending.bounds.x0, pending.bounds.y0);
    let max = app.to_screen(pending.bounds.x1, pending.bounds.y1);
    let rect = UiRect::from_min_max(min, max);
    if !rect.is_positive() {
        return;
    }
    let uv = UiRect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    painter.rect_filled(rect, egui::CornerRadius::ZERO, canvas_background);
    if let Some(background) = pending.background.as_ref() {
        painter.image(background.id(), rect, uv, Color32::WHITE);
    }
    painter.image(preview.image.id(), rect, uv, Color32::WHITE);
}

pub(crate) fn toolbar(app: &mut FigApp, ui: &mut Ui) {
    let Some(pending) = app.pending_image_vectorize.as_mut() else { return };
    let main = crate::toolbar::pill_rect(app.canvas.rect, crate::toolbar::main_width());
    // Surface margins plus the exact width of every cell and separator.
    let width = 16.0 + 28.0 + 83.0 + 3.0 * 17.0 + 3.0 * 32.0 + 174.0 + 32.0;
    let at = pos2(app.canvas.rect.center().x - width / 2.0, main.min.y - 48.0);
    let mut accept = false;
    let mut cancel = false;
    Menu::area(egui::Id::new("vectorize_toolbar"), at).show(ui.ctx(), |ui| {
        let bounds = UiRect::from_min_size(at, vec2(width, 40.0));
        ui.interact(bounds, ui.id().with("vectorize_toolbar_background"), egui::Sense::CLICK | egui::Sense::DRAG);
        crate::a11y::area(&mut app.ui.nav, crate::a11y::Region::Toolbar, bounds);
        crate::a11y::container_bounds(ui, bounds);
        Surface::palette().show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                cancel = Button::icon(Icon::Close, tr("Cancel")).width(ControlWidth::Fixed(28.0)).show(ui).clicked();
                ui.add_sized([83.0, 32.0], egui::Label::new(tr("Vectorize")));
                divider(ui);
                for (mode, title) in [(Mode::Color, tr("Full color")), (Mode::Grayscale, tr("Grayscale")), (Mode::BlackWhite, tr("Black and white"))]
                {
                    let response = Button::new("")
                        .variant(if pending.settings.mode == mode { ButtonVariant::Secondary } else { ButtonVariant::Ghost })
                        .width(ControlWidth::Fixed(32.0))
                        .show(ui)
                        .on_hover_text(title);
                    crate::a11y::named_button(&response, title);
                    paint_mode(ui, response.rect, mode);
                    if response.clicked() {
                        pending.settings.mode = mode;
                        pending.changed_at = Instant::now();
                    }
                }
                divider(ui);
                let black_white = pending.settings.mode == Mode::BlackWhite;
                let value = if black_white { f64::from(pending.settings.threshold) } else { f64::from(pending.settings.colors) };
                let slider = Slider::new(ui.id().with("vectorize_amount"), if black_white { tr("Threshold") } else { tr("Number of colors") })
                    .range(if black_white { 0.0 } else { 1.0 }, if black_white { 255.0 } else { 16.0 })
                    .step(1.0)
                    .value(Some(value))
                    .width(ControlWidth::Fixed(174.0))
                    .show(ui);
                if let Some((value, _)) = slider.change {
                    if black_white {
                        pending.settings.threshold = value.round().clamp(0.0, 255.0) as u8;
                    } else {
                        pending.settings.colors = value.round().clamp(1.0, 16.0) as u8;
                    }
                    pending.changed_at = Instant::now();
                }
                divider(ui);
                let ready = pending.preview.as_ref().is_some_and(|preview| preview.settings == pending.settings) && pending.rx.is_none();
                accept = Button::icon(Icon::Check, tr("Apply vectorization"))
                    .variant(ButtonVariant::Primary)
                    .size(ButtonSize::Large)
                    .width(ControlWidth::Fixed(32.0))
                    .enabled(ready)
                    .show(ui)
                    .clicked();
            });
        });
    });
    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    if cancel {
        app.pending_image_vectorize = None;
    } else if accept {
        confirm(app);
    }
}

fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(17.0, 32.0), egui::Sense::hover());
    let x = rect.center().x;
    ui.painter().line_segment(
        [pos2(x, rect.top() + 4.0), pos2(x, rect.bottom() - 4.0)],
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
}

fn paint_mode(ui: &Ui, rect: UiRect, mode: Mode) {
    let swatch = UiRect::from_center_size(rect.center(), vec2(16.0, 16.0));
    let painter = ui.painter();
    match mode {
        Mode::Color => {
            for (i, color) in [Color32::RED, Color32::YELLOW, Color32::GREEN, Color32::BLUE].into_iter().enumerate() {
                let tile = UiRect::from_min_size(pos2(swatch.min.x + i as f32 * 4.0, swatch.min.y), vec2(4.0, 16.0));
                let corners = egui::CornerRadius {
                    nw: if i == 0 { 3 } else { 0 },
                    sw: if i == 0 { 3 } else { 0 },
                    ne: if i == 3 { 3 } else { 0 },
                    se: if i == 3 { 3 } else { 0 },
                };
                painter.rect_filled(tile, corners, color);
            }
        }
        Mode::Grayscale => {
            for i in 0..4 {
                let shade = (i * 72 + 24) as u8;
                let tile = UiRect::from_min_size(pos2(swatch.min.x + i as f32 * 4.0, swatch.min.y), vec2(4.0, 16.0));
                let corners = egui::CornerRadius {
                    nw: if i == 0 { 3 } else { 0 },
                    sw: if i == 0 { 3 } else { 0 },
                    ne: if i == 3 { 3 } else { 0 },
                    se: if i == 3 { 3 } else { 0 },
                };
                painter.rect_filled(tile, corners, Color32::from_gray(shade));
            }
        }
        Mode::BlackWhite => {
            painter.rect_filled(swatch, egui::CornerRadius::same(3), Color32::BLACK);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(swatch.right() - 3.0, swatch.top()),
                    pos2(swatch.right(), swatch.top() + 3.0),
                    pos2(swatch.right(), swatch.bottom() - 3.0),
                    pos2(swatch.right() - 3.0, swatch.bottom()),
                    pos2(swatch.left() + 3.0, swatch.bottom()),
                    pos2(swatch.left(), swatch.bottom() - 3.0),
                ],
                Color32::WHITE,
                egui::Stroke::NONE,
            ));
        }
    }
    painter.rect_stroke(swatch, egui::CornerRadius::same(2), egui::Stroke::new(1.0, Color32::from_gray(125)), egui::StrokeKind::Inside);
}

fn confirm(app: &mut FigApp) {
    let Some(pending) = app.pending_image_vectorize.take() else { return };
    if !source_unchanged(app, &pending) {
        app.toast(tr("Vectorization canceled because the image changed"));
        return;
    }
    let Some(preview) = pending.preview.filter(|preview| preview.settings == pending.settings) else { return };
    let Some(paint) = pending.source.fills.get(pending.index) else { return };
    let PaintKind::Image(image) = &paint.kind else { return };
    if app.run("image.vectorize", json!({"id": pending.id, "index": pending.index, "expectedImage": image.image, "svg": preview.svg})).is_some() {
        app.toast(tr("Image vectorized"));
    }
}

fn render_background(doc: &Document, page: NodeId, id: NodeId, bounds: Rect, size: [usize; 2]) -> Option<egui::ColorImage> {
    let (width, height) = (u32::try_from(size[0]).ok()?, u32::try_from(size[1]).ok()?);
    let view = figcraft_render::View { origin: (bounds.x0, bounds.y0), scale: f64::from(width) / bounds.width(), width, height };
    let opts = figcraft_render::Options { skip: vec![id], ..Default::default() };
    let pixels = figcraft_render::render_view(doc, page, &view, &opts)?;
    Some(crate::render_worker::screen_image(&pixels, doc.color_profile))
}

fn trace(png: &[u8], settings: Settings) -> Result<(String, egui::ColorImage), String> {
    let mut rgba = image::load_from_memory(png).map_err(|error| format!("cannot decode image: {error}"))?.to_rgba8();
    if rgba.pixels().all(|pixel| pixel[3] < 128) {
        return Err("image has no visible pixels".into());
    }
    prepare_pixels(&mut rgba, settings);
    let (width, height) = rgba.dimensions();
    let source = vtracer::ColorImage { pixels: rgba.into_raw(), width: width as usize, height: height as usize };
    let svg = vtracer::convert(source, vtracer::Config::from_preset(vtracer::Preset::Poster))?;
    if svg.paths.is_empty() {
        return Err("image has no traceable shapes".into());
    }
    let svg = svg.to_string();
    let image = raster_svg(&svg, width, height)?;
    Ok((svg, image))
}

fn prepare_pixels(image: &mut image::RgbaImage, settings: Settings) {
    let palette = if settings.mode == Mode::Color { quantize_palette(image, settings.colors as usize) } else { Vec::new() };
    for pixel in image.pixels_mut() {
        if pixel[3] < 128 {
            pixel[3] = 0;
            continue;
        }
        pixel[3] = 255;
        match settings.mode {
            Mode::Color => {
                if let Some(color) = palette.iter().min_by_key(|color| distance([pixel[0], pixel[1], pixel[2]], **color)) {
                    pixel[0] = color[0];
                    pixel[1] = color[1];
                    pixel[2] = color[2];
                }
            }
            Mode::Grayscale => {
                let luminance = (u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29) / 256;
                let count = u32::from(settings.colors.max(2));
                let shade = ((luminance * (count - 1) + 127) / 255 * 255 / (count - 1)) as u8;
                pixel[0] = shade;
                pixel[1] = shade;
                pixel[2] = shade;
            }
            Mode::BlackWhite => {
                let luminance = (u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29) / 256;
                let shade = if luminance >= u32::from(settings.threshold) { 255 } else { 0 };
                pixel[0] = shade;
                pixel[1] = shade;
                pixel[2] = shade;
            }
        }
    }
}

fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.into_iter().zip(b).map(|(a, b)| u32::from(a.abs_diff(b)).pow(2)).sum()
}

fn quantize_palette(image: &image::RgbaImage, count: usize) -> Vec<[u8; 3]> {
    let pixels: Vec<[u8; 3]> = image
        .pixels()
        .step_by((image.len() / 4 / 32_768).max(1))
        .filter(|pixel| pixel[3] >= 128)
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    if pixels.is_empty() {
        return Vec::new();
    }
    let mut palette = vec![pixels[0]];
    while palette.len() < count.min(16) {
        let Some(&next) = pixels.iter().max_by_key(|pixel| palette.iter().map(|color| distance(**pixel, *color)).min().unwrap_or(0)) else { break };
        if palette.contains(&next) {
            break;
        }
        palette.push(next);
    }
    for _ in 0..5 {
        let mut sums = vec![[0u64; 4]; palette.len()];
        for pixel in &pixels {
            let Some((index, _)) = palette.iter().enumerate().min_by_key(|(_, color)| distance(*pixel, **color)) else { continue };
            for channel in 0..3 {
                sums[index][channel] += u64::from(pixel[channel]);
            }
            sums[index][3] += 1;
        }
        for (color, sum) in palette.iter_mut().zip(sums) {
            for channel in 0..3 {
                if let Some(average) = sum[channel].checked_div(sum[3]) {
                    color[channel] = average as u8;
                }
            }
        }
    }
    palette
}

fn raster_svg(svg: &str, width: u32, height: u32) -> Result<egui::ColorImage, String> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|error| error.to_string())?;
    let mut pixels = tiny_skia::Pixmap::new(width, height).ok_or("cannot allocate vector preview")?;
    fn paths(nodes: &[usvg::Node], pixels: &mut tiny_skia::Pixmap) {
        for node in nodes {
            match node {
                usvg::Node::Group(group) => paths(group.children(), pixels),
                usvg::Node::Path(path) => {
                    let Some(fill) = path.fill() else { continue };
                    let usvg::Paint::Color(color) = fill.paint() else { continue };
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.red, color.green, color.blue, (fill.opacity().get() * 255.0).round() as u8);
                    paint.anti_alias = true;
                    if let Some(data) = path.data().clone().transform(path.abs_transform()) {
                        let rule = match fill.rule() {
                            usvg::FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
                            usvg::FillRule::NonZero => tiny_skia::FillRule::Winding,
                        };
                        pixels.fill_path(&data, &paint, rule, tiny_skia::Transform::identity(), None);
                    }
                }
                _ => {}
            }
        }
    }
    paths(tree.root().children(), &mut pixels);
    Ok(egui::ColorImage::from_rgba_premultiplied([width as usize, height as usize], pixels.data()))
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use figcraft_color::PaintKind;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use serde_json::json;

    use super::{Mode, Settings, confirm, prepare_pixels, start, trace};

    #[test]
    fn recolor_modes_change_pixels_and_preserve_transparency() {
        let mut image = RgbaImage::from_pixel(2, 1, Rgba([230, 20, 35, 255]));
        image.put_pixel(1, 0, Rgba([100, 150, 200, 0]));
        prepare_pixels(&mut image, Settings { mode: Mode::Grayscale, colors: 3, threshold: 128 });
        assert_eq!(image.get_pixel(0, 0)[0], image.get_pixel(0, 0)[1]);
        assert_eq!(image.get_pixel(1, 0)[3], 0);
        prepare_pixels(&mut image, Settings { mode: Mode::BlackWhite, colors: 3, threshold: 128 });
        assert!(matches!(image.get_pixel(0, 0)[0], 0 | 255));
    }

    #[test]
    fn preview_does_not_change_document_until_confirmed() {
        let (mut harness, _) = crate::live_tests::Harness::new();
        let mut pixels = RgbaImage::from_pixel(24, 24, Rgba([0, 0, 0, 0]));
        for y in 4..20 {
            for x in 4..20 {
                pixels.put_pixel(x, y, Rgba([210, 35, 70, 255]));
            }
        }
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(pixels).write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let result = harness
            .app
            .session
            .execute(
                "image.place",
                &json!({"data": base64::engine::general_purpose::STANDARD.encode(png), "x": 20, "y": 30, "width": 48, "height": 48}),
            )
            .unwrap();
        let id = figcraft_doc::NodeId(result["id"].as_u64().unwrap());
        harness.app.session.execute("select.set", &json!({"ids": [id.0]})).unwrap();
        start(&mut harness.app, &harness.ctx, id, 0);
        for _ in 0..100 {
            harness.frame(vec![]);
            if harness.app.pending_image_vectorize.as_ref().is_some_and(|pending| pending.preview.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(harness.app.pending_image_vectorize.as_ref().is_some_and(|pending| pending.preview.is_some()));
        assert!(matches!(&harness.app.session.doc().unwrap().node(id).unwrap().fills[0].kind, PaintKind::Image(_)));
        confirm(&mut harness.app);
        assert!(harness.app.session.doc().unwrap().node(id).is_none());
    }

    #[test]
    fn vtracer_paths_replace_a_transparent_image_and_undo_restores_it() {
        let mut pixels = RgbaImage::from_pixel(48, 48, Rgba([0, 0, 0, 0]));
        for y in 8..40 {
            for x in 8..40 {
                pixels.put_pixel(x, y, Rgba([230, 20, 35, 255]));
            }
        }
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(pixels).write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let (svg, preview) = trace(&png, Settings::default()).unwrap();
        assert!(svg.contains("<path"));
        assert_eq!(preview.size, [48, 48]);
        assert_eq!(preview.pixels[0].a(), 0);
        assert!(preview.pixels[24 * 48 + 24].r() > 150);

        let mut session = figcraft_engine::Session::new();
        let original = session
            .execute(
                "image.place",
                &json!({"data": base64::engine::general_purpose::STANDARD.encode(&png), "x": 12, "y": 24, "width": 96, "height": 96}),
            )
            .unwrap();
        let id = original["id"].as_u64().unwrap();
        let original_hash = original["image"].as_str().unwrap();
        let vector = session.execute("image.vectorize", &json!({"id": id, "index": 0, "expectedImage": original_hash, "svg": svg})).unwrap();
        let vector_id = vector["id"].as_u64().unwrap();
        assert!(vector["paths"].as_u64().unwrap() > 0);
        let doc = session.doc().unwrap();
        assert!(doc.node(figcraft_doc::NodeId(id)).is_none());
        let traced = doc.node(figcraft_doc::NodeId(vector_id)).unwrap();
        assert_eq!(doc.world_bounds(figcraft_doc::NodeId(vector_id)), Some(figcraft_geom::Rect::new(12.0, 24.0, 108.0, 120.0)));
        assert!(!traced.children.is_empty());

        let exported = session.execute("file.export", &json!({"ids": [vector_id], "format": "png", "includeBoundingBox": true})).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(exported["data"].as_str().unwrap()).unwrap();
        let result = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(result.get_pixel(0, 0)[3], 0);
        assert!(result.get_pixel(48, 48)[0] > 150 && result.get_pixel(48, 48)[3] > 200);

        session.execute("edit.undo", &json!({})).unwrap();
        let restored = session.doc().unwrap().node(figcraft_doc::NodeId(id)).unwrap();
        assert!(matches!(&restored.fills[0].kind, PaintKind::Image(_)));
    }
}
