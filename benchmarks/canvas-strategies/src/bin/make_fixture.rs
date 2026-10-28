//! Build a reproducible, editable benchmark file from Lizard's own example documents.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use figcraft_color::{ImageScaleMode, Paint};
use figcraft_doc::proto::{Action, Easing, Flow, Interaction, Transition, Trigger};
use figcraft_doc::{Document, ImageData, Node, NodeId, NodeKind};
use figcraft_geom::Affine;

const OUTPUT: &str = "benchmarks/canvas-strategies/fixtures/ultrawide-workbook.figcraft";

fn add_source(dest: &mut Document, page: NodeId, path: &str, offset: (f64, f64)) -> Result<Vec<NodeId>, Box<dyn Error>> {
    let (source, warnings) = figcraft_format::load_file(Path::new(path))?;
    if !warnings.is_empty() {
        return Err(format!("{path}: {warnings:?}").into());
    }
    let source_page = source.current_page.ok_or("source has no page")?;
    let roots = source.get(source_page)?.children.clone();
    let mut map = BTreeMap::new();
    let mut trees = Vec::with_capacity(roots.len());
    for &root in &roots {
        let nodes: Vec<Node> = source.subtree(root).into_iter().filter_map(|id| source.node(id).cloned()).collect();
        for node in &nodes {
            map.insert(node.id, dest.alloc_id());
        }
        trees.push((root, nodes));
    }
    for (key, image) in &source.images {
        dest.images.entry(key.clone()).or_insert_with(|| image.clone());
    }
    let mut inserted = Vec::with_capacity(trees.len());
    for (root, nodes) in trees {
        let new_root = dest.paste_nodes(&nodes, &map, root, page, None)?;
        if let Some(node) = dest.node_mut(new_root) {
            node.transform = Affine::translate(offset) * node.transform;
        }
        inserted.push(new_root);
    }
    Ok(inserted)
}

fn image_cards(doc: &mut Document, page: NodeId) -> Result<(), Box<dyn Error>> {
    let bytes = std::fs::read("assets/app-icon/figcraft-512.png")?;
    let key = "benchmark-local-app-icon".to_string();
    doc.images.insert(key.clone(), ImageData { bytes: Arc::new(bytes), width: 512, height: 512 });
    for row in 0..3 {
        for col in 0..4 {
            let frame = doc.create(
                page,
                NodeKind::Frame,
                &format!("Asset gallery {}", row * 4 + col + 1),
                9000.0 + f64::from(col) * 900.0,
                160.0 + f64::from(row) * 720.0,
                760.0,
                600.0,
            )?;
            for card in 0..6 {
                let x = 26.0 + f64::from(card % 3) * 240.0;
                let y = 30.0 + f64::from(card / 3) * 275.0;
                let id = doc.create(frame, NodeKind::Rectangle, "Image card", x, y, 212.0, 212.0)?;
                if let Some(node) = doc.node_mut(id) {
                    node.fills = vec![Paint::image(key.clone(), ImageScaleMode::Fill)];
                    node.corners.radii = [18.0; 4];
                    node.opacity = 0.84 + (card as f32 % 3.0) * 0.08;
                }
            }
        }
    }
    Ok(())
}

fn connect_screens(doc: &mut Document, page: NodeId, roots: &[NodeId], name: &str) {
    let screens: Vec<NodeId> = roots
        .iter()
        .copied()
        .filter(|id| {
            doc.node(*id).is_some_and(|node| matches!(node.kind, NodeKind::Frame) && (300.0..=500.0).contains(&node.width) && node.height >= 700.0)
        })
        .collect();
    if let Some(&start) = screens.first()
        && let Some(page_node) = doc.node_mut(page)
        && let NodeKind::Page { flows, .. } = &mut page_node.kind
    {
        flows.push(Flow { name: name.into(), start, description: "Connected UI screens for the canvas benchmark".into() });
    }
    for pair in screens.windows(2) {
        if let [from, to] = pair
            && let Some(node) = doc.node_mut(*from)
        {
            node.interactions.push(Interaction {
                trigger: Trigger::OnClick,
                actions: vec![Action::Navigate { dest: *to, preserve_scroll: false }],
                transition: Transition::Instant,
                duration_ms: 300,
                easing: Easing::EaseOut,
                delay_ms: 0,
            });
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut doc = Document::new("Lizard ultrawide design workbook");
    let page = doc.current_page.ok_or("new document has no page")?;
    let app = add_source(&mut doc, page, "examples/ridgeline-app.figcraft", (150.0, 160.0))?;
    let onboarding = add_source(&mut doc, page, "examples/ridgeline-onboarding.figcraft", (150.0, 1350.0))?;
    let dashboard = add_source(&mut doc, page, "examples/ridgeline-studio-dashboard.figcraft", (1500.0, 160.0))?;
    let glass = add_source(&mut doc, page, "examples/glass-and-vectors.figcraft", (2600.0, 1420.0))?;
    let system = add_source(&mut doc, page, "examples/ridgeline-design-system.figcraft", (5000.0, 160.0))?;
    let icon = add_source(&mut doc, page, "assets/app-icon/Lizard-new-icon.figcraft", (1900.0, 2700.0))?;
    connect_screens(&mut doc, page, &app, "Trail app");
    connect_screens(&mut doc, page, &onboarding, "Trail onboarding");
    for row in 1..4 {
        for &root in app.iter().chain(onboarding.iter()).chain(dashboard.iter()).chain(glass.iter()).chain(icon.iter()) {
            let copy = doc.duplicate_into(root, page, None)?;
            if let Some(node) = doc.node_mut(copy) {
                node.transform = Affine::translate((0.0, f64::from(row) * 3500.0)) * node.transform;
                node.opacity = (1.0 - row as f32 * 0.04).max(0.8);
            }
        }
    }
    let _ = system;
    image_cards(&mut doc, page)?;
    let frames = doc.top_frames(page).len();
    let nodes = doc.node_count();
    let effects = doc.nodes().map(|n| n.effects.len()).sum::<usize>();
    let auto_layout = doc.nodes().filter(|n| n.layout.is_some()).count();
    let interactions = doc.nodes().map(|n| n.interactions.len()).sum::<usize>();
    let flows = doc
        .node(page)
        .and_then(|n| match &n.kind {
            NodeKind::Page { flows, .. } => Some(flows.len()),
            _ => None,
        })
        .unwrap_or(0);
    figcraft_format::save_file(&doc, Path::new(OUTPUT), None)?;
    println!(
        "fixture={OUTPUT} nodes={nodes} top_frames={frames} effects={effects} auto_layout={auto_layout} images={} interactions={interactions} flows={flows}",
        doc.images.len()
    );
    Ok(())
}
