use lizard_doc::{NodeId, NodeKind};
use serde_json::json;

use crate::Session;
use crate::components::tests::fill;

fn run(s: &mut Session, command: &str, params: serde_json::Value) -> serde_json::Value {
    s.execute(command, &params).unwrap_or_else(|error| panic!("{command}: {error}"))
}

fn id(v: &serde_json::Value) -> NodeId {
    NodeId(v["id"].as_u64().unwrap_or_default())
}

#[test]
fn one_main_resolves_size_style_and_state_without_drawn_combinations() {
    let mut s = Session::new();
    let frame = id(&run(&mut s, "frame.create", json!({"name":"Button", "width":120, "height":40})));
    let text = id(&run(&mut s, "text.create", json!({"parent":frame, "text":"Button", "x":4, "y":4})));
    let main = id(&run(&mut s, "component.create", json!({"ids":[frame]})));
    for (name, options) in
        [("Size", vec!["Small", "Medium", "Large"]), ("Style", vec!["Primary", "Secondary"]), ("State", vec!["Default", "Hover", "Pressed"])]
    {
        run(&mut s, "component.matrixSetAxis", json!({"id":main, "name":name, "options":options}));
    }
    for (size, width, font) in [("Small", 80, 12), ("Medium", 120, 14), ("Large", 160, 18)] {
        run(&mut s, "component.matrixSetCell", json!({"id":main, "layer":main, "field":"width", "when":{"Size":size}, "value":width}));
        run(&mut s, "component.matrixSetCell", json!({"id":main, "layer":text, "field":"fontSize", "when":{"Size":size}, "value":font}));
    }
    for (style, state, color) in [
        ("Primary", "Default", "#2244aa"),
        ("Primary", "Hover", "#3355bb"),
        ("Primary", "Pressed", "#113399"),
        ("Secondary", "Default", "#dddddd"),
        ("Secondary", "Hover", "#cccccc"),
        ("Secondary", "Pressed", "#bbbbbb"),
    ] {
        run(&mut s, "component.matrixSetCell", json!({"id":main, "layer":main, "field":"fill", "when":{"Style":style,"State":state}, "value":color}));
    }
    let first = id(&run(&mut s, "instance.create", json!({"component":main, "x":200, "y":0})));
    let second = id(&run(&mut s, "instance.create", json!({"component":main, "x":400, "y":0})));
    assert!(
        s.doc().unwrap().node(second).is_some_and(|n| n.overrides.is_empty()),
        "new instance overrides: {:?}",
        s.doc().unwrap().node(second).map(|n| &n.overrides)
    );
    for size in ["Small", "Medium", "Large"] {
        for style in ["Primary", "Secondary"] {
            for state in ["Default", "Hover", "Pressed"] {
                let instance = id(&run(&mut s, "instance.create", json!({"component":main})));
                run(&mut s, "instance.setProperties", json!({"ids":[instance], "properties":{"Size":size,"Style":style,"State":state}}));
                let described = run(&mut s, "component.matrixDescribe", json!({"id":instance}));
                assert_eq!(described["resolved"].as_array().map(Vec::len), Some(3));
                assert_eq!(described["choices"]["Size"], size);
                assert_eq!(described["choices"]["Style"], style);
                assert_eq!(described["choices"]["State"], state);
            }
        }
    }
    assert!(
        s.doc().unwrap().node(second).is_some_and(|n| n.overrides.is_empty()),
        "before selection overrides: {:?}",
        s.doc().unwrap().node(second).map(|n| &n.overrides)
    );
    run(&mut s, "instance.setProperties", json!({"ids":[first], "properties":{"Size":"Small", "Style":"Primary", "State":"Hover"}}));
    run(&mut s, "instance.setProperties", json!({"ids":[second], "properties":{"Size":"Large", "Style":"Secondary", "State":"Pressed"}}));
    let doc = s.doc().unwrap();
    assert_eq!(doc.node(first).map(|n| n.width), Some(80.0));
    assert_eq!(doc.node(second).map(|n| n.width), Some(160.0), "second: {:?}", doc.node(second));
    assert_eq!(fill(&s, first), "3355BB");
    assert_eq!(fill(&s, second), "BBBBBB");
    let first_text = doc.subtree(first).into_iter().find(|i| doc.node(*i).is_some_and(|n| n.source == Some(text))).unwrap();
    let second_text = doc.subtree(second).into_iter().find(|i| doc.node(*i).is_some_and(|n| n.source == Some(text))).unwrap();
    assert_eq!(doc.node(first_text).and_then(|n| n.kind.text()).map(|t| t.style.font_size), Some(12.0));
    assert_eq!(doc.node(second_text).and_then(|n| n.kind.text()).map(|t| t.style.font_size), Some(18.0));
    let described = run(&mut s, "component.matrixDescribe", json!({"id":first}));
    assert_eq!(described["choices"]["State"], "Hover");
    assert_eq!(described["resolved"].as_array().map(Vec::len), Some(3));
    let code = run(&mut s, "inspect.componentCode", json!({"id":first, "language":"react"}));
    assert!(code["definition"].as_str().is_some_and(|text| text.contains("data-size") && text.contains("data-style") && text.contains("data-state")));
    assert_eq!(s.doc().unwrap().nodes().filter(|n| matches!(n.kind, NodeKind::Component { .. })).count(), 1);
    let saved = lizard_format::save_bytes(s.doc().unwrap(), None).unwrap();
    let (restored, warnings) = lizard_format::load_bytes(&saved).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(restored.node(main).map(|n| n.component_matrix.axes.len()), Some(3));
    assert_eq!(restored.node(first).map(|n| n.width), Some(80.0));
    run(&mut s, "component.matrixSetCell", json!({"id":main, "layer":main, "field":"width", "when":{"Size":"Small"}, "value":88}));
    assert_eq!(s.doc().unwrap().node(first).map(|n| n.width), Some(88.0));
    assert_eq!(s.doc().unwrap().node(second).map(|n| n.width), Some(160.0));
    run(&mut s, "node.set", json!({"id":first, "fill":"#ff0000"}));
    run(
        &mut s,
        "component.matrixSetCell",
        json!({"id":main, "layer":main, "field":"fill", "when":{"Style":"Primary","State":"Hover"}, "value":"#4455cc"}),
    );
    assert_eq!(fill(&s, first), "FF0000", "the instance's own colour stays overridden");
    assert_eq!(fill(&s, second), "BBBBBB");
}

