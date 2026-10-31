//! Effect bounds include children and use page-space units under transforms.

use figcraft_doc::{Document, Effect, EffectKind, NodeKind};
use figcraft_geom::Affine;

#[test]
fn group_blur_reaches_past_its_child() -> Result<(), Box<dyn std::error::Error>> {
    let mut doc = Document::new("group blur");
    let page = doc.first_page().ok_or("missing page")?;
    let group = doc.create(page, NodeKind::Group, "G", 0.0, 0.0, 100.0, 100.0)?;
    doc.create(group, NodeKind::Rectangle, "R", 150.0, 0.0, 20.0, 20.0)?;
    let mut blur = Effect::layer_blur();
    if let EffectKind::LayerBlur(settings) = &mut blur.kind {
        settings.radius = 20.0;
    }
    doc.node_mut(group).ok_or("missing group")?.effects.push(blur);
    let bounds = doc.world_render_bounds(group).ok_or("missing group bounds")?;
    assert!(bounds.x1 >= 200.0, "group blur must cover the child past the group box: {bounds:?}");
    let local = super::draw::local_paint_bounds(&doc, doc.node(group).ok_or("missing group")?, Affine::IDENTITY, 0, None);
    assert!(local.x1 >= 200.0, "canvas layer must include the same paint: {local:?}");
    Ok(())
}

#[test]
fn scaled_shadow_keeps_its_page_space_offset() -> Result<(), Box<dyn std::error::Error>> {
    let mut doc = Document::new("scaled shadow");
    let page = doc.first_page().ok_or("missing page")?;
    let id = doc.create(page, NodeKind::Rectangle, "R", 0.0, 0.0, 100.0, 100.0)?;
    let node = doc.node_mut(id).ok_or("missing rectangle")?;
    node.transform = Affine::scale(0.5);
    let mut shadow = Effect::drop_shadow();
    if let EffectKind::DropShadow(settings) = &mut shadow.kind {
        settings.radius = 0.0;
        settings.spread = 0.0;
        settings.offset = (0.0, 80.0);
    }
    node.effects.push(shadow);
    let bounds = doc.world_render_bounds(id).ok_or("missing shadow bounds")?;
    assert!(bounds.y1 >= 130.0, "shadow offset remains 80 page units after halving: {bounds:?}");
    let local = super::draw::local_paint_bounds(&doc, doc.node(id).ok_or("missing rectangle")?, doc.world_transform(id), 0, None);
    let drawn = figcraft_geom::transform_rect_bbox(doc.world_transform(id), local);
    assert!(drawn.y1 >= 130.0, "canvas layer must not crop the shadow: {drawn:?}");
    Ok(())
}
