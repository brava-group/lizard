//! Image paints: adjustment presets, built in and saved in the file.

use figcraft_color::{ImageFilters, PaintKind};
use figcraft_doc::{Document, ImagePreset};
use serde_json::{Value, json};

use super::{CommandSpec, ok};
use crate::params::*;
use crate::{Result, Session, bad, spec};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "image.vectorize",
            "Vectorize image",
            "{id, index, expectedImage, svg} replaces one leaf image layer with editable SVG paths at the same page bounds; one undo step",
            true,
            vectorize
        ),
        spec!(
            "image.replaceBytes",
            "Replace image contents",
            "{id, index, data (base64 PNG), expectedImage?} replaces one image fill's source while preserving its placement; one undo step",
            true,
            replace_bytes
        ),
        spec!(
            "image.adjust",
            "Adjust image",
            "{ids?, target? (fill|stroke, default fill), index? (default every image paint), preset? (vivid|muted|warm|cool|blackAndWhite|fade|reset, or the name of a preset saved in the file), filters? ({exposure, contrast, saturation, temperature, tint, highlights, shadows}: each -1..1, applied over the preset)}  sets the adjustments of the layers' image paints; returns {changed}",
            true,
            adjust
        ),
        spec!(
            "image.savePreset",
            "Save image preset",
            "{name, filters? (default the adjustments of the first image paint of `ids`/the selection), ids?, target?, index?}  saved in the file; a preset with the same name is replaced",
            true,
            save_preset
        ),
        spec!("image.deletePreset", "Delete image preset", "{name}", true, |s, p| {
            let name = req_string("image.deletePreset", p, "name")?;
            let doc = s.doc_mut()?;
            let before = doc.image_presets.len();
            doc.image_presets.retain(|x| x.name != name);
            if doc.image_presets.len() == before {
                return Err(bad("image.deletePreset", format!("no preset named `{name}`")));
            }
            ok()
        }),
        spec!("image.presets", "Image presets", "{} → {builtIn: [{id, label, filters}], saved: [{name, filters}]}", false, |s, _| {
            let built_in: Vec<Value> = ImageFilters::PRESETS.iter().map(|(id, label, f)| json!({"id": id, "label": label, "filters": f})).collect();
            Ok(json!({"builtIn": built_in, "saved": s.doc()?.image_presets}))
        }),
    ]
}

/// Replace a raster layer with VTracer paths while keeping its place in the layer tree.
fn vectorize(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "image.vectorize";
    let id = req_id(CMD, p, "id")?;
    let index = req_index(CMD, p, "index")?;
    let expected = req_string(CMD, p, "expectedImage")?;
    let svg = req_string(CMD, p, "svg")?;
    let doc = s.doc_mut()?;
    let source = doc.node(id).ok_or(crate::EngineError::NoNode(id))?.clone();
    if !source.children.is_empty() {
        return Err(bad(CMD, "vectorization requires an image layer without children"));
    }
    let Some(paint) = source.fills.get(index) else { return Err(bad(CMD, "image fill no longer exists")) };
    let PaintKind::Image(image) = &paint.kind else { return Err(bad(CMD, "fill is no longer an image")) };
    if image.image != expected {
        return Err(bad(CMD, "image changed while vectorization was running"));
    }
    let parent = source.parent.ok_or_else(|| bad(CMD, "image layer has no parent"))?;
    let at = doc.index_in_parent(id).ok_or_else(|| bad(CMD, "image layer is missing from its parent"))?;
    let bounds = doc.world_bounds(id).ok_or_else(|| bad(CMD, "image layer has no bounds"))?;
    if !bounds.width().is_finite() || !bounds.height().is_finite() || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return Err(bad(CMD, "image layer has invalid bounds"));
    }
    let name = format!("{} vectorized", source.name);
    let imported = figcraft_svg::import_svg_with_notes(doc, svg.as_bytes(), parent, 0.0, 0.0, &name)
        .map_err(|error| bad(CMD, format!("cannot import traced paths: {error}")))?;
    let frame = imported.frame;
    let traced = doc.node(frame).ok_or_else(|| bad(CMD, "traced layer is missing"))?;
    if traced.children.is_empty() || traced.width <= 0.0 || traced.height <= 0.0 {
        doc.remove(frame)?;
        return Err(bad(CMD, "VTracer produced no editable paths"));
    }
    let world = figcraft_geom::Affine::translate((bounds.x0, bounds.y0))
        * figcraft_geom::Affine::scale_non_uniform(bounds.width() / traced.width, bounds.height() / traced.height);
    doc.set_world_transform(frame, world)?;
    doc.remove(id)?;
    doc.reparent(frame, parent, Some(at))?;
    let paths = doc.node(frame).map_or(0, |node| node.children.len());
    s.set_selection(vec![frame]);
    Ok(json!({"id": frame, "paths": paths, "warnings": imported.notes}))
}

