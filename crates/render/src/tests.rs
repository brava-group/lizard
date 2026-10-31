use super::*;
use lizard_color::{Color, ColorStop, Gradient, GradientKind, Paint};
use lizard_doc::{Effect, EffectKind, NodeKind, Shadow, StrokeAlign, TextData};

fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * p.width() + x) * 4) as usize;
    let d = &p.data()[i..i + 4];
    [d[0], d[1], d[2], d[3]]
}

fn doc_with_rect(fill: Color) -> (lizard_doc::Document, lizard_doc::NodeId, lizard_doc::NodeId) {
    let mut d = lizard_doc::Document::new("t");
    let p = d.first_page().unwrap();
    let r = d.create(p, NodeKind::Rectangle, "R", 10.0, 10.0, 50.0, 30.0).unwrap();
    d.get_mut(r).unwrap().fills = vec![Paint::solid(fill)];
    (d, p, r)
}

fn view(w: u32, h: u32) -> View {
    View { origin: (0.0, 0.0), scale: 1.0, width: w, height: h }
}

#[test]
fn solid_rect_and_page_background() {
    let (d, p, _) = doc_with_rect(Color::from_hex("FF0000").unwrap());
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 30, 20), [255, 0, 0, 255]);
    assert_eq!(px(&pix, 5, 5), [0xF5, 0xF5, 0xF5, 255]);
}

#[test]
fn vector_faces_render_their_own_fill_without_recoloring_the_layer() -> Result<(), &'static str> {
    use lizard_doc::{Region, Segment, VectorNetwork, Vertex, WindingRule};
    let mut network = VectorNetwork::default();
    for (x0, y0, size) in [(0.0, 0.0, 60.0), (20.0, 20.0, 20.0)] {
        let start = network.vertices.len();
        for (x, y) in [(x0, y0), (x0 + size, y0), (x0 + size, y0 + size), (x0, y0 + size)] {
            network.vertices.push(Vertex::new(x, y));
        }
        for i in 0..4 {
            network.segments.push(Segment::line(start + i, start + (i + 1) % 4));
        }
    }
    let blue = Color::from_hex("0066FF").ok_or("bad blue")?;
    let red = Color::from_hex("FF2200").ok_or("bad red")?;
    network.regions = vec![
        Region { loops: vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7]], winding: WindingRule::EvenOdd, fills: None },
        Region { loops: vec![vec![4, 5, 6, 7]], winding: WindingRule::NonZero, fills: Some(vec![Paint::solid(red)]) },
    ];
    let mut doc = lizard_doc::Document::new("region colors");
    let page = doc.first_page().ok_or("missing page")?;
    let id = doc.create(page, NodeKind::Vector { network }, "Vector", 10.0, 10.0, 60.0, 60.0).map_err(|_| "missing vector")?;
    doc.get_mut(id).map_err(|_| "missing node")?.fills = vec![Paint::solid(blue)];
    let pix = render_view(&doc, page, &view(80, 80), &Options::default()).ok_or("render failed")?;
    assert_eq!(px(&pix, 20, 20), [0, 102, 255, 255]);
    assert_eq!(px(&pix, 40, 40), [255, 34, 0, 255]);
    Ok(())
}

