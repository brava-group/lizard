//! Shader definition and fill-instance commands. All edits participate in document history.

use figcraft_color::{BlendMode, Color, Paint, PaintKind, ShaderFill};
use figcraft_doc::{ShaderDefinition, ShaderParameterControl};
use serde_json::{Value, json};

use super::{CommandSpec, ok};
use crate::params::{num, req_index, req_string, req_targets};
use crate::{Result, Session, bad, spec};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!("shader.register", "Register WGSL fill", "{definition: ShaderDefinition}", true, register),
        spec!("shader.apply", "Apply shader fill", "{ids?, shaderId}", true, apply),
        spec!("shader.demo", "Apply wave shader", "{ids?}", true, demo),
        spec!("shader.setParameter", "Set shader parameter", "{ids?, index?, parameter, value}", true, set_parameter),
    ]
}

fn register(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "shader.register";
    let definition: ShaderDefinition = serde_json::from_value(p.get("definition").cloned().ok_or_else(|| bad(CMD, "definition is required"))?)
        .map_err(|e| bad(CMD, e.to_string()))?;
    figcraft_shader::validate(&definition).map_err(|e| bad(CMD, e))?;
    let id = definition.id.clone();
    let doc = s.doc_mut()?;
    doc.shaders.insert(id.clone(), definition);
    let users: Vec<_> = doc
        .nodes()
        .filter(|node| node.fills.iter().any(|paint| matches!(&paint.kind, PaintKind::Shader(fill) if fill.shader_id == id)))
        .map(|node| node.id)
        .collect();
    doc.mark_dirty(users);
    Ok(json!({"shaderId": id}))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "shader.apply";
    let ids = req_targets(CMD, s, p)?;
    let shader_id = req_string(CMD, p, "shaderId")?;
    let definition = s.doc()?.shaders.get(&shader_id).cloned().ok_or_else(|| bad(CMD, "unknown shader id"))?;
    figcraft_shader::validate(&definition).map_err(|e| bad(CMD, e))?;
    let values = definition.parameters.iter().flat_map(|parameter| parameter.default_entries()).collect();
    let paint = Paint { kind: PaintKind::Shader(ShaderFill { shader_id, values }), opacity: 1.0, visible: true, blend: BlendMode::Normal };
    for id in ids {
        let node = s.doc_mut()?.get_mut(id)?;
        if node.is_page() {
            return Err(bad(CMD, "a page cannot have a shader fill"));
        }
        node.fills.push(paint.clone());
        node.styles.fill = None;
    }
    ok()
}

fn demo(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "shader.demo";
    let ids = req_targets(CMD, s, p)?;
    let definition = figcraft_shader::demo();
    figcraft_shader::validate(&definition).map_err(|e| bad(CMD, e))?;
    let values = definition.parameters.iter().flat_map(|parameter| parameter.default_entries()).collect();
    let paint = Paint {
        kind: PaintKind::Shader(ShaderFill { shader_id: definition.id.clone(), values }),
        opacity: 1.0,
        visible: true,
        blend: BlendMode::Normal,
    };
    let doc = s.doc_mut()?;
    for id in &ids {
        if doc.node(*id).is_none_or(|node| node.is_page()) {
            return Err(bad(CMD, "select a shape or text layer"));
        }
    }
    doc.shaders.entry(definition.id.clone()).or_insert(definition);
    for id in ids {
        let node = doc.get_mut(id)?;
        node.fills.push(paint.clone());
        node.styles.fill = None;
    }
    ok()
}