/// Apply an asynchronously processed image only if the source fill is still the same.
fn replace_bytes(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "image.replaceBytes";
    let id = req_id(CMD, p, "id")?;
    let index = req_index(CMD, p, "index")?;
    let data = req_string(CMD, p, "data")?;
    let expected = string(p, "expectedImage");
    let doc = s.doc_mut()?;
    let node = doc.node(id).ok_or(crate::EngineError::NoNode(id))?;
    let Some(paint) = node.fills.get(index) else { return Err(bad(CMD, "image fill no longer exists")) };
    let PaintKind::Image(source) = &paint.kind else { return Err(bad(CMD, "fill is no longer an image")) };
    if expected.as_ref().is_some_and(|hash| *hash != source.image) {
        return Err(bad(CMD, "image changed while background removal was running"));
    }
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| bad(CMD, format!("invalid image data: {e}")))?;
    let (hash, _, _) = figcraft_format::add_image(doc, bytes).map_err(|e| bad(CMD, e.to_string()))?;
    let node = doc.get_mut(id)?;
    let old = node.fills[index].clone();
    if let PaintKind::Image(image) = &mut node.fills[index].kind {
        image.image = hash.clone();
    }
    super::lists::rebind_edited_paint(node, true, index, &old);
    node.styles.fill = None;
    Ok(json!({"id": id, "image": hash}))
}

/// The longest name a saved preset may have.
const MAX_NAME: usize = 100;
/// The most presets a file keeps.
const MAX_PRESETS: usize = 500;

/// Whether `p` asks for strokes (`target: stroke`) rather than fills.
fn strokes(cmd: &str, p: &Value) -> Result<bool> {
    match string(p, "target").as_deref() {
        None | Some("fill") | Some("fills") => Ok(false),
        Some("stroke") | Some("strokes") => Ok(true),
        Some(_) => Err(bad(cmd, "target: fill|stroke")),
    }
}

/// The preset `name`: one saved in the file under that exact name, else a built-in one.
pub fn find_preset(doc: &Document, name: &str) -> Option<ImageFilters> {
    doc.image_presets.iter().find(|x| x.name == name).map(|x| x.filters).or_else(|| ImageFilters::preset(name))
}

/// `base` with the adjustments `v` names (an object of -1..1 numbers) set.
fn with_values(cmd: &str, base: ImageFilters, v: &Value) -> Result<ImageFilters> {
    let Value::Object(m) = v else { return Err(bad(cmd, "filters: an object of adjustments")) };
    let mut f = base;
    for (k, val) in m {
        let x = f64_of(val).ok_or_else(|| bad(cmd, format!("filters.{k}: a number")))? as f32;
        let slot = match k.as_str() {
            "exposure" => &mut f.exposure,
            "contrast" => &mut f.contrast,
            "saturation" => &mut f.saturation,
            "temperature" => &mut f.temperature,
            "tint" => &mut f.tint,
            "highlights" => &mut f.highlights,
            "shadows" => &mut f.shadows,
            _ => return Err(bad(cmd, format!("unknown adjustment `{k}`"))),
        };
        *slot = x;
    }
    Ok(f.sanitized())
}

fn adjust(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "image.adjust";
    let ids = req_targets(cmd, s, p)?;
    let stroke = strokes(cmd, p)?;
    let index = match get(p, "index") {
        None | Some(Value::Null) => None,
        Some(_) => Some(req_index(cmd, p, "index")?),
    };
    let preset = match string(p, "preset") {
        Some(name) => Some(find_preset(s.doc()?, &name).ok_or_else(|| bad(cmd, format!("no preset named `{name}`")))?),
        None => None,
    };
    let values = get(p, "filters").filter(|v| !v.is_null());
    if preset.is_none() && values.is_none() {
        return Err(bad(cmd, "pass `preset` or `filters`"));
    }
    let doc = s.doc_mut()?;
    let mut changed = 0;
    for id in ids {
        let n = doc.get(id)?;
        if !if stroke { &n.strokes } else { &n.fills }.iter().any(|x| matches!(x.kind, PaintKind::Image(_))) {
            continue;
        }
        let n = doc.get_mut(id)?;
        let list = if stroke { &mut n.strokes } else { &mut n.fills };
        let mut touched = false;
        for (i, paint) in list.iter_mut().enumerate() {
            let PaintKind::Image(img) = &mut paint.kind else { continue };
            if index.is_some_and(|k| k != i) {
                continue;
            }
            let base = preset.unwrap_or(img.filters);
            img.filters = match values {
                Some(v) => with_values(cmd, base, v)?,
                None => base,
            };
            touched = true;
            changed += 1;
        }
        if touched {
            // Like any paint edit, the layer leaves its paint style.
            if stroke {
                n.styles.stroke = None;
            } else {
                n.styles.fill = None;
            }
        }
    }
    if changed == 0 {
        return Err(bad(cmd, if index.is_some() { "no image paint at that index" } else { "the layers have no image paints" }));
    }
    Ok(json!({"changed": changed}))
}