#[test]
fn painted_face_with_unassigned_edges_renders_the_whole_face() -> Result<(), &'static str> {
    use lizard_doc::{Segment, VectorNetwork, Vertex};
    let mut network = VectorNetwork::default();
    let points = [(260.0, 100.0), (260.0, 180.0), (0.0, 180.0), (0.0, 0.0), (100.0, 100.0), (180.0, 180.0), (0.0, 100.0)];
    let edges = [(0, 1), (1, 5), (2, 6), (3, 4), (4, 0), (4, 5), (5, 2), (6, 4), (6, 3)];
    network.vertices = points.into_iter().map(|(x, y)| Vertex::new(x, y)).collect();
    network.segments = edges.into_iter().map(|(a, b)| Segment::line(a, b)).collect();
    assert_eq!(network.paint(lizard_geom::Point::new(40.0, 150.0)), Some(true));
    let mut doc = lizard_doc::Document::new("face plus open edges");
    let page = doc.first_page().ok_or("missing page")?;
    let id = doc.create(page, NodeKind::Vector { network }, "Vector", 0.0, 0.0, 260.0, 180.0).map_err(|_| "missing vector")?;
    doc.get_mut(id).map_err(|_| "missing node")?.fills = vec![Paint::solid(Color::from_hex("F63434").ok_or("invalid test color")?)];
    let pix = render_view(&doc, page, &view(280, 200), &Options::default()).ok_or("render failed")?;
    assert_eq!(px(&pix, 40, 150), [0xF6, 0x34, 0x34, 255], "lower part of the face");
    assert_eq!(px(&pix, 100, 120), [0xF6, 0x34, 0x34, 255], "upper triangle of the same face");
    let node = doc.get_mut(id).map_err(|_| "missing node")?;
    node.fills[0].opacity = 0.5;
    let translucent = render_view(&doc, page, &view(280, 200), &Options::default()).ok_or("render failed")?;
    assert_eq!(px(&translucent, 40, 150), px(&translucent, 100, 120), "one face must have even opacity");
    let preview =
        render_view(&doc, page, &View { scale: 0.5, ..view(140, 100) }, &Options { lod: true, ..Options::default() }).ok_or("preview failed")?;
    assert_eq!(px(&preview, 20, 75), px(&preview, 50, 60), "zoomed-out preview keeps the whole face painted");
    let node = doc.get_mut(id).map_err(|_| "missing node")?;
    node.fills[0].opacity = 1.0;
    let NodeKind::Vector { network } = &mut node.kind else { return Err("missing vector") };
    network.materialize_faces();
    let top = network.region_at(lizard_geom::Point::new(20.0, 60.0)).ok_or("missing top face")?;
    network.regions[top].fills = Some(Vec::new());
    let without_top = render_view(&doc, page, &view(280, 200), &Options::default()).ok_or("render failed")?;
    assert_eq!(px(&without_top, 20, 60), [0xF5, 0xF5, 0xF5, 255], "Delete removes only the selected fill");
    assert_eq!(px(&without_top, 40, 150), [0xF6, 0x34, 0x34, 255], "middle face remains filled");
    assert_eq!(px(&without_top, 220, 150), [0xF6, 0x34, 0x34, 255], "right face remains filled");
    Ok(())
}

#[test]
fn curve_handle_y_changes_the_cached_outline_key() {
    use std::hash::Hasher;
    let mut first = lizard_geom::BezPath::new();
    first.move_to((0.0, 0.0));
    first.curve_to((10.0, 20.0), (30.0, 40.0), (50.0, 60.0));
    let mut second = lizard_geom::BezPath::new();
    second.move_to((0.0, 0.0));
    second.curve_to((10.0, 20.0), (30.0, 41.0), (50.0, 60.0));
    let mut a = std::collections::hash_map::DefaultHasher::new();
    let mut b = std::collections::hash_map::DefaultHasher::new();
    crate::draw::hash_path(&first, &mut a);
    crate::draw::hash_path(&second, &mut b);
    assert_ne!(a.finish(), b.finish());
}

