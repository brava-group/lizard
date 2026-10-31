//! Place one icon from the bundled open-source catalogs as editable, self-contained vectors.

use lizard_color::{Color, PaintKind};
use lizard_doc::{Document, NodeId};
use lizard_geom::Affine;
use serde_json::{Value, json};

use super::CommandSpec;
use crate::icon_catalog;
use crate::params::{boolean, color_of, get, num, num_or, opt_id, req_id, req_string, string};
use crate::{Result, Session, bad, spec};

pub const LIBRARY_KEY: &str = "lizard.icon.library";
pub const NAME_KEY: &str = "lizard.icon.name";
pub const COLOR_KEY: &str = "lizard.icon.color";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "icon.create",
            "Place library icon",
            "{library: lucide|feather|phosphor|material-symbols-outlined|material-symbols-rounded|material-symbols-sharp, icon: exact icon name from search_icons, x?, y?, width?, height? (default 24, aspect ratio preserved when only one is set), parent?, name?, color? (#hex, default black), props?, select?} → {id, library, icon}. Search with the read-only MCP search_icons tool first; reuse the same library across a screen.",
            true,
            create
        ),
        spec!(
            "icon.set",
            "Edit library icon",
            "{id, library?, icon? (exact name from search_icons), color? (#hex), width?, height?}. Swaps the icon artwork while preserving the layer id, position, effects and undo history; changing colour or size also updates the artwork.",
            true,
            set
        ),
    ]
}

fn icon_color(p: &Value) -> Result<Color> {
    let value = get(p, "color").or_else(|| get(p, "fill")).or_else(|| get(p, "props").and_then(|props| props.get("fill")));
    match value {
        Some(value) => color_of(value).ok_or_else(|| bad("icon.create", "`color` must be a #RGB, #RRGGBB or #RRGGBBAA colour")),
        None => Ok(Color::BLACK),
    }
}

fn recolor(doc: &mut Document, root: NodeId, color: Color) {
    let mut pending = doc.node(root).map_or_else(Vec::new, |node| node.children.clone());
    while let Some(id) = pending.pop() {
        if let Some(node) = doc.node_mut(id) {
            pending.extend(node.children.iter().copied());
            for paint in node.fills.iter_mut().chain(node.strokes.iter_mut()) {
                if let PaintKind::Solid { color: shade } = &mut paint.kind {
                    *shade = color;
                }
            }
        }
    }
}

pub fn create(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "icon.create";
    let supplied_library = req_string(CMD, p, "library")?;
    let library = icon_catalog::family_id(&supplied_library).ok_or_else(|| bad(CMD, format!("unknown icon library `{supplied_library}`")))?;
    let supplied_icon = req_string(CMD, p, "icon")?;
    let icon = supplied_icon.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    let svg = icon_catalog::svg(library, &icon)
        .map_err(|e| bad(CMD, e))?
        .ok_or_else(|| bad(CMD, format!("`{icon}` is not in `{library}`; call search_icons to find an exact name or another library")))?;
    let color = icon_color(p)?;
    let page = s.page()?;
    let parent = opt_id(CMD, p, "parent")?.unwrap_or(page);
    let doc = s.doc()?;
    let parent_node = doc.get(parent)?;
    if !parent_node.kind.is_container() {
        return Err(bad(CMD, format!("{parent} can't contain layers")));
    }
    let space = if parent_node.kind.bounds_follow_children() { doc.frame_relative(parent) } else { Affine::IDENTITY };
    let x = num_or(p, "x", 0.0);
    let y = num_or(p, "y", 0.0);
    if !x.is_finite() || !y.is_finite() {
        return Err(bad(CMD, "`x` and `y` must be finite"));
    }
    let name = string(p, "name").unwrap_or_else(|| icon.clone());
    let ds = s.ds_mut()?;
    let doc = ds.doc_mut();
    let id = lizard_svg::import_svg(doc, svg.as_bytes(), parent, x, y, &name).map_err(|e| bad(CMD, e.to_string()))?;
    let (original_w, original_h, children) = {
        let node = doc.get(id)?;
        (node.width, node.height, node.children.clone())
    };
    if original_w <= 0.0 || original_h <= 0.0 {
        return Err(bad(CMD, "the icon has no usable size"));
    }
    let asked_w = num(p, "width");
    let asked_h = num(p, "height");
    let (width, height) = match (asked_w, asked_h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w * original_h / original_w),
        (None, Some(h)) => (h * original_w / original_h, h),
        (None, None) => (24.0, 24.0),
    };
    if !width.is_finite()
        || !height.is_finite()
        || !(0.0..=8192.0).contains(&width)
        || !(0.0..=8192.0).contains(&height)
        || width == 0.0
        || height == 0.0
    {
        return Err(bad(CMD, "icon width and height must be finite and between 0 and 8192"));
    }
    let scale = Affine::scale_non_uniform(width / original_w, height / original_h);
    for child in children {
        if let Some(node) = doc.node_mut(child) {
            node.transform = scale * node.transform;
        }
    }
    if let Some(node) = doc.node_mut(id) {
        node.width = width;
        node.height = height;
        node.transform = space.inverse() * Affine::translate((x, y));
        node.plugin_data.insert(LIBRARY_KEY.into(), library.into());
        node.plugin_data.insert(NAME_KEY.into(), icon.clone());
        node.plugin_data.insert(COLOR_KEY.into(), color.to_css_hex());
    }
    recolor(doc, id, color);
    if let Some(Value::Object(props)) = get(p, "props") {
        let mut props = props.clone();
        props.remove("fill");
        props.remove("fills");
        super::props::apply(CMD, doc, id, &props)?;
    }
    if boolean(p, "select").unwrap_or(true) {
        ds.selection = vec![id];
    }
    Ok(json!({"id": id, "library": library, "icon": icon}))
}

