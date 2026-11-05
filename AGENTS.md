# Lizard — instructions for agents

Lizard is an independent Rust/egui interface design editor. The desktop executable and public product name are Lizard. New native documents use `.lzd`; legacy `.lizard` and `.figcraft` documents still open. Internal workspace crates use `lizard-*` names. Workbench UI is the reusable egui component library and its separate catalog.

## Start here

1. Read `ROADMAP.md`, `docs/gaps.md`, and any local `plan/STATUS.md` or `plan/execution-plan.md` before planning work.
2. Inspect the working tree and preserve user changes. Implement the requested work, test it, and inspect screenshots for UI changes.
3. Keep the configured `upstream` remote read-only. Never push Lizard work to it.

## Architecture and quality

- User-visible editor operations go through the command engine in `crates/engine/src/cmd/`. UI panels dispatch commands and read engine state.
- Keep the layer boundaries checked by `cargo xtask layers`. Lower-level crates must not depend on egui.
- Shipped Rust code must not use `unwrap()`, `expect()`, `panic!`, `todo!`, `unimplemented!`, `unreachable!`, or unsafe code. Add regression tests for crash fixes.
- Preserve native and WASM builds. Run `cargo xtask ci` before committing when resources permit; report any gate that could not run.
- Commit coherent changes with a milestone or task label. Push only to the Lizard repository's `main` after verification.

## Assets and provenance

- Use original or redistributable assets. Keep required licences beside bundled assets and third-party notices in `NOTICE`.
- The former project's ArtCraft marks are not part of Lizard. Do not restore them.
- Preserve the original contributors' credits, licences, and git history. Do not rewrite authorship.
- Fonts remain external through the optional `CRAFT_FONTS_DIR`; builds must work without them.

## Running the app

```sh
cargo run -p lizard --bin lizard -- --control <port>
```

On macOS, `packaging/macos/dev-app.sh --control <port>` launches the app bundle with its **Lizard** Dock name.

The JSON control channel and screenshot method are documented in `docs/control-protocol.md`. The CLI workspace package is still `lizard-cli`.