#[test]
fn outside_stroke_of_nested_painted_faces_does_not_cross_the_inner_face() -> Result<(), &'static str> {
    use lizard_doc::{Region, Segment, VectorNetwork, Vertex, WindingRule};
    let mut network = VectorNetwork::default();
    for (x0, y0, size) in [(0.0, 0.0, 60.0), (20.0, 20.0, 20.0)] {
        let first_vertex = network.vertices.len();
        for (x, y) in [(x0, y0), (x0 + size, y0), (x0 + size, y0 + size), (x0, y0 + size)] {
            network.vertices.push(Vertex::new(x, y));
        }
        for index in 0..4 {
            network.segments.push(Segment::line(first_vertex + index, first_vertex + (index + 1) % 4));
        }
    }
    let black = Color::BLACK;
    let red = Color::from_hex("FF0000").ok_or("bad red")?;
    network.regions = vec![
        Region { loops: vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7]], winding: WindingRule::EvenOdd, fills: None },
        Region { loops: vec![vec![4, 5, 6, 7]], winding: WindingRule::NonZero, fills: Some(vec![Paint::solid(black)]) },
    ];
    let mut doc = lizard_doc::Document::new("outside stroke");
    let page = doc.first_page().ok_or("missing page")?;
    let id = doc.create(page, NodeKind::Vector { network }, "Vector", 10.0, 10.0, 60.0, 60.0).map_err(|_| "missing vector")?;
    let node = doc.get_mut(id).map_err(|_| "missing node")?;
    node.fills = vec![Paint::solid(Color::WHITE)];
    node.strokes = vec![Paint::solid(red)];
    node.stroke.weight = 4.0;
    node.stroke.align = StrokeAlign::Outside;
    let pix = render_view(&doc, page, &view(80, 80), &Options::default()).ok_or("render failed")?;
    assert_eq!(px(&pix, 8, 40), [255, 0, 0, 255], "outside edge");
    assert_eq!(px(&pix, 27, 40), [255, 255, 255, 255], "inner partition must not get the layer stroke");
    assert_eq!(px(&pix, 32, 40), [0, 0, 0, 255], "outside stroke must not cover the inner painted face");
    assert_eq!(px(&pix, 40, 40), [0, 0, 0, 255], "inner face keeps its own fill");
    let mut with_hole = doc.clone();
    if let NodeKind::Vector { network } = &mut with_hole.get_mut(id).map_err(|_| "missing vector")?.kind {
        network.regions.pop();
    }
    let hole_pix = render_view(&with_hole, page, &view(80, 80), &Options::default()).ok_or("hole render failed")?;
    assert_eq!(px(&hole_pix, 32, 40), [255, 0, 0, 255], "an actual hole still has an outside edge");
    Ok(())
}

#[test]
fn open_vector_with_fill_paints_area_without_closing_stroke() -> Result<(), &'static str> {
    use lizard_doc::VectorNetwork;
    use lizard_geom::Point;
    let mut doc = lizard_doc::Document::new("open vector fill");
    let page = doc.first_page().ok_or("missing page")?;
    let network = VectorNetwork::from_points(&[Point::new(0.0, 0.0), Point::new(30.0, 0.0), Point::new(30.0, 30.0), Point::new(0.0, 30.0)], false);
    let id = doc.create(page, NodeKind::Vector { network }, "Open", 10.0, 10.0, 30.0, 30.0).map_err(|_| "missing vector")?;
    let node = doc.get_mut(id).map_err(|_| "missing vector node")?;
    node.fills = vec![Paint::solid(Color::from_hex("F63434").ok_or("invalid test color")?)];
    let pix = render_view(&doc, page, &view(60, 60), &Options::default()).ok_or("render failed")?;
    assert_eq!(px(&pix, 25, 25), [0xF6, 0x34, 0x34, 255]);
    assert_eq!(lizard_doc::geometry::hit_leaf(&doc, page, Point::new(25.0, 25.0), 1.0, None), Some(id));
    let Some(NodeKind::Vector { network }) = doc.node(id).map(|node| &node.kind) else { return Err("missing vector network") };
    assert!(network.regions.is_empty());
    assert!(!matches!(network.stroke_path().elements().last(), Some(lizard_geom::kurbo::PathEl::ClosePath)));
    Ok(())
}

#[test]
fn mixed_closed_and_open_vector_subpaths_both_paint() -> Result<(), &'static str> {
    let mut path = lizard_geom::BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((10.0, 0.0));
    path.line_to((10.0, 10.0));
    path.line_to((0.0, 10.0));
    path.close_path();
    path.move_to((20.0, 0.0));
    path.line_to((30.0, 0.0));
    path.line_to((30.0, 10.0));
    path.line_to((20.0, 10.0));
    let network = lizard_doc::VectorNetwork::from_path(&path);
    let mut doc = lizard_doc::Document::new("mixed vector fill");
    let page = doc.first_page().ok_or("missing page")?;
    let id = doc.create(page, NodeKind::Vector { network }, "Mixed", 10.0, 10.0, 30.0, 10.0).map_err(|_| "missing vector")?;
    doc.get_mut(id).map_err(|_| "missing vector node")?.fills = vec![Paint::solid(Color::from_hex("F63434").ok_or("invalid test color")?)];
    let pix = render_view(&doc, page, &view(60, 30), &Options::default()).ok_or("render failed")?;
    assert_eq!(px(&pix, 15, 15), [0xF6, 0x34, 0x34, 255]);
    assert_eq!(px(&pix, 35, 15), [0xF6, 0x34, 0x34, 255]);
    assert_eq!(px(&pix, 25, 15), [0xF5, 0xF5, 0xF5, 255]);
    Ok(())
}

