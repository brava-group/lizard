//! Paint stacks owned by vector faces. An unset stack inherits the layer's fills.

use lizard_color::{Color, Paint, PaintKind};
use lizard_doc::NodeKind;
use serde_json::{Value, json};

use super::{editing, indices};
use crate::params::*;
use crate::{Result, Session, bad};

pub(super) fn run(session: &mut Session, params: &Value, action: &str) -> Result<Value> {
    let command = format!("vector.region.fill.{action}");
    let edit = editing(session)?;
    let id = id(params, "id").unwrap_or(edit.id);
    if !edit.edits(id) {
        return Err(bad(&command, "layer is not being edited"));
    }
    let mut regions = if get(params, "regions").is_some() {
        indices(params, "regions").ok_or_else(|| bad(&command, "invalid region indices"))?
    } else {
        edit.regions.clone()
    };
    regions.sort_unstable();
    regions.dedup();
    if regions.is_empty() {
        return Err(bad(&command, "select a filled region"));
    }
    let index = opt_index(&command, params, "index")?;
    let from_to = if action == "move" { Some((req_index(&command, params, "from")?, req_index(&command, params, "to")?)) } else { None };
    let replacement = get(params, "paint").map(|value| paint_of(value).ok_or_else(|| bad(&command, "invalid paint"))).transpose()?;
    let color = color(params, "color");
    let opacity = num(params, "opacity");
    let visible = boolean(params, "visible");
    let blend = blend(params, "blend");
    let node = session.doc_mut()?.get_mut(id)?;
    let inherited = node.fills.clone();
    let NodeKind::Vector { network } = &mut node.kind else { return Err(bad(&command, "layer is not a vector")) };
    if regions.iter().any(|region| *region >= network.regions.len()) {
        return Err(bad(&command, "region index out of range"));
    }
    for region in regions {
        let Some(region) = network.regions.get_mut(region) else { continue };
        let list = region.fills.get_or_insert_with(|| inherited.clone());
        match action {
            "add" => list.push(replacement.clone().unwrap_or_else(|| Paint::solid(Color::DEFAULT_SHAPE))),
            "remove" => {
                let at = index.unwrap_or(list.len().saturating_sub(1));
                if at >= list.len() {
                    return Err(bad(&command, "paint index out of range"));
                }
                list.remove(at);
            }
            "move" => {
                let Some((from, to)) = from_to else { continue };
                if from >= list.len() || to >= list.len() {
                    return Err(bad(&command, "paint index out of range"));
                }
                let paint = list.remove(from);
                list.insert(to, paint);
            }
            "set" => {
                if list.is_empty() {
                    list.push(Paint::solid(Color::DEFAULT_SHAPE));
                }
                let at = index.unwrap_or(list.len() - 1);
                let Some(paint) = list.get_mut(at) else { return Err(bad(&command, "paint index out of range")) };
                if let Some(next) = &replacement {
                    *paint = next.clone();
                }
                if let Some(value) = color {
                    paint.kind = PaintKind::Solid { color: value.with_alpha(1.0) };
                    if value.a < 1.0 {
                        paint.opacity = value.a;
                    }
                }
                if let Some(value) = opacity {
                    paint.opacity = (if value > 1.0 { value / 100.0 } else { value }).clamp(0.0, 1.0) as f32;
                }
                if let Some(value) = visible {
                    paint.visible = value;
                }
                if let Some(value) = blend {
                    paint.blend = value;
                }
            }
            _ => return Err(bad(&command, "unknown action")),
        }
    }
    Ok(json!({"ok": true}))
}
