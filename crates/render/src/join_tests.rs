//! Outside rectangle strokes preserve the selected corner join.

use lizard_color::{Color, Paint};
use lizard_doc::{Document, NodeKind, StrokeAlign, StrokeJoin};
use lizard_geom::kurbo::{Point, Shape};

fn winding_at(align: StrokeAlign, join: StrokeJoin, at: Point) -> Result<i32, Box<dyn std::error::Error>> {
    let mut doc = Document::new("join");
    let page = doc.first_page().ok_or("missing page")?;
    let id = doc.create(page, NodeKind::Rectangle, "R", 0.0, 0.0, 60.0, 40.0)?;
    let node = doc.node_mut(id).ok_or("missing rectangle")?;
    node.strokes = vec![Paint::solid(Color::BLACK)];
    node.stroke.weight = 10.0;
    node.stroke.align = align;
    node.stroke.join = join;
    let rect = lizard_geom::kurbo::Rect::new(0.0, 0.0, 60.0, 40.0).to_path(0.1);
    let node = doc.node(id).ok_or("missing rectangle")?;
    Ok(crate::draw::stroke_area(node, &rect, true).winding(at))
}

#[test]
fn outside_joins_follow_the_requested_corner() -> Result<(), Box<dyn std::error::Error>> {
    let outer_corner = Point::new(-9.5, -9.5);
    assert_eq!(winding_at(StrokeAlign::Outside, StrokeJoin::Round, outer_corner)?, 0);
    assert_eq!(winding_at(StrokeAlign::Outside, StrokeJoin::Bevel, outer_corner)?, 0);
    assert_ne!(winding_at(StrokeAlign::Outside, StrokeJoin::Miter, outer_corner)?, 0);
    for join in [StrokeJoin::Round, StrokeJoin::Bevel, StrokeJoin::Miter] {
        assert_ne!(winding_at(StrokeAlign::Inside, join, Point::new(0.5, 0.5))?, 0);
    }
    Ok(())
}
