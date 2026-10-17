//! Paste of layers copied from another design app: the clipboard's design data (a `.fig` canvas in
//! its HTML) read through the `.fig` import and pasted as Paste pastes our own layers.

use base64::Engine as _;
use serde_json::json;

use super::*;
use crate::cmd::edit::{CLIP_CLOSE, CLIP_OPEN, design_clipboard_buffer, design_clipboard_image_urls, restore_copied_images};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn id(v: &Value) -> NodeId {
    params::id_of(&v["id"]).unwrap()
}

fn copied_file() -> Vec<u8> {
    let mut s = Session::new();
    let frame = id(&s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 200, "height": 100, "name": "Card"})).unwrap());
    s.execute("node.create", &json!({"type": "rectangle", "x": 10, "y": 10, "width": 40, "height": 20, "parent": frame, "name": "Badge"})).unwrap();
    s.execute("node.create", &json!({"type": "ellipse", "x": 300, "y": 0, "width": 50, "height": 50, "name": "Dot"})).unwrap();
    let r = s.execute("file.export", &json!({"format": "fig"})).unwrap();
    B64.decode(r["data"].as_str().unwrap()).unwrap()
}

fn clipboard_html(buffer: &[u8]) -> String {
    let mut wrapped = B64.encode(buffer);
    wrapped.insert(8, '\n');
    format!(r#"<meta charset="utf-8"><span data-buffer="<!--{CLIP_OPEN}{wrapped}{CLIP_CLOSE}-->"></span>"#)
}

fn canvas_from_file(file: Vec<u8>) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(file)).unwrap();
    let mut canvas = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("canvas.fig").unwrap(), &mut canvas).unwrap();
    canvas
}

fn bare_canvas() -> Vec<u8> {
    canvas_from_file(copied_file())
}

fn png(img: &image::RgbaImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn clipboard_metadata_only_accepts_signed_figma_image_links() {
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let meta = json!({"imageHashes": {
        hash: "https://s3-alpha-sig.figma.com/img/example?Expires=123&Signature=abc",
        "not-a-hash": "https://s3-alpha-sig.figma.com/img/other",
        "abcdefabcdefabcdefabcdefabcdefabcdefabcd": "https://example.com/img/other"
    }});
    let html = format!("(figmeta){}(/figmeta)", B64.encode(meta.to_string()));
    let urls = design_clipboard_image_urls(&html);
    assert_eq!(urls.len(), 1);
    assert_eq!(urls.get(hash).map(String::as_str), Some("https://s3-alpha-sig.figma.com/img/example?Expires=123&Signature=abc"));
    assert!(design_clipboard_image_urls("(figmeta)not base64(/figmeta)").is_empty());
}

#[test]
fn signed_image_bytes_replace_placeholder_without_changing_fill_settings() {
    use sha1::{Digest as _, Sha1};

    let original = png(&image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 98, 204, 255])));
    let hash = format!("{:x}", Sha1::digest(&original));
    let mut source = Session::new();
    let photo = id(&source.execute("image.place", &json!({"data": B64.encode(&original), "width": 30, "height": 20})).unwrap());
    let doc = source.doc_mut().unwrap();
    let old_key = match &doc.node(photo).unwrap().fills[0].kind {
        figcraft_color::PaintKind::Image(img) => img.image.clone(),
        _ => panic!("expected an image fill"),
    };
    let mut placeholder = doc.images.remove(&old_key).unwrap();
    placeholder.bytes = std::sync::Arc::new(vec![0]);
    doc.images.insert(hash.clone(), placeholder);
    let figcraft_color::PaintKind::Image(img) = &mut doc.node_mut(photo).unwrap().fills[0].kind else { panic!("expected an image fill") };
    img.image = hash.clone();
    img.scale_mode = figcraft_color::ImageScaleMode::Fit;
    img.scale = 0.42;

    let mut wrong = serde_json::Map::new();
    wrong.insert(hash.clone(), json!(B64.encode(b"wrong bytes")));
    restore_copied_images(doc, &wrong);
    assert_eq!(doc.images[&hash].bytes.as_ref(), &[0]);

    let mut images = serde_json::Map::new();
    images.insert(hash.clone(), json!(B64.encode(&original)));
    restore_copied_images(doc, &images);
    let figcraft_color::PaintKind::Image(img) = &doc.node(photo).unwrap().fills[0].kind else { panic!("expected an image fill") };
    assert_eq!(img.scale_mode, figcraft_color::ImageScaleMode::Fit);
    assert_eq!(img.scale, 0.42);
    assert_eq!(doc.images[&img.image].bytes.as_ref(), &original);
    assert!(!doc.images.contains_key(&hash));
}