pub fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "icon.set";
    let id = req_id(CMD, p, "id")?;
    let (current_library, current_icon, current_color, old_width, old_height, old_children) = {
        let node = s.doc()?.get(id)?;
        let library = node.plugin_data.get(LIBRARY_KEY).ok_or_else(|| bad(CMD, format!("{id} is not a catalog icon")))?.clone();
        let icon = node.plugin_data.get(NAME_KEY).ok_or_else(|| bad(CMD, format!("{id} has no icon name")))?.clone();
        let color = node.plugin_data.get(COLOR_KEY).and_then(|value| Color::from_hex(value)).unwrap_or(Color::BLACK);
        (library, icon, color, node.width, node.height, node.children.clone())
    };
    let library = match string(p, "library") {
        Some(value) => icon_catalog::family_id(&value).ok_or_else(|| bad(CMD, format!("unknown icon library `{value}`")))?.to_string(),
        None => current_library.clone(),
    };
    let icon = string(p, "icon").map_or_else(|| current_icon.clone(), |name| name.trim().to_ascii_lowercase().replace(['_', ' '], "-"));
    let new_svg = if library != current_library || icon != current_icon {
        Some(
            icon_catalog::svg(&library, &icon)
                .map_err(|e| bad(CMD, e))?
                .ok_or_else(|| bad(CMD, format!("`{icon}` is not in `{library}`; call search_icons first")))?,
        )
    } else {
        None
    };
    let color = match get(p, "color") {
        Some(value) => color_of(value).ok_or_else(|| bad(CMD, "`color` must be a #RGB, #RRGGBB or #RRGGBBAA colour"))?,
        None => current_color,
    };
    let width = num(p, "width").unwrap_or(old_width);
    let height = num(p, "height").unwrap_or(old_height);
    if !width.is_finite()
        || !height.is_finite()
        || !(0.0..=8192.0).contains(&width)
        || !(0.0..=8192.0).contains(&height)
        || width == 0.0
        || height == 0.0
    {
        return Err(bad(CMD, "icon width and height must be finite and between 0 and 8192"));
    }
    let doc = s.ds_mut()?.doc_mut();
    if let Some(svg) = new_svg {
        let artwork = lizard_svg::import_svg(doc, svg.as_bytes(), id, 0.0, 0.0, "Icon artwork").map_err(|e| bad(CMD, e.to_string()))?;
        let (source_w, source_h) = {
            let node = doc.get(artwork)?;
            (node.width, node.height)
        };
        if source_w <= 0.0 || source_h <= 0.0 {
            return Err(bad(CMD, "the replacement icon has no usable size"));
        }
        for child in old_children {
            doc.remove(child)?;
        }
        doc.get_mut(artwork)?.transform = Affine::scale_non_uniform(width / source_w, height / source_h);
    } else if (width - old_width).abs() > f64::EPSILON || (height - old_height).abs() > f64::EPSILON {
        for child in old_children {
            if let Some(node) = doc.node_mut(child) {
                node.transform = Affine::scale_non_uniform(width / old_width, height / old_height) * node.transform;
            }
        }
    }
    if let Some(node) = doc.node_mut(id) {
        node.width = width;
        node.height = height;
        node.plugin_data.insert(LIBRARY_KEY.into(), library.clone());
        node.plugin_data.insert(NAME_KEY.into(), icon.clone());
        node.plugin_data.insert(COLOR_KEY.into(), color.to_css_hex());
    }
    if new_svg.is_some() || get(p, "color").is_some() {
        recolor(doc, id, color);
    }
    Ok(json!({"id": id, "library": library, "icon": icon, "width": width, "height": height}))
}

