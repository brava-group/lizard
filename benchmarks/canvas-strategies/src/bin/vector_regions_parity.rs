//! Compare Vello GPU and the document renderer for individually painted vector faces.

use std::error::Error;

use figcraft_canvas_gpu::vello_scene::{GpuCanvas, has_unhandled_effects_in_view};
use figcraft_color::{Color, Paint};
use figcraft_doc::{Document, NodeKind, Region, Segment, VectorNetwork, Vertex, WindingRule};
use figcraft_render::{Options, View, render_view};

fn add_square(network: &mut VectorNetwork, x: f64, fill: Option<Vec<Paint>>) {
    let first_vertex = network.vertices.len();
    for (dx, dy) in [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)] {
        network.vertices.push(Vertex::new(x + dx, dy));
    }
    let first_segment = network.segments.len();
    for index in 0..4 {
        network.segments.push(Segment::line(first_vertex + index, first_vertex + (index + 1) % 4));
    }
    network.regions.push(Region { loops: vec![(first_segment..first_segment + 4).collect()], winding: WindingRule::NonZero, fills: fill });
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut doc = Document::new("Vector face GPU parity");
    let page = doc.current_page.ok_or("new document has no page")?;
    let mut network = VectorNetwork::default();
    add_square(&mut network, 0.0, Some(vec![Paint::solid(Color::new(1.0, 0.0, 0.0, 1.0))]));
    add_square(&mut network, 40.0, Some(vec![Paint::solid(Color::new(0.0, 0.0, 1.0, 1.0))]));
    add_square(&mut network, 80.0, Some(Vec::new()));
    let id = doc.create(page, NodeKind::Vector { network }, "Three faces", 10.0, 10.0, 110.0, 30.0)?;
    doc.node_mut(id).ok_or("vector missing")?.fills = vec![Paint::solid(Color::new(0.0, 1.0, 0.0, 1.0))];
    let view = View { origin: (0.0, 0.0), scale: 2.0, width: 256, height: 100 };
    if has_unhandled_effects_in_view(&doc, &view) {
        return Err("supported vector face document was routed to fallback".into());
    }
    let reference = render_view(&doc, page, &view, &Options::default()).ok_or("CPU render failed")?;
    let mut gpu = GpuCanvas::new(view.width, view.height)?;
    let clear = match doc.node(page).map(|node| &node.kind) {
        Some(NodeKind::Page { background, background_visible: true, .. }) => [background.r, background.g, background.b],
        _ => [1.0; 3],
    };
    let background = clear.map(|channel| (channel * 255.0).round() as u8);
    gpu.render_submit_document(&doc, page, &view, clear)?;
    gpu.wait()?;
    let actual = gpu.pixels()?;
    let pixel = |bytes: &[u8], x: usize, y: usize| -> Option<[u8; 4]> {
        let index = (y * view.width as usize + x) * 4;
        Some(bytes.get(index..index + 4)?.try_into().ok()?)
    };
    for (x, expected) in [(50, [255, 0, 0]), (130, [0, 0, 255]), (210, background)] {
        let a = pixel(&actual, x, 50).ok_or("GPU pixel missing")?;
        let b = pixel(reference.data(), x, 50).ok_or("reference pixel missing")?;
        println!("x={x} GPU={a:?} CPU={b:?}");
        if a[..3].iter().zip(expected).any(|(channel, want)| channel.abs_diff(want) > 4)
            || a[..3].iter().zip(b[..3].iter()).any(|(channel, want)| channel.abs_diff(*want) > 4)
        {
            return Err(format!("painted face mismatch at x={x}").into());
        }
    }
    Ok(())
}