#[test]
fn clipboard_preview_recovers_missing_image_in_its_layer() {
    // A real Figma clipboard canvas can keep an image hash without its bytes. The PNG preview
    // covers the copied frame, and the photo occupies just one child rectangle within it.
    let original = png(&image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 255])));
    let mut source = Session::new();
    let frame = id(&source.execute("frame.create", &json!({"x": 0, "y": 0, "width": 200, "height": 100, "name": "Card"})).unwrap());
    source
        .execute(
            "image.place",
            &json!({"data": B64.encode(&original), "x": 20, "y": 10, "width": 40, "height": 20, "parent": frame, "name": "Photo"}),
        )
        .unwrap();
    let file = B64.decode(source.execute("file.export", &json!({"format": "fig"})).unwrap()["data"].as_str().unwrap()).unwrap();
    let canvas = canvas_from_file(file.clone());
    let mut preview = image::RgbaImage::from_pixel(400, 200, image::Rgba([255, 255, 255, 255]));
    for y in 20..60 {
        for x in 40..120 {
            preview.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
        }
    }

    let mut pasted = Session::new();
    let result = pasted.execute("edit.pasteDesignLayers", &json!({"html": clipboard_html(&canvas), "preview": B64.encode(png(&preview))})).unwrap();
    assert!(!result["warnings"].to_string().contains("image fill(s) whose image isn't"));
    let card = params::id_of(&result["ids"][0]).unwrap();
    let doc = pasted.doc().unwrap();
    let photo = doc.node(doc.node(card).unwrap().children[0]).unwrap();
    assert_eq!(photo.name, "Photo");
    assert_eq!((photo.width, photo.height), (40.0, 20.0));
    let figcraft_color::PaintKind::Image(img) = &photo.fills[0].kind else { panic!("photo fill") };
    let recovered = image::load_from_memory(&doc.images[&img.image].bytes).unwrap().to_rgba8();
    assert_eq!(recovered.dimensions(), (80, 40));
    assert_eq!(recovered.get_pixel(40, 20).0, [255, 0, 0, 255]);
    let rendered = figcraft_render::render_node(doc, card, 1.0).unwrap();
    let pixel = |x: u32, y: u32| {
        let at = ((y * rendered.width() + x) * 4) as usize;
        &rendered.data()[at..at + 4]
    };
    assert_eq!(pixel(40, 20), [255, 0, 0, 255]);
    assert_ne!(pixel(150, 50), [255, 0, 0, 255]);

    let mut without_preview = Session::new();
    let result = without_preview.execute("edit.pasteDesignLayers", &json!({"html": clipboard_html(&canvas)})).unwrap();
    assert!(result["warnings"].to_string().contains("image fill(s) whose image isn't"));

    // When the actual image bytes exist in the .fig archive, the preview must not replace them.
    let mut with_bytes = Session::new();
    let result = with_bytes.execute("edit.pasteDesignLayers", &json!({"html": clipboard_html(&file), "preview": B64.encode(png(&preview))})).unwrap();
    let doc = with_bytes.doc().unwrap();
    let card = params::id_of(&result["ids"][0]).unwrap();
    let photo = doc.node(doc.node(card).unwrap().children[0]).unwrap();
    let figcraft_color::PaintKind::Image(img) = &photo.fills[0].kind else { panic!("photo fill") };
    let imported = image::load_from_memory(&doc.images[&img.image].bytes).unwrap().to_rgba8();
    assert_eq!(imported.get_pixel(0, 0).0, [0, 0, 255, 255]);
}

fn names(s: &Session, ids: &[NodeId]) -> Vec<String> {
    let doc = s.doc().unwrap();
    ids.iter().map(|i| doc.node(*i).unwrap().name.clone()).collect()
}

#[test]
fn buffer_is_found_in_the_html() {
    let file = copied_file();
    assert_eq!(design_clipboard_buffer(&clipboard_html(&file)), Some(file));
    assert_eq!(design_clipboard_buffer("<p>plain</p>"), None);
    assert_eq!(design_clipboard_buffer(&format!("{CLIP_OPEN}not base64!{CLIP_CLOSE}")), None);
    assert_eq!(design_clipboard_buffer(&format!("{CLIP_OPEN}{}{CLIP_CLOSE}", B64.encode(b"hello"))), None);
    assert_eq!(design_clipboard_buffer(&format!("{CLIP_OPEN}ZmlnLQ")), None);
    // Web text that only mentions the markers holds no design data.
    assert_eq!(design_clipboard_buffer(&format!("<p>see {CLIP_OPEN} and {CLIP_CLOSE} in the docs</p>")), None);
}