#[test]
fn animated_shader_fill_changes_canvas_at_explicit_time() {
    use lizard_color::{PaintKind, ShaderFill};
    let (mut doc, page, rectangle) = doc_with_rect(Color::BLACK);
    let Some(aurora) = lizard_shader::presets().into_iter().find(|shader| shader.id == "lizard.aurora") else { panic!("Aurora preset missing") };
    let shader_id = aurora.id.clone();
    doc.shaders.insert(shader_id.clone(), aurora);
    doc.get_mut(rectangle).unwrap().fills =
        vec![Paint { kind: PaintKind::Shader(ShaderFill { shader_id, values: Default::default() }), ..Paint::solid(Color::WHITE) }];
    let view = view(100, 100);
    let first = render_view(&doc, page, &view, &Options { time: 0.0, ..Default::default() }).unwrap();
    let second = render_view(&doc, page, &view, &Options { time: 1.0, ..Default::default() }).unwrap();
    assert_ne!(px(&first, 30, 20), px(&second, 30, 20));
    assert_eq!(px(&first, 5, 5), px(&second, 5, 5));
}

#[test]
fn hidden_page_background_renders_transparent_without_losing_its_color() {
    let (mut d, p, _) = doc_with_rect(Color::from_hex("FF0000").unwrap());
    if let NodeKind::Page { background_visible, .. } = &mut d.get_mut(p).unwrap().kind {
        *background_visible = false;
    }
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 5, 5), [0, 0, 0, 0]);
    assert_eq!(px(&pix, 30, 20), [255, 0, 0, 255]);
    let with_override = render_view(&d, p, &view(100, 100), &Options { background: Some(Color::WHITE), ..Default::default() }).unwrap();
    assert_eq!(px(&with_override, 5, 5), [255, 255, 255, 255]);
}

#[test]
fn zoom_scales() {
    let (d, p, _) = doc_with_rect(Color::BLACK);
    let v = View { origin: (0.0, 0.0), scale: 2.0, width: 200, height: 200 };
    let pix = render_view(&d, p, &v, &Options::default()).unwrap();
    assert_eq!(px(&pix, 100, 60)[3], 255);
    assert_eq!(px(&pix, 118, 78), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 125, 60)[0], 0xF5);
}

#[test]
fn opacity_and_layer() {
    let (mut d, p, r) = doc_with_rect(Color::BLACK);
    d.get_mut(r).unwrap().opacity = 0.5;
    let pix = render_view(&d, p, &view(100, 100), &Options { background: Some(Color::WHITE), ..Default::default() }).unwrap();
    let v = px(&pix, 30, 20)[0];
    assert!((120..=135).contains(&v), "{v}");
}

#[test]
fn drop_shadow_paints_outside() {
    let (mut d, p, r) = doc_with_rect(Color::WHITE);
    d.get_mut(r).unwrap().effects.push(Effect {
        kind: EffectKind::DropShadow(Shadow { color: Color::BLACK, offset: (0.0, 10.0), radius: 0.0, ..Default::default() }),
        visible: true,
    });
    let pix = render_view(&d, p, &view(100, 100), &Options { background: Some(Color::WHITE), ..Default::default() }).unwrap();
    // Below the rect (y 40..50) is shadow.
    assert!(px(&pix, 30, 45)[0] < 30, "{:?}", px(&pix, 30, 45));
    // The rect itself stays white.
    assert_eq!(px(&pix, 30, 20)[0], 255);
}

