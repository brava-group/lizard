//! Count which saved-document shadows can use the measured analytic GPU path.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use canvas_strategies_bench::gpu_scene::gpu_drop_shadow_supported;
use lizard_doc::{EffectKind, NodeKind};

fn main() -> Result<(), Box<dyn Error>> {
    for path in ["benchmarks/canvas-strategies/fixtures/ultrawide-workbook.lizard", "/Users/matheuspires/Documents/Lizard-new-icon.lizard"] {
        let (doc, warnings) = lizard_format::load_file(Path::new(path))?;
        if !warnings.is_empty() {
            return Err(format!("{path}: {warnings:?}").into());
        }
        let mut counts = BTreeMap::new();
        for node in doc.nodes() {
            for effect in &node.effects {
                if let EffectKind::DropShadow(shadow) = &effect.kind {
                    let kind = match node.kind {
                        NodeKind::Rectangle => "rectangle",
                        NodeKind::Frame => "frame",
                        NodeKind::Instance { .. } => "instance",
                        NodeKind::Component { .. } => "component",
                        _ => "other",
                    };
                    *counts.entry((kind, gpu_drop_shadow_supported(&doc, node, shadow))).or_insert(0usize) += 1;
                }
            }
        }
        println!("document={path}");
        for ((kind, supported), count) in counts {
            println!("kind={kind} supported={supported} count={count}");
        }
    }
    Ok(())
}