fn save_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "image.savePreset";
    let name: String = req_string(cmd, p, "name")?.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    if name.is_empty() {
        return Err(bad(cmd, "`name` is empty"));
    }
    let filters = match get(p, "filters").filter(|v| !v.is_null()) {
        Some(v) => with_values(cmd, ImageFilters::NONE, v)?,
        None => {
            let stroke = strokes(cmd, p)?;
            let index = num(p, "index").filter(|k| k.is_finite() && *k >= 0.0).map(|k| k as usize);
            let doc = s.doc()?;
            targets(s, p)
                .iter()
                .filter_map(|id| doc.node(*id))
                .flat_map(|n| if stroke { n.strokes.iter() } else { n.fills.iter() }.enumerate())
                .find_map(|(i, x)| match &x.kind {
                    PaintKind::Image(img) if index.is_none_or(|k| k == i) => Some(img.filters),
                    _ => None,
                })
                .ok_or_else(|| bad(cmd, "pass `filters`, or select a layer with an image paint"))?
        }
    };
    let doc = s.doc_mut()?;
    let preset = ImagePreset { name: name.clone(), filters };
    match doc.image_presets.iter().position(|x| x.name == name) {
        Some(i) => {
            if let Some(x) = doc.image_presets.get_mut(i) {
                *x = preset;
            }
        }
        None if doc.image_presets.len() >= MAX_PRESETS => return Err(bad(cmd, format!("a file keeps at most {MAX_PRESETS} presets"))),
        None => doc.image_presets.push(preset),
    }
    Ok(json!({"name": name, "filters": filters}))
}

#[cfg(test)]
mod replace_tests {
    use base64::Engine as _;
    use figcraft_color::{ImageScaleMode, PaintKind};
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use serde_json::json;

    use crate::Session;

    fn png(alpha: u8) -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(3, 2, Rgba([220, 20, 40, alpha])));
        let mut bytes = Vec::new();
        let result = image.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png);
        assert!(result.is_ok());
        bytes
    }

    #[test]
    fn processed_image_keeps_placement_and_undo_restores_original() {
        let mut session = Session::new();
        let original = base64::engine::general_purpose::STANDARD.encode(png(255));
        let placed = session.execute("image.place", &json!({"data": original, "mode": "crop", "width": 40, "height": 30}));
        assert!(placed.is_ok());
        let placed = placed.unwrap_or_default();
        let id = placed["id"].as_u64().unwrap_or_default();
        let source = placed["image"].as_str().unwrap_or_default().to_string();
        let replacement = base64::engine::general_purpose::STANDARD.encode(png(0));
        let params = json!({"id": id, "index": 0, "expectedImage": source, "data": replacement});
        assert!(session.execute("image.replaceBytes", &params).is_ok());
        let node = session.doc().ok().and_then(|doc| doc.node(figcraft_doc::NodeId(id)));
        assert!(node.is_some());
        if let Some(node) = node {
            assert_eq!((node.width, node.height), (40.0, 30.0));
            assert!(matches!(&node.fills[0].kind, PaintKind::Image(image) if image.scale_mode == ImageScaleMode::Crop && image.image != source));
        }
        assert!(session.execute("image.replaceBytes", &params).is_err(), "stale asynchronous result must not overwrite a newer fill");
        assert!(session.execute("edit.undo", &json!({})).is_ok());
        let node = session.doc().ok().and_then(|doc| doc.node(figcraft_doc::NodeId(id)));
        assert!(node.is_some_and(|node| matches!(&node.fills[0].kind, PaintKind::Image(image) if image.image == source)));
    }
}
