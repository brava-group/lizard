//! An app started with automation roots: its frames and control requests read and write only
//! inside them, and captures come back as data without a path.

use std::path::PathBuf;
use std::sync::Arc;

use lizard_engine::Session;
use lizard_engine::file_access::Roots;
use serde_json::{Value, json};

use crate::{LizardApp, NoServices};

struct Dirs(PathBuf);

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn confined_app(name: &str) -> (LizardApp, Dirs) {
    let d = std::env::temp_dir().join(format!("lizard-ui-roots-{name}-{}", std::process::id()));
    for sub in ["out", "outside"] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
    }
    let mut s = Session::new();
    s.execute("shape.rectangle", &json!({"x": 0, "y": 0, "width": 20, "height": 10})).unwrap();
    let mut app = LizardApp::new(s, Box::new(NoServices));
    app.automation_roots = Some(Arc::new(Roots::new(&[], &[d.join("out").to_string_lossy().into_owned()]).unwrap()));
    (app, Dirs(d))
}

#[test]
fn orchestrator_feedback_is_delivered_only_to_its_assigned_worker() {
    let (mut app, _dirs) = confined_app("review-routing");
    let document = app.session.ds().unwrap().uid;
    app.ui.agent.review_feedback = vec![(1, "Fix section A".into()), (2, "Fix section B".into())];
    let first = handle(&mut app, "agent.review.take", json!({"__agent_document":document,"__agent_index":1})).unwrap();
    assert_eq!(first, json!(["Fix section A"]));
    assert_eq!(app.ui.agent.review_feedback, [(2, "Fix section B".into())]);
    app.ui.agent.integration_started = true;
    let lead = handle(&mut app, "agent.review.take", json!({"__agent_document":document,"__agent_index":0})).unwrap();
    assert_eq!(lead, json!(["Fix section B"]));
}

/// Control method `method` handled as the desktop app does: inside the frame guard.
fn handle(app: &mut LizardApp, method: &str, p: Value) -> Result<Value, String> {
    let ctx = egui::Context::default();
    let mut res = Err("not run".to_string());
    crate::windows::guard(app, &ctx, |app| res = super::handle(app, &ctx, method, &p));
    res
}

#[test]
fn a_confined_app_writes_only_inside_its_write_root() {
    let (mut app, d) = confined_app("write");
    let p = |rel: &str| d.0.join(rel).to_string_lossy().into_owned();
    let e = handle(&mut app, "ui.render", json!({"path": p("outside/r.png")})).unwrap_err();
    assert!(e.contains("automation path rejected"), "{e}");
    let e =
        handle(&mut app, "engine.execute", json!({"command": "file.export", "params": {"format": "svg", "path": p("outside/r.svg")}})).unwrap_err();
    assert!(e.contains("automation path rejected"), "{e}");
    assert!(handle(&mut app, "ui.render", json!({"path": p("out/r.png")})).is_ok());
    // Without a path the PNG comes back as data.
    let r = handle(&mut app, "ui.render", json!({})).unwrap();
    assert!(r["data"].as_str().is_some_and(|d| !d.is_empty()), "{r}");
    assert_eq!(std::fs::read_dir(d.0.join("outside")).unwrap().count(), 0);
    assert!(d.0.join("out/r.png").is_file());
    // Outside the guard (the app's own work) nothing is confined.
    assert!(app.session.execute("file.export", &json!({"format": "svg", "path": p("outside/own.svg")})).is_ok());
}

#[test]
fn a_confined_app_saves_only_inside_its_write_root() {
    // #836: saving over the control channel, both as a new file and to the current one.
    let (mut app, d) = confined_app("save");
    let p = |rel: &str| d.0.join(rel).to_string_lossy().into_owned();
    for cmd in ["file.saveAs", "file.save"] {
        let e = handle(&mut app, "engine.execute", json!({"command": cmd, "params": {"path": p("outside/a.lizard")}})).unwrap_err();
        assert!(e.contains("automation path rejected"), "{cmd}: {e}");
        let e = handle(&mut app, "engine.execute", json!({"command": cmd, "params": {"path": p("out/../outside/b.lizard")}})).unwrap_err();
        assert!(e.contains("automation path rejected"), "{cmd}: {e}");
    }
    assert_eq!(std::fs::read_dir(d.0.join("outside")).unwrap().count(), 0, "nothing written outside the root");
    handle(&mut app, "engine.execute", json!({"command": "file.saveAs", "params": {"path": p("out/in.lizard")}})).unwrap();
    assert!(d.0.join("out/in.lizard").is_file());
    // Saving again goes to the same file, inside the root.
    handle(&mut app, "engine.execute", json!({"command": "file.save", "params": {}})).unwrap();
}

