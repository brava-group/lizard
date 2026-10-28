# GPU canvas background preparation experiment

This standalone Rust package exercises a retained GPU tile compositor on a saved Lizard document. It is a benchmark prototype, not the editor's shipped renderer. Run it from the repository root on macOS with a Metal GPU. If `examples/ultrawide-workbook.figcraft` is missing, generate it from Lizard's example documents first:

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin make_fixture
```

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin streaming_window -- --slots=240 --background-warm --verify
```

`--background-warm` presents the window before preparing tiles. A GPU worker first generates a small overview, then fills the visible viewport, nearby pan tiles, and zoom detail tiles. The main thread installs completed textures in a bounded atlas and redraws in batches. Live tile requests take priority over planned work. A 100 ms redraw watchdog covers a missed initial or final wakeup. The ordinary pan/zoom trace starts after preparation; `--interact-during-warm` starts it when the base pan area is ready. The latter mode cannot be combined with `--verify`, whose pixel reference assumes that the initial camera's tiles remain resident.

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin streaming_window -- --slots=240 --background-warm --interact-during-warm
```

Measured on an Apple M1 Metal display at 5120 × 2100 and 75 Hz, using the 3,887-node, 77-frame workbook. Values are from one run per mode of the persisted benchmark source. Times start when the benchmark creates the window and end when it submits the indicated frame to the GPU; they are not photon-on-screen measurements. Repeat runs vary with system load.

| Mode | First image submitted | Initial full-resolution area | Pan area ready | All 238 tiles ready | Missing base frames | Stale zoom-detail frames |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Synchronous preparation | 4.78 s | 4.78 s | 4.78 s | 4.78 s | 0 | 0 |
| Background preparation, trace after ready | 1.45 s | 2.70 s | 4.89 s | 6.29 s | 0 | 0 |
| Background preparation, early trace | 1.15 s | 2.58 s | 4.68 s | 6.72 s | 0 | 26 |

The background mode opens visibly earlier, but it does **not** meet a four-second full-ready target on this fixture. Early pan stays covered; early zoom still shows stale detail for roughly 0.3 seconds while detail tiles are generated. The early-interaction run coalesced 8 of 120 synthetic input states. The verified run had 0 missing/stale frames, 120 completed states, 35.27 ms p95 GPU queue completion, and 0 completions over 50 ms. Its pixel difference from the CPU reference was 0.891% at base zoom and 0.914% at detail zoom using a 4/255 threshold. The atlas occupies about 471 MiB, excluding other GPU allocations.

The overview is deliberately low resolution and may look soft until full-resolution tiles replace it. The prototype needs a sharp initial path, complete effect parity, a smaller memory budget, and a documented handoff before it can replace the current editor canvas. The benchmark uses a synthetic camera trace; it does not yet measure real pointer input or the editor's text-edit latency.

Run the focused regression tests with:

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo test --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin streaming_window
CARGO_TARGET_DIR=target/canvas-strategies/target cargo test --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --lib streaming_slot_moves_without_leaving_old_pixels_and_respects_zoom
```