#[test]
fn inner_shadow_and_blur_dont_crash() {
    let (mut d, p, r) = doc_with_rect(Color::WHITE);
    let n = d.get_mut(r).unwrap();
    n.effects.push(Effect::inner_shadow());
    n.effects.push(Effect::layer_blur());
    n.effects.push(Effect::background_blur());
    n.effects.push(Effect {
        kind: EffectKind::Noise(lizard_doc::Noise { size: 1.0, density: 0.5, color: Color::BLACK, ..Default::default() }),
        visible: true,
    });
    let _ = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
}

#[test]
fn gradients_render() {
    for kind in [GradientKind::Linear, GradientKind::Radial, GradientKind::Angular, GradientKind::Diamond] {
        let (mut d, p, r) = doc_with_rect(Color::BLACK);
        let g = Gradient {
            kind,
            stops: vec![
                ColorStop { position: 0.0, color: Color::from_hex("FF0000").unwrap() },
                ColorStop { position: 1.0, color: Color::from_hex("0000FF").unwrap() },
            ],
            from: (0.0, 0.5),
            to: (1.0, 0.5),
            width: (0.0, 1.0),
        };
        let g = if kind == GradientKind::Linear { g } else { Gradient { from: (0.5, 0.5), to: (1.0, 0.5), width: (0.5, 1.0), ..g } };
        d.get_mut(r).unwrap().fills = vec![Paint::gradient(g)];
        let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
        let left = px(&pix, if kind == GradientKind::Linear { 11 } else { 35 }, 25);
        let right = px(&pix, 58, 25);
        assert_ne!(left, right, "{kind:?}");
    }
}

#[test]
fn frame_clips_children() {
    let mut d = lizard_doc::Document::new("t");
    let p = d.first_page().unwrap();
    let f = d.create(p, NodeKind::Frame, "F", 0.0, 0.0, 50.0, 50.0).unwrap();
    let r = d.create(f, NodeKind::Rectangle, "R", 25.0, 25.0, 50.0, 50.0).unwrap();
    d.get_mut(r).unwrap().fills = vec![Paint::solid(Color::BLACK)];
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 40, 40), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 60, 60)[0], 0xF5);
    d.get_mut(f).unwrap().clip = false;
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 60, 60), [0, 0, 0, 255]);
}

#[test]
fn strokes_align() {
    let (mut d, p, r) = doc_with_rect(Color::WHITE);
    let n = d.get_mut(r).unwrap();
    n.strokes = vec![Paint::solid(Color::BLACK)];
    n.stroke.weight = 4.0;
    n.stroke.align = StrokeAlign::Outside;
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 30, 8), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 30, 12), [255, 255, 255, 255]);
    d.get_mut(r).unwrap().stroke.align = StrokeAlign::Inside;
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 30, 12), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 30, 8)[0], 0xF5);
}

#[test]
fn mask_clips_siblings() {
    let mut d = lizard_doc::Document::new("t");
    let p = d.first_page().unwrap();
    let g = d.create(p, NodeKind::Group, "G", 0.0, 0.0, 100.0, 100.0).unwrap();
    let m = d.create(g, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "M", 0.0, 0.0, 50.0, 50.0).unwrap();
    d.get_mut(m).unwrap().is_mask = true;
    let r = d.create(g, NodeKind::Rectangle, "R", 0.0, 0.0, 100.0, 100.0).unwrap();
    d.get_mut(r).unwrap().fills = vec![Paint::solid(Color::BLACK)];
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 25, 25), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 2, 2)[0], 0xF5);
    assert_eq!(px(&pix, 80, 80)[0], 0xF5);
}

#[test]
fn page_level_mask_clips_only_the_following_run() -> Result<(), Box<dyn std::error::Error>> {
    let mut doc = lizard_doc::Document::new("page mask");
    let page = doc.first_page().ok_or("new document has no page")?;
    let mask = doc.create(page, NodeKind::Rectangle, "Mask", 0.0, 0.0, 50.0, 100.0)?;
    doc.get_mut(mask)?.is_mask = true;
    let content = doc.create(page, NodeKind::Rectangle, "Content", 0.0, 0.0, 100.0, 100.0)?;
    doc.get_mut(content)?.fills = vec![Paint::solid(Color::BLACK)];
    let pix = render_view(&doc, page, &view(100, 100), &Options::default()).ok_or("render failed")?;
    assert_eq!(px(&pix, 25, 50), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 75, 50)[0], 0xF5);
    Ok(())
}