#[test]
fn bare_clipboard_canvas_pastes_as_editable_layers() {
    let canvas = bare_canvas();
    assert!(canvas.starts_with(b"fig-"));
    let html = clipboard_html(&canvas);
    assert_eq!(design_clipboard_buffer(&html), Some(canvas));
    let mut session = Session::new();
    let result = session.execute("edit.pasteDesignLayers", &json!({"html": html})).unwrap();
    let ids: Vec<NodeId> = result["ids"].as_array().unwrap().iter().map(|value| params::id_of(value).unwrap()).collect();
    assert_eq!(names(&session, &ids), ["Card", "Dot"]);
    assert_eq!(names(&session, &session.doc().unwrap().node(ids[0]).unwrap().children), ["Badge"]);
}

#[test]
fn oversized_design_data_is_refused_before_decoding() {
    // Just past the cap (256 MB of data, as base64), made of valid base64 characters.
    let big = "A".repeat((256 * 1024 * 1024usize).div_ceil(3) * 4 + 4);
    assert_eq!(design_clipboard_buffer(&format!("{CLIP_OPEN}{big}{CLIP_CLOSE}")), None);
    let mut s = Session::new();
    let e = s.execute("edit.pasteDesignLayers", &json!({ "data": big })).unwrap_err();
    assert!(e.to_string().contains("256 MB"), "{e}");
    assert!(s.clipboard.nodes.is_empty());
}

#[test]
fn layers_paste_into_the_page_and_undo() {
    let mut s = Session::new();
    let page = s.page().unwrap();
    let r = s.execute("edit.pasteDesignLayers", &json!({"html": clipboard_html(&copied_file())})).unwrap();
    let ids: Vec<NodeId> = r["ids"].as_array().unwrap().iter().map(|v| params::id_of(v).unwrap()).collect();
    assert_eq!(names(&s, &ids), ["Card", "Dot"]);
    assert_eq!(s.selection(), ids);
    let doc = s.doc().unwrap();
    assert!(ids.iter().all(|i| doc.node(*i).unwrap().parent == Some(page)));
    let card = doc.node(ids[0]).unwrap();
    assert_eq!(names(&s, &card.children), ["Badge"]);
    assert_eq!((card.width, card.height), (200.0, 100.0));
    assert!(s.undo().unwrap());
    assert!(ids.iter().all(|i| !s.doc().unwrap().contains(*i)));
}

#[test]
fn a_second_paste_repeats_the_copied_layers() {
    let mut s = Session::new();
    // `data` may be wrapped and unpadded, like the HTML's base64.
    let mut data = B64.encode(copied_file()).trim_end_matches('=').to_string();
    data.insert(8, '\n');
    s.execute("edit.pasteDesignLayers", &json!({ "data": data })).unwrap();
    let r = s.execute("edit.paste", &json!({})).unwrap();
    let ids: Vec<NodeId> = r["ids"].as_array().unwrap().iter().map(|v| params::id_of(v).unwrap()).collect();
    assert_eq!(names(&s, &ids), ["Card", "Dot"]);
}

#[test]
fn pastes_into_the_selected_frame() {
    let mut s = Session::new();
    let host = id(&s.execute("frame.create", &json!({"x": 1000, "y": 0, "width": 800, "height": 600})).unwrap());
    s.execute("select.set", &json!({"ids": [host]})).unwrap();
    let r = s.execute("edit.pasteDesignLayers", &json!({"html": clipboard_html(&copied_file())})).unwrap();
    let first = params::id_of(&r["ids"][0]).unwrap();
    assert_eq!(s.doc().unwrap().node(first).unwrap().parent, Some(host));
}

#[test]
fn other_content_is_refused_without_changes() {
    let mut s = Session::new();
    let before = s.doc().unwrap().nodes().count();
    assert!(s.execute("edit.pasteDesignLayers", &json!({})).is_err());
    assert!(s.execute("edit.pasteDesignLayers", &json!({"html": "<b>hi</b>"})).is_err());
    assert!(s.execute("edit.pasteDesignLayers", &json!({"data": B64.encode(b"PK not a zip")})).is_err());
    assert!(s.execute("edit.pasteDesignLayers", &json!({"data": B64.encode(b"fig-kiwi\x00\x00\x00\x00junk")})).is_err());
    assert_eq!(s.doc().unwrap().nodes().count(), before);
    assert!(s.clipboard.nodes.is_empty());
}
