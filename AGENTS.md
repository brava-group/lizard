# Lizard — instructions for agents

Lizard is an independent Rust/egui interface design editor based on FigCraft. The desktop executable and public product name are Lizard. Existing `.figcraft` documents and internal `figcraft-*` crate names are retained for compatibility until a deliberate format and API migration.

## Start here

1. Read `ROADMAP.md`, `docs/gaps.md`, and any local `plan/STATUS.md` or `plan/execution-plan.md` before planning work.
2. Inspect the working tree and preserve user changes. Implement the requested work, test it, and inspect screenshots for UI changes.
3. Keep `storytold/figcraft` as a read-only upstream reference. Never push Lizard work to it.

## Architecture and quality

- User-visible editor operations go through the command engine in `crates/engine/src/cmd/`. UI panels dispatch commands and read engine state.
- Keep the layer boundaries checked by `cargo xtask layers`. Lower-level crates must not depend on egui.
- Shipped Rust code must not use `unwrap()`, `expect()`, `panic!`, `todo!`, `unimplemented!`, `unreachable!`, or unsafe code. Add regression tests for crash fixes.
- Preserve native and WASM builds. Run `cargo xtask ci` before committing when resources permit; report any gate that could not run.
- Commit coherent changes with a milestone or task label. Push only to the Lizard repository's `main` after verification.

## Assets and provenance

- Use original or redistributable assets, record every asset under `assets/` or `docs/images/` in `ATTRIBUTION.md`, and keep required licences.
- The former project's ArtCraft marks are not part of Lizard. Do not restore them.
- Preserve the original FigCraft contributors' credits, licences, and history. Do not rewrite authorship.
- Fonts remain external through the optional `CRAFT_FONTS_DIR`; builds must work without them.

## Running the app

```sh
cargo run -p figcraft --bin lizard -- --control <port>
```

The JSON control channel and screenshot method are documented in `docs/control-protocol.md`. The CLI workspace package is still `figcraft-cli`.