#[test]
fn window_captures_are_written_through_the_roots_or_returned() {
    let (app, d) = confined_app("capture");
    let img = egui::ColorImage::filled([3, 2], egui::Color32::RED);
    let roots = app.automation_roots.clone();
    let out = d.0.join("out/shot.png").to_string_lossy().into_owned();
    let outside = d.0.join("outside/shot.png").to_string_lossy().into_owned();
    lizard_engine::file_access::confined(roots.as_ref(), || {
        assert!(super::shot_reply(&img, Some(&outside)).unwrap_err().contains("automation path rejected"));
        assert_eq!(super::shot_reply(&img, Some(&out)).unwrap()["width"], 3);
        let data = super::shot_reply(&img, None).unwrap();
        assert_eq!((data["format"].as_str(), data["height"].as_u64()), (Some("png"), Some(2)));
    });
    assert!(d.0.join("out/shot.png").is_file() && !d.0.join("outside/shot.png").exists());
}

#[test]
fn parallel_spawn_requires_distinct_existing_frames() {
    let (mut app, _dirs) = confined_app("parallel");
    let ds = app.session.ds().unwrap();
    let document = ds.uid;
    app.ui.agent.busy = true;
    app.ui.agent.runtime_document = Some(document);
    app.ui.agent.active_agent_count = 3;
    app.ui.agent.active_parallel_mode = crate::agent_panel::ParallelMode::SideBySide;
    let frame = app.session.execute("node.create", &json!({"type":"frame", "name":"First task"})).unwrap()["id"].clone();
    let request = |second: Value| {
        json!({
            "__agent_document": document,
            "__agent_index": 0,
            "config": [
                {"prompt":"Build the first screen", "containerNodes":[frame]},
                {"prompt":"Build the second screen", "containerNodes":[second]}
            ]
        })
    };
    let duplicate = handle(&mut app, "agent.spawn", request(frame.clone())).unwrap_err();
    assert!(duplicate.contains("non-overlapping frame"), "{duplicate}");
    let missing = handle(&mut app, "agent.spawn", request(json!(999_999))).unwrap_err();
    assert!(missing.contains("does not exist"), "{missing}");
    let second = app.session.execute("node.create", &json!({"type":"frame", "name":"Second task"})).unwrap()["id"].clone();
    let valid = handle(&mut app, "agent.spawn", request(second)).unwrap_err();
    assert!(valid.contains("desktop app"), "{valid}");
}

#[test]
fn parallel_spawn_accepts_sibling_sections_but_rejects_ancestor_overlap() {
    let (mut app, _dirs) = confined_app("parallel-sections");
    let document = app.session.ds().unwrap().uid;
    app.ui.agent.busy = true;
    app.ui.agent.runtime_document = Some(document);
    app.ui.agent.active_agent_count = 3;
    let screen = app.session.execute("node.create", &json!({"type":"frame", "name":"Screen"})).unwrap()["id"].clone();
    let first = app.session.execute("node.create", &json!({"type":"frame", "name":"Header", "parent":screen})).unwrap()["id"].clone();
    let second = app.session.execute("node.create", &json!({"type":"frame", "name":"Body", "parent":screen})).unwrap()["id"].clone();
    let nested = app.session.execute("node.create", &json!({"type":"frame", "name":"Nested", "parent":first})).unwrap()["id"].clone();
    let request = |left: Value, right: Value| {
        json!({
            "__agent_document": document,
            "__agent_index": 0,
            "config": [
                {"prompt":"Build the header", "containerNodes":[left]},
                {"prompt":"Build the body", "containerNodes":[right]}
            ]
        })
    };
    let root = handle(&mut app, "agent.spawn", request(screen, first.clone())).unwrap_err();
    assert!(root.contains("section frames"), "{root}");
    let overlap = handle(&mut app, "agent.spawn", request(first.clone(), nested)).unwrap_err();
    assert!(overlap.contains("non-overlapping frame"), "{overlap}");
    let siblings = handle(&mut app, "agent.spawn", request(first, second)).unwrap_err();
    assert!(siblings.contains("desktop app"), "{siblings}");
}