/// Most-used icon library inside one subtree, with a deterministic tie-breaker. The agent
/// harness uses this to retain a screen's visual language across turns.
pub fn dominant_library(doc: &Document, root: NodeId, max_nodes: usize) -> Option<&'static str> {
    let mut counts = [0usize; 6];
    let mut pending = vec![root];
    let mut seen = 0usize;
    while let Some(id) = pending.pop() {
        if seen >= max_nodes {
            break;
        }
        seen += 1;
        let Some(node) = doc.node(id) else { continue };
        if let Some(family) = node.plugin_data.get(LIBRARY_KEY).and_then(|name| icon_catalog::family_id(name))
            && let Some(index) = icon_catalog::FAMILIES.iter().position(|item| item == &family)
        {
            counts[index] += 1;
        }
        pending.extend(node.children.iter().copied());
    }
    let most = counts.iter().copied().max().unwrap_or(0);
    (most > 0).then(|| icon_catalog::FAMILIES[counts.iter().position(|count| *count == most).unwrap_or(0)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use lizard_doc::NodeKind;

    #[test]
    fn icon_creation_is_editable_and_keeps_provenance() -> Result<()> {
        let mut session = Session::new();
        let created = session.execute("icon.create", &json!({"library":"lucide","icon":"home","x":11,"y":15,"width":32,"color":"#DC2626"}))?;
        let id = crate::params::id(&created, "id").ok_or_else(|| bad("test", "missing id"))?;
        let doc = session.doc()?;
        let node = doc.get(id)?;
        assert!(matches!(node.kind, NodeKind::Frame));
        assert_eq!((node.width, node.height), (32.0, 32.0));
        assert!(!node.children.is_empty());
        assert_eq!(node.plugin_data.get(LIBRARY_KEY).map(String::as_str), Some("lucide"));
        assert_eq!(node.plugin_data.get(NAME_KEY).map(String::as_str), Some("home"));
        assert_eq!(dominant_library(doc, id, 100), Some("lucide"));
        assert!(doc.node(node.children[0]).is_some());
        Ok(())
    }

    #[test]
    fn missing_name_does_not_change_document() -> Result<()> {
        let mut session = Session::new();
        let page = session.page()?;
        let before = session.doc()?.get(page)?.children.len();
        assert!(session.execute("icon.create", &json!({"library":"feather","icon":"not-a-real-icon"})).is_err());
        assert_eq!(session.doc()?.get(page)?.children.len(), before);
        Ok(())
    }

    #[test]
    fn every_library_creates_document_vectors() -> Result<()> {
        let mut session = Session::new();
        for library in icon_catalog::FAMILIES {
            let icon = if library == "phosphor" { "house" } else { "home" };
            let created = session.execute("icon.create", &json!({"library":library, "icon":icon, "color":"#336699", "select":false}))?;
            let id = crate::params::id(&created, "id").ok_or_else(|| bad("test", "missing id"))?;
            let node = session.doc()?.get(id)?;
            assert!(!node.children.is_empty(), "{library}");
            assert_eq!(node.plugin_data.get(LIBRARY_KEY).map(String::as_str), Some(library));
        }
        Ok(())
    }

    #[test]
    fn swap_and_recolor_preserve_layer_id_and_undo() -> Result<()> {
        let mut session = Session::new();
        let created = session.execute("icon.create", &json!({"library":"lucide", "icon":"home", "color":"#000000"}))?;
        let id = crate::params::id(&created, "id").ok_or_else(|| bad("test", "missing id"))?;
        session.execute("icon.set", &json!({"id":id, "icon":"search", "color":"#2563EB", "width":40}))?;
        let changed = session.doc()?.get(id)?;
        assert_eq!(changed.width, 40.0);
        assert_eq!(changed.plugin_data.get(NAME_KEY).map(String::as_str), Some("search"));
        assert_eq!(changed.plugin_data.get(COLOR_KEY).map(String::as_str), Some("#2563EB"));
        session.execute("edit.undo", &json!({}))?;
        let reverted = session.doc()?.get(id)?;
        assert_eq!(reverted.width, 24.0);
        assert_eq!(reverted.plugin_data.get(NAME_KEY).map(String::as_str), Some("home"));
        Ok(())
    }
}