#[test]
fn ambiguous_cells_and_invalid_values_are_rejected() {
    let mut s = Session::new();
    let frame = id(&run(&mut s, "frame.create", json!({"name":"Button", "width":100, "height":30})));
    let main = id(&run(&mut s, "component.create", json!({"ids":[frame]})));
    for axis in ["Style", "State"] {
        run(&mut s, "component.matrixSetAxis", json!({"id":main, "name":axis, "options":["A","B"]}));
    }
    run(&mut s, "component.matrixSetCell", json!({"id":main, "layer":main, "field":"fill", "when":{"Style":"A"}, "value":"#111111"}));
    assert!(
        s.execute("component.matrixSetCell", &json!({"id":main, "layer":main, "field":"fill", "when":{"State":"A"}, "value":"#222222"})).is_err()
    );
    assert!(s.execute("component.matrixSetCell", &json!({"id":main, "layer":main, "field":"width", "when":{"Style":"A"}, "value":"wide"})).is_err());
    let instance = id(&run(&mut s, "instance.create", json!({"component":main})));
    assert!(s.execute("instance.setProperties", &json!({"ids":[instance], "properties":{"Style":"Missing"}})).is_err());
}

#[test]
fn renaming_axis_preserves_instance_choices_and_cells() {
    let mut s = Session::new();
    let frame = id(&run(&mut s, "frame.create", json!({"name":"Button", "width":100, "height":30})));
    let main = id(&run(&mut s, "component.create", json!({"ids":[frame]})));
    run(&mut s, "component.matrixSetAxis", json!({"id":main, "name":"Size", "options":["Small","Large"]}));
    run(&mut s, "component.matrixSetCell", json!({"id":main, "layer":main, "field":"width", "when":{"Size":"Large"}, "value":180}));
    let instance = id(&run(&mut s, "instance.create", json!({"component":main})));
    run(&mut s, "instance.setProperties", json!({"ids":[instance], "properties":{"Size":"Large"}}));
    run(&mut s, "component.matrixRenameAxis", json!({"id":main, "name":"Size", "newName":"Dimension"}));
    let described = run(&mut s, "component.matrixDescribe", json!({"id":instance}));
    assert_eq!(described["choices"]["Dimension"], "Large");
    assert!(described["choices"].get("Size").is_none());
    assert_eq!(described["fields"][0]["cells"][0]["when"]["Dimension"], "Large");
    assert_eq!(s.doc().unwrap().node(instance).map(|node| node.width), Some(180.0));
}