#[test]
fn parallel_activity_keeps_each_cursor_at_its_own_work() {
    let (mut app, _dirs) = confined_app("cursors");
    let document = app.session.ds().unwrap().uid;
    app.ui.agent.busy = true;
    app.ui.agent.runtime_document = Some(document);
    app.ui.agent.runtime_conversation = Some(42);
    for (index, x) in [(0, 100), (1, 400)] {
        handle(
            &mut app,
            "agent.activity",
            json!({
                "__agent_document":document,
                "__agent_index":index,
                "phase":"start",
                "tool":"create_node",
                "position":{"x":x,"y":20}
            }),
        )
        .unwrap();
    }
    assert_eq!(app.ui.agent.presence.point, Some((100.0, 20.0)));
    assert_eq!(app.ui.agent.other_presences[0].point, Some((400.0, 20.0)));
}

#[test]
fn parallel_runs_use_atomic_batches_to_preserve_other_agents_edits() {
    let (mut app, _dirs) = confined_app("parallel-batch");
    let document = app.session.ds().unwrap().uid;
    app.ui.agent.busy = true;
    app.ui.agent.runtime_document = Some(document);
    app.ui.agent.active_agent_count = 2;
    let response = handle(
        &mut app,
        "agent.batch.begin",
        json!({"__agent_document":document,"__agent_index":1,"commands":[{"command":"node.create","params":{"type":"frame"}}]}),
    )
    .unwrap();
    assert_eq!(response["accepted"], false);
    assert!(app.ui.streamed_batch.is_none());
}

#[test]
fn collaborator_cannot_create_a_second_screen_or_leave_its_section() {
    let (mut app, _dirs) = confined_app("parallel-scope");
    let document = app.session.ds().unwrap().uid;
    let screen = app.session.execute("node.create", &json!({"type":"frame","name":"Screen"})).unwrap()["id"].clone();
    let section = app.session.execute("node.create", &json!({"type":"frame","name":"Section","parent":screen})).unwrap()["id"].clone();
    app.ui.agent.busy = true;
    app.ui.agent.runtime_document = Some(document);
    app.ui.agent.active_agent_count = 2;
    app.ui.agent.active_parallel_mode = crate::agent_panel::ParallelMode::Split;
    app.ui.agent.parallel_assignments = vec![Vec::new(), vec![lizard_engine::params::id_of(&section).unwrap()]];
    let create = |parent: Option<Value>| {
        let mut params = json!({"type":"rectangle"});
        if let Some(parent) = parent {
            params["parent"] = parent;
        }
        json!({"__agent_document":document,"__agent_index":1,"command":"node.create","params":params})
    };
    assert!(handle(&mut app, "engine.execute", create(None)).is_err());
    assert!(handle(&mut app, "engine.execute", create(Some(screen.clone()))).is_err());
    assert!(handle(&mut app, "engine.execute", create(Some(section.clone()))).is_ok());
    let outside = json!({"__agent_document":document,"__agent_index":1,"command":"node.set","params":{"ids":[screen],"props":{"name":"Wrong"}}});
    assert!(handle(&mut app, "engine.execute", outside).is_err());
    let batch = json!({
        "__agent_document":document,"__agent_index":1,"command":"command.batch",
        "params":{"commands":[
            {"as":"card","command":"node.create","params":{"type":"frame","parent":section}},
            {"command":"node.create","params":{"type":"text","parent":{"$ref":"card"},"text":"Inside"}}
        ]}
    });
    assert!(handle(&mut app, "engine.execute", batch).is_ok());
}