fn set_parameter(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "shader.setParameter";
    let ids = req_targets(CMD, s, p)?;
    let parameter = req_string(CMD, p, "parameter")?;
    let raw_value = p.get("value").ok_or_else(|| bad(CMD, "value is required"))?;
    let index = p.get("index").map(|_| req_index(CMD, p, "index")).transpose()?;
    let updates: Result<Vec<_>> = ids
        .iter()
        .map(|id| {
            let node = s.doc()?.node(*id).ok_or_else(|| bad(CMD, "layer is missing"))?;
            let at = index.unwrap_or(node.fills.len().saturating_sub(1));
            let fill = node
                .fills
                .get(at)
                .and_then(|paint| match &paint.kind {
                    PaintKind::Shader(fill) => Some(fill),
                    _ => None,
                })
                .ok_or_else(|| bad(CMD, "fill is not a shader"))?;
            let definition = s.doc()?.shaders.get(&fill.shader_id).ok_or_else(|| bad(CMD, "shader definition is missing"))?;
            let spec = definition.parameters.iter().find(|spec| spec.id == parameter).ok_or_else(|| bad(CMD, "unknown parameter"))?;
            let entries = if spec.control == ShaderParameterControl::Color {
                let color: Color = serde_json::from_value(raw_value.clone()).map_err(|_| bad(CMD, "value must be a color"))?;
                let mut values = std::collections::BTreeMap::new();
                spec.set_color(&mut values, color);
                values.into_iter().collect::<Vec<_>>()
            } else {
                let value = num(p, "value").ok_or_else(|| bad(CMD, "value must be finite"))? as f32;
                let value = spec.scalar_value(value).ok_or_else(|| bad(CMD, "value is invalid for this control"))?;
                vec![(parameter.clone(), value)]
            };
            Ok((*id, at, entries))
        })
        .collect();
    for (id, at, entries) in updates? {
        let node = s.doc_mut()?.get_mut(id)?;
        if let Some(Paint { kind: PaintKind::Shader(fill), .. }) = node.fills.get_mut(at) {
            fill.values.extend(entries);
        }
    }
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_parameter_survives_undo_and_file_roundtrip() {
        let mut session = Session::new();
        let id = session.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 80, "height": 60})).unwrap()["id"].as_u64().unwrap();
        session.execute("shader.demo", &json!({"ids": [id]})).unwrap();
        session.execute("shader.setParameter", &json!({"ids": [id], "parameter": "frequency", "value": 7.0})).unwrap();
        let node = session.doc().unwrap().node(figcraft_doc::NodeId(id)).unwrap();
        assert!(matches!(&node.fills.last().unwrap().kind, PaintKind::Shader(fill) if fill.values.get("frequency") == Some(&7.0)));
        let bytes = crate::cmd::file::file_bytes(session.ds().unwrap()).unwrap();
        let (loaded, _) = figcraft_format::load_bytes(&bytes).unwrap();
        assert!(loaded.shaders.contains_key(figcraft_shader::DEMO_ID));
        assert!(
            matches!(&loaded.node(figcraft_doc::NodeId(id)).unwrap().fills.last().unwrap().kind, PaintKind::Shader(fill) if fill.values.get("frequency") == Some(&7.0))
        );
        session.execute("edit.undo", &json!({})).unwrap();
        let node = session.doc().unwrap().node(figcraft_doc::NodeId(id)).unwrap();
        assert!(matches!(&node.fills.last().unwrap().kind, PaintKind::Shader(fill) if fill.values.get("frequency") == Some(&2.0)));
    }

    #[test]
    fn invalid_definition_never_enters_document() {
        let mut session = Session::new();
        let mut definition = figcraft_shader::demo();
        definition.id = "broken".into();
        definition.source = "not WGSL".into();
        assert!(session.execute("shader.register", &json!({"definition": definition})).is_err());
        assert!(!session.doc().unwrap().shaders.contains_key("broken"));
    }

    #[test]
    fn typed_shader_values_survive_save_and_undo() {
        let mut session = Session::new();
        let aurora = figcraft_shader::presets().into_iter().find(|definition| definition.id == "lizard.aurora");
        assert!(aurora.is_some());
        let Some(aurora) = aurora else { return };
        session.execute("shader.register", &json!({"definition": aurora})).unwrap();
        let id = session.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 80, "height": 60})).unwrap()["id"].as_u64().unwrap();
        session.execute("shader.apply", &json!({"ids": [id], "shaderId": "lizard.aurora"})).unwrap();
        session.execute("shader.setParameter", &json!({"ids": [id], "parameter": "color1", "value": "#FF8800"})).unwrap();
        session.execute("shader.setParameter", &json!({"ids": [id], "parameter": "paletteDirection", "value": 1})).unwrap();
        assert!(session.execute("shader.setParameter", &json!({"ids": [id], "parameter": "paletteDirection", "value": 0.5})).is_err());
        let bytes = crate::cmd::file::file_bytes(session.ds().unwrap()).unwrap();
        let (loaded, _) = figcraft_format::load_bytes(&bytes).unwrap();
        let paint = loaded.node(figcraft_doc::NodeId(id)).and_then(|node| node.fills.last());
        assert!(
            matches!(paint.map(|paint| &paint.kind), Some(PaintKind::Shader(fill)) if fill.values.get("color1.r") == Some(&1.0) && fill.values.get("paletteDirection") == Some(&1.0))
        );
        session.execute("edit.undo", &json!({})).unwrap();
        let paint = session.doc().unwrap().node(figcraft_doc::NodeId(id)).and_then(|node| node.fills.last());
        assert!(matches!(paint.map(|paint| &paint.kind), Some(PaintKind::Shader(fill)) if fill.values.get("paletteDirection") == Some(&0.0)));
    }
}