#[test]
fn luminance_and_vector_masks() {
    let mut d = lizard_doc::Document::new("t");
    let p = d.first_page().unwrap();
    let g = d.create(p, NodeKind::Group, "G", 0.0, 0.0, 100.0, 100.0).unwrap();
    // The mask: white on the left half, black on the right.
    let m = d.create(g, NodeKind::Rectangle, "M", 0.0, 0.0, 100.0, 100.0).unwrap();
    let grad = lizard_color::Gradient {
        kind: lizard_color::GradientKind::Linear,
        stops: vec![
            lizard_color::ColorStop { position: 0.0, color: Color::WHITE },
            lizard_color::ColorStop { position: 0.49, color: Color::WHITE },
            lizard_color::ColorStop { position: 0.51, color: Color::BLACK },
            lizard_color::ColorStop { position: 1.0, color: Color::BLACK },
        ],
        from: (0.0, 0.5),
        to: (1.0, 0.5),
        width: (0.0, 1.0),
    };
    d.get_mut(m).unwrap().fills = vec![Paint::gradient(grad)];
    d.get_mut(m).unwrap().is_mask = true;
    let r = d.create(g, NodeKind::Rectangle, "R", 0.0, 0.0, 100.0, 100.0).unwrap();
    d.get_mut(r).unwrap().fills = vec![Paint::solid(Color::BLACK)];
    // Alpha: the mask is opaque everywhere, so everything shows.
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 80, 50), [0, 0, 0, 255]);
    // Luminance: only the white half shows.
    d.get_mut(m).unwrap().mask_type = lizard_doc::MaskType::Luminance;
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 20, 50), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 80, 50)[0], 0xF5, "black hides");
    // Vector: a fully transparent ellipse still masks by its outline.
    let e = d.create(g, NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "E", 0.0, 0.0, 50.0, 50.0).unwrap();
    d.get_mut(e).unwrap().fills = vec![Paint::solid(Color::new(0.0, 0.0, 0.0, 0.0))];
    d.get_mut(e).unwrap().is_mask = true;
    d.get_mut(e).unwrap().mask_type = lizard_doc::MaskType::Vector;
    d.get_mut(g).unwrap().children = vec![e, r];
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 25, 25), [0, 0, 0, 255], "inside the outline");
    assert_eq!(px(&pix, 80, 80)[0], 0xF5, "outside");
    // As an alpha mask the transparent ellipse hides everything.
    d.get_mut(e).unwrap().mask_type = lizard_doc::MaskType::Alpha;
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 25, 25)[0], 0xF5);
}

#[test]
fn boolean_subtract_renders_hole() {
    let mut d = lizard_doc::Document::new("t");
    let p = d.first_page().unwrap();
    let b = d.create(p, NodeKind::BooleanOp { op: lizard_doc::BoolOp::Subtract }, "B", 0.0, 0.0, 100.0, 100.0).unwrap();
    d.get_mut(b).unwrap().fills = vec![Paint::solid(Color::BLACK)];
    d.create(b, NodeKind::Rectangle, "A", 0.0, 0.0, 100.0, 100.0).unwrap();
    d.create(b, NodeKind::Rectangle, "Hole", 25.0, 25.0, 50.0, 50.0).unwrap();
    let pix = render_view(&d, p, &view(100, 100), &Options::default()).unwrap();
    assert_eq!(px(&pix, 10, 10), [0, 0, 0, 255]);
    assert_eq!(px(&pix, 50, 50)[0], 0xF5);
}

