//! The vector edit context menu exposes topology selection on a nested vector.

use egui::Pos2;
use figcraft_doc::{NodeKind, Segment, Vertex, WindingRule};
use figcraft_engine::Session;
use serde_json::json;

use crate::layers_drag_tests::Ui;
use crate::live_tests::{flat, has_text, text_center, texts};
use crate::present_tests::id;

#[test]
fn context_menu_selects_outer_and_inner_contours() {
    let mut session = Session::new();
    let vector = id(&session
        .execute(
            "vector.create",
            &json!({
                "points": [[0, 0], [200, 0], [200, 200], [0, 200]], "closed": true
            }),
        )
        .unwrap());
    let doc = session.doc_mut().unwrap();
    let node = doc.get_mut(vector).unwrap();
    let NodeKind::Vector { network } = &mut node.kind else { panic!("expected vector") };
    for (x, y) in [(60.0, 60.0), (140.0, 60.0), (140.0, 140.0), (60.0, 140.0)] {
        network.vertices.push(Vertex::new(x, y));
    }
    for i in 0..4 {
        network.segments.push(Segment::line(4 + i, 4 + (i + 1) % 4));
    }
    network.regions[0].loops.push(vec![4, 5, 6, 7]);
    network.regions[0].winding = WindingRule::EvenOdd;
    figcraft_engine::vectoredit::refit(doc, vector).unwrap();
    session.execute("vector.edit.begin", &json!({"id": vector})).unwrap();
    let mut ui = Ui::new(session);
    assert!(ui.app.session.ds().unwrap().vector_edit.is_some());
    ui.frame(vec![]);

    ui.app.ui.context_menu = Some((Pos2::new(700.0, 300.0), vec![vector]));
    ui.frame(vec![]);
    let shapes = flat(&ui.frame_output(vec![]).shapes);
    assert!(
        has_text(&shapes, "Select outer contour"),
        "menu open: {:?}, labels: {:?}",
        ui.app.ui.context_menu,
        texts(&shapes).iter().map(|row| &row.0).collect::<Vec<_>>()
    );
    assert!(has_text(&shapes, "Select inner contours"));
    let at = text_center(&shapes, "Select inner contours");
    ui.button(at, true);
    ui.button(at, false);
    let selected = ui.app.session.execute("vector.edit.state", &json!({})).unwrap();
    assert_eq!(selected["overlay"]["selectedSegments"], json!([4, 5, 6, 7]));
    assert!(ui.app.ui.last_error.is_none(), "{:?}", ui.app.ui.last_error);
}
