//! Project tasks: `cargo xtask <task>`.
//!
//! - `ci`      fmt check, clippy -D warnings, tests, layers, assets, wasm check
//! - `layers`  crate dependency layering check
//! - `assets`  required bundled asset licences and notices are present
//! - `parity`  menu-catalog coverage → docs/parity-checklist.md
//! - `version` print the version, or `version set X.Y.Z`
//! - `icon`    regenerate the app icon from the navigation logo (needs resvg)
//! - `wasm`    check the core crates and web app for wasm32, and build the plugin SDK and examples
//! - `plugins` format-check, lint, test and build the plugin SDK and example plugins (`plugins/`, a
//!   workspace of its own) for wasm32: `<target>/plugins/dist/<example>/` holds each manifest and
//!   module, ready for Plugins ▸ Development ▸ Import plugin from manifest…; then runs every
//!   built example in the engine (`lizard-cli run`), so a module that no longer works fails here
//! - `bench`   release-build timings at 1k/10k/50k layers (`bench 1000,10000` for other sizes) and
//!   with a synthetic design system used by 2,000 instances

mod icon;
mod layers;

use std::process::{Command, ExitCode};

fn run(cmd: &mut Command) -> bool {
    eprintln!("$ {cmd:?}");
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(|p| p.to_path_buf()).unwrap_or_default()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let task = args.first().map(String::as_str).unwrap_or("help");
    let ok = match task {
        "ci" => ci(),
        "layers" => layers::check(&root()),
        "assets" => assets(),
        "parity" => parity(),
        "version" => version(args.get(1).map(String::as_str), args.get(2).map(String::as_str)),
        "icon" => icon::generate(&root()),
        "wasm" => wasm(),
        "plugins" => plugins(),
        "bench" => bench(args.get(1).map_or("1000,10000,50000", String::as_str)),
        _ => {
            eprintln!("tasks: ci, layers, assets, parity, version [set X.Y.Z], icon, wasm, plugins, bench [SIZES]");
            task == "help"
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn ci() -> bool {
    run(cargo().args(["fmt", "--all", "--check"]))
        && run(cargo().args(["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]))
        && run(cargo().args(["test", "--workspace"]))
        && layers::check(&root())
        && assets()
        && wasm()
}

fn wasm() -> bool {
    run(cargo().args([
        "check",
        "--target",
        "wasm32-unknown-unknown",
        "-p",
        "lizard-collab",
        "-p",
        "lizard-collab-net",
        "-p",
        "lizard-engine",
        "-p",
        "lizard-ui-egui",
        "-p",
        "lizard-web",
    ])) && plugins()
}

/// The example plugins in `plugins/examples` (each folder's name is its package; its module is
/// `<name with _>.wasm`).
const EXAMPLE_PLUGINS: &[&str] = &["tidy-names", "swatch-grid", "counter", "focus-list", "dot-grid"];

/// The plugin workspace: fmt, clippy and build for wasm32, host tests, then each example's manifest
/// and module side by side in `<target>/plugins/dist/<example>/`.
fn plugins() -> bool {
    let root = root();
    let manifest = root.join("plugins").join("Cargo.toml");
    let target = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| root.join("target")).join("plugins");
    let ws = |args: &[&str]| {
        let mut c = cargo();
        c.args(args).arg("--manifest-path").arg(&manifest);
        c
    };
    let wasm = ["--target", "wasm32-unknown-unknown", "--locked"];
    let ok = run(&mut ws(&["fmt", "--all", "--check"]))
        && run(ws(&["clippy", "--workspace", "--release"]).args(wasm).arg("--target-dir").arg(&target).args(["--", "-D", "warnings"]))
        && run(ws(&["test", "--workspace", "--locked"]).arg("--target-dir").arg(&target))
        && run(ws(&["build", "--workspace", "--release"]).args(wasm).arg("--target-dir").arg(&target));
    if !ok {
        return false;
    }
    for name in EXAMPLE_PLUGINS {
        let module = format!("{}.wasm", name.replace('-', "_"));
        let dist = target.join("dist").join(name);
        let copied = std::fs::create_dir_all(&dist)
            .and_then(|()| std::fs::copy(root.join("plugins/examples").join(name).join("lizard-plugin.json"), dist.join("lizard-plugin.json")))
            .and_then(|_| std::fs::copy(target.join("wasm32-unknown-unknown/release").join(&module), dist.join(&module)));
        match copied {
            Ok(bytes) => eprintln!("plugins: {} ({} KB)", dist.display(), bytes / 1024),
            Err(e) => {
                eprintln!("plugins: {name}: {e}");
                return false;
            }
        }
    }
    smoke(&target)
}

/// What the built examples are asked to do, each in the engine as a script would: the commands of
/// every plugin API (a run, a panel, a widget with its clicks, typed text, menu and timer). A
/// widget command without an `id` acts on the selected widget (the one just inserted).
const SMOKE: &[(&str, &str)] = &[
    ("shape.rectangle", r#"{"x": 0, "y": 0, "width": 40, "height": 40, "props": {"name": "  a   layer copy "}}"#),
    ("select.all", "{}"),
    ("plugin.run", r#"{"id": "com.brava.lizard.examples.tidy-names", "command": "tidy", "params": {"case": "Title Case"}}"#),
    ("plugin.run", r#"{"id": "com.brava.lizard.examples.swatch-grid", "command": "panel"}"#),
    ("plugin.event", r#"{"id": "com.brava.lizard.examples.swatch-grid", "control": "palette", "value": "Ocean"}"#),
    ("plugin.event", r#"{"id": "com.brava.lizard.examples.swatch-grid", "control": "generate"}"#),
    ("plugin.close", r#"{"id": "com.brava.lizard.examples.swatch-grid"}"#),
    ("widget.insert", r#"{"plugin": "com.brava.lizard.examples.counter", "x": 0, "y": 200}"#),
    ("widget.click", r#"{"target": "increment"}"#),
    ("widget.menuAction", r#"{"item": "step", "value": "5"}"#),
    ("widget.menuAction", r#"{"item": "reset"}"#),
    ("widget.insert", r#"{"plugin": "com.brava.lizard.examples.focus-list", "x": 0, "y": 400}"#),
    ("widget.setText", r#"{"input": "title", "text": "Smoke test"}"#),
    ("widget.setText", r#"{"input": "new", "text": "One more"}"#),
    ("widget.click", r#"{"target": "toggle.0"}"#),
    ("widget.click", r#"{"target": "focus"}"#),
    ("widget.fireTimer", "{}"),
    ("widget.list", "{}"),
    ("plugin.run", r#"{"id": "com.brava.lizard.examples.dot-grid"}"#),
    ("plugin.event", r#"{"id": "com.brava.lizard.examples.dot-grid", "control": "columns", "value": 4}"#),
    ("plugin.event", r#"{"id": "com.brava.lizard.examples.dot-grid", "control": "generate"}"#),
    ("plugin.close", r#"{"id": "com.brava.lizard.examples.dot-grid"}"#),
];

/// Import the examples built into `<target>/dist` and run [`SMOKE`] with them.
fn smoke(target: &std::path::Path) -> bool {
    let dist = target.join("dist").to_string_lossy().replace('\\', "/");
    let imports = EXAMPLE_PLUGINS.iter().map(|name| format!(r#"{{"command": "plugin.import", "params": {{"path": "{dist}/{name}"}}}}"#));
    let steps = SMOKE.iter().map(|(command, params)| format!(r#"{{"command": "{command}", "params": {params}}}"#));
    let script = target.join("smoke.jsonl");
    if let Err(e) = std::fs::write(&script, imports.chain(steps).collect::<Vec<_>>().join("\n")) {
        eprintln!("plugins: {}: {e}", script.display());
        return false;
    }
    run(cargo().args(["run", "-q", "-p", "lizard-cli", "--", "run"]).arg(&script).stdout(std::process::Stdio::null()))
}

/// Engine/render timings (`lizard-cli perf`) and UI frame timings (an ignored test driving the
/// whole UI headless), release builds, for each comma-separated document size in layers.
fn bench(sizes: &str) -> bool {
    run(cargo().args(["run", "--release", "-p", "lizard-cli", "--", "perf", "--layers", sizes]))
        && run(cargo().env("LIZARD_BENCH_LAYERS", sizes).args([
            "test",
            "--release",
            "-p",
            "lizard-ui-egui",
            "--lib",
            "perf_ui_frames",
            "--",
            "--ignored",
            "--nocapture",
        ]))
}

/// Keep the licences required by bundled icon and app artwork.
fn assets() -> bool {
    let root = root();
    let required = [
        "NOTICE",
        "LICENSE-MIT",
        "LICENSE-APACHE",
        "assets/app-icon/LICENSE.txt",
        "assets/icon-packs/LICENSE-lucide",
        "assets/icon-packs/LICENSE-feather",
        "assets/icon-packs/LICENSE-phosphor",
        "assets/icon-packs/LICENSE-material",
    ];
    let missing: Vec<_> = required.into_iter().filter(|path| !root.join(path).is_file()).collect();
    if !missing.is_empty() {
        eprintln!("missing asset licences or notices:\n  {}", missing.join("\n  "));
        return false;
    }
    eprintln!("assets: ok");
    true
}

fn parity() -> bool {
    let extra = [
        "file.open",
        "file.import",
        "file.placeImage",
        "file.save",
        "file.saveAs",
        "file.export",
        "file.exportSvg",
        "edit.copyAsText",
        "edit.copyAsCss",
        "edit.copyAsSvg",
        "edit.copyAsPng",
        "edit.copyLink",
    ];
    let (done, total, missing) = lizard_engine::catalog::parity(&extra);
    let pct = done as f64 * 100.0 / total.max(1) as f64;
    // The generation date: the date of the last commit (no clock dependency in xtask).
    let date = Command::new("git")
        .args(["log", "-1", "--format=%cs"])
        .current_dir(root())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|d| d.trim().to_string())
        .unwrap_or_default();
    let mut md = format!(
        "# Parity checklist\n\n> **Generated:** {date} by `cargo xtask parity`. Do not edit by hand.\n\nLizard commands registered for the incumbent's main menu and toolbar ({} commands in the engine).\n\n**{done} / {total} menu items ({pct:.1}%)**\n\n",
        lizard_engine::all_commands().len()
    );
    if missing.is_empty() {
        md.push_str("Every catalog item has a command.\n");
    } else {
        md.push_str("## Missing\n\n");
        for m in &missing {
            md.push_str(&format!("- `{m}`\n"));
        }
    }
    md.push_str("\nMenu coverage measures breadth only; see docs/target-app-parity.md for depth and fidelity.\n");
    let path = root().join("docs/parity-checklist.md");
    if let Err(e) = std::fs::write(&path, md) {
        eprintln!("{}: {e}", path.display());
        return false;
    }
    eprintln!("parity: {done}/{total} ({pct:.1}%) → docs/parity-checklist.md");
    true
}

fn version(sub: Option<&str>, value: Option<&str>) -> bool {
    let path = root().join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&path) else { return false };
    let cur = text
        .lines()
        .skip_while(|l| !l.starts_with("[workspace.package]"))
        .find_map(|l| l.strip_prefix("version = \"").and_then(|r| r.strip_suffix('"')))
        .unwrap_or("0.0.0")
        .to_string();
    match (sub, value) {
        (Some("set"), Some(v)) => {
            let valid =
                v.split(['-', '+']).next().is_some_and(|core| core.split('.').count() == 3 && core.split('.').all(|p| p.parse::<u64>().is_ok()));
            if !valid {
                eprintln!("version must look like 1.2.3 or 1.2.3-rc.1");
                return false;
            }
            let new = text.replacen(&format!("version = \"{cur}\""), &format!("version = \"{v}\""), 1);
            if std::fs::write(&path, new).is_err() {
                return false;
            }
            eprintln!("version {cur} → {v}");
            run(cargo().args(["update", "--workspace"]))
        }
        _ => {
            println!("{cur}");
            true
        }
    }
}