#[test]
fn text_renders_something() {
    let mut d = lizard_doc::Document::new("t");
    let p = d.first_page().unwrap();
    let mut td = TextData::new("Hello");
    td.style.font_size = 32.0;
    d.create(p, NodeKind::Text(td), "T", 0.0, 0.0, 200.0, 50.0).unwrap();
    let pix = render_view(&d, p, &view(200, 60), &Options { background: Some(Color::WHITE), ..Default::default() }).unwrap();
    let dark = pix.data().as_chunks::<4>().0.iter().filter(|c| c[0] < 100).count();
    if lizard_text::find_face("Inter", "Regular").is_some() {
        assert!(dark > 50, "{dark}");
    }
}

/// The machine's colour emoji face, if it has one (this test skips without: no font ships here).
fn emoji_face() -> Option<(String, String)> {
    lizard_text::scan_system_fonts();
    let mut db = lizard_text::db().write().unwrap();
    db.emoji_fallbacks().into_iter().find(|f| f.has_color()).map(|f| (f.family.clone(), f.style.clone()))
}

#[test]
fn colour_emoji_draw_their_pixels() {
    let Some((family, style)) = emoji_face() else { return };
    let mut d = lizard_doc::Document::new("t");
    let p = d.first_page().unwrap();
    let mut td = TextData::new("\u{1F600}");
    (td.style.font_family, td.style.font_style) = (family.clone(), style.clone());
    td.style.font_size = 48.0;
    d.create(p, NodeKind::Text(td), "T", 0.0, 0.0, 200.0, 64.0).unwrap();
    let pix = render_view(&d, p, &view(100, 80), &Options { background: Some(Color::WHITE), ..Default::default() }).unwrap();
    // The emoji is the only thing in this document that paints saturated colour.
    let coloured = pix
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|c| {
            let (r, g, b) = (i16::from(c[0]), i16::from(c[1]), i16::from(c[2]));
            c[3] == 255 && (r.max(g).max(b) - r.min(g).min(b)) > 60
        })
        .count();
    assert!(coloured > 200, "{family} drew {coloured} coloured pixels");
}

#[test]
fn export_png_roundtrip() {
    let (d, _, r) = doc_with_rect(Color::from_hex("00FF00").unwrap());
    let pix = render_node(&d, r, 2.0).unwrap();
    assert_eq!((pix.width(), pix.height()), (100, 60));
    let png = encode_png(&pix).unwrap();
    let back = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(back.get_pixel(50, 30).0, [0, 255, 0, 255]);
    let _ = encode_jpeg(&pix, Color::WHITE, 90).unwrap();
}

#[test]
fn exports_carry_the_colour_profile() {
    use image::ImageDecoder;
    use lizard_color::ColorProfile;
    use lizard_doc::ExportFormat;
    let (d, _, r) = doc_with_rect(Color::from_hex("00FF00").unwrap());
    let pix = render_node(&d, r, 1.0).unwrap();
    for profile in ColorProfile::ALL {
        let png = encode_export(&pix, ExportFormat::Png, profile).unwrap();
        // The inserted chunk has a valid CRC: the decoder accepts the file and keeps the pixels.
        let mut dec = image::codecs::png::PngDecoder::new(std::io::Cursor::new(&png)).unwrap();
        let icc = dec.icc_profile().unwrap();
        assert_eq!(icc.is_some(), !profile.is_srgb(), "{profile:?}");
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8().get_pixel(5, 5).0, [0, 255, 0, 255]);
        for f in [ExportFormat::Jpg, ExportFormat::Webp] {
            assert!(!encode_export(&pix, f, profile).unwrap().is_empty());
        }
        assert!(encode_export(&pix, ExportFormat::Svg, profile).is_err());
    }
    assert_eq!(png_insert_chunk(b"not a png".to_vec(), b"sRGB", &[0]), b"not a png");
}

#[test]
fn images_are_managed_into_the_file_profile() {
    use lizard_color::{ColorProfile, ImageScaleMode, Paint};
    let (mut d, _, r) = doc_with_rect(Color::WHITE);
    let mut red = Pixmap::new(4, 4).unwrap();
    red.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
    d.images.insert("red".into(), lizard_doc::ImageData { bytes: std::sync::Arc::new(encode_png(&red).unwrap()), width: 4, height: 4 });
    d.get_mut(r).unwrap().fills = vec![Paint::image("red", ImageScaleMode::Fill)];
    let center = |d: &Document| {
        let pix = render_node(d, r, 1.0).unwrap();
        let (x, y) = (pix.width() / 2, pix.height() / 2);
        let px = pix.pixel(x, y).unwrap();
        (pix, [px.red(), px.green(), px.blue()])
    };
    assert_eq!(center(&d).1, [255, 0, 0]);
    // In a Display P3 file the (sRGB) image is converted into P3 values…
    d.color_profile = ColorProfile::DisplayP3;
    let (mut pix, rgb) = center(&d);
    assert!(rgb[0].abs_diff(234) <= 1 && rgb[1].abs_diff(51) <= 1 && rgb[2].abs_diff(35) <= 1, "{rgb:?}");
    // …and comes back to the screen unchanged.
    to_srgb(&mut pix, d.color_profile);
    let px = pix.pixel(pix.width() / 2, pix.height() / 2).unwrap();
    assert!(px.red() >= 254 && px.green() <= 1 && px.blue() <= 1, "{px:?}");
}

#[test]
fn hostile_sizes() {
    let (mut d, p, r) = doc_with_rect(Color::BLACK);
    let n = d.get_mut(r).unwrap();
    n.width = f64::NAN;
    n.height = 1e12;
    n.effects.push(Effect { kind: EffectKind::LayerBlur(lizard_doc::Blur::uniform(1e9)), visible: true });
    let _ = render_view(&d, p, &view(64, 64), &Options::default());
    let _ = render_node(&d, r, f64::NAN);
    let _ = render_view(&d, p, &View { origin: (f64::NAN, 0.0), scale: 0.0, width: 0, height: 0 }, &Options::default());
}

#[test]
fn inner_shadow_reaches_every_edge() {
    let (mut d, p, r) = doc_with_rect(Color::WHITE);
    d.get_mut(r).unwrap().effects.push(Effect {
        kind: EffectKind::InnerShadow(Shadow { color: Color::BLACK, offset: (0.0, 4.0), radius: 8.0, ..Default::default() }),
        visible: true,
    });
    let pix = render_view(&d, p, &view(100, 100), &Options { background: Some(Color::WHITE), ..Default::default() }).unwrap();
    // Left edge (no offset that way) is still shaded; the centre is not.
    assert!(px(&pix, 11, 25)[0] < 235, "{:?}", px(&pix, 11, 25));
    assert!(px(&pix, 35, 30)[0] > 245);
}

#[test]
fn rect_clip_frame_matches_same_rect_fill_on_fractional_edges() {
    let white = Options { background: Some(Color::WHITE), ..Default::default() };
    for (x, y, w, h) in [(0.5, 0.5, 10.0, 10.0), (0.0, 0.0, 10.5, 10.5), (0.0, 0.0, 10.0, 10.0)] {
        let mut clipped = lizard_doc::Document::new("clipped");
        let p = clipped.first_page().unwrap();
        let f = clipped.create(p, NodeKind::Frame, "F", x, y, w, h).unwrap();
        clipped.get_mut(f).unwrap().fills.clear();
        clipped.get_mut(f).unwrap().clip = true;
        let c = clipped.create(f, NodeKind::Rectangle, "C", -5.0, -5.0, w + 10.0, h + 10.0).unwrap();
        clipped.get_mut(c).unwrap().fills = vec![Paint::solid(Color::BLACK)];

        let mut direct = lizard_doc::Document::new("direct");
        let dp = direct.first_page().unwrap();
        let r = direct.create(dp, NodeKind::Rectangle, "R", x, y, w, h).unwrap();
        direct.get_mut(r).unwrap().fills = vec![Paint::solid(Color::BLACK)];

        let a = render_view(&clipped, p, &view(12, 12), &white).unwrap();
        let e = render_view(&direct, dp, &view(12, 12), &white).unwrap();
        for (px_x, px_y) in [(0, 0), (0, 5), (5, 0), (5, 5), (10, 5), (5, 10), (10, 10), (11, 11)] {
            assert_eq!(px(&a, px_x, px_y), px(&e, px_x, px_y), "frame {x},{y} {w}x{h} at ({px_x},{px_y})");
        }
    }
}
