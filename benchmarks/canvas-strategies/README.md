# GPU canvas background preparation experiment

This standalone Rust package exercises a retained GPU tile compositor on a saved Lizard document. It is a benchmark prototype, not the editor's shipped renderer. Run it from the repository root on macOS with a Metal GPU. If `benchmarks/canvas-strategies/fixtures/ultrawide-workbook.figcraft` is missing, generate it from Lizard's example documents first:

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

`--background-warm` presents the window before preparing tiles. A GPU worker first generates a small overview, then fills the visible viewport, nearby pan tiles, and zoom detail tiles. The main thread installs completed textures in a bounded atlas and redraws in batches. Live tile requests take priority over planned work, including when a tile was previously rendered and then evicted. A 100 ms redraw watchdog covers a missed initial or final wakeup. The ordinary pan/zoom trace starts after preparation; `--interact-during-warm` starts it when the initial visible base and detail tiles are ready. Add `--interact-at-preview` to begin at the first low-resolution overview. These early-interaction modes cannot be combined with `--verify`, whose pixel reference assumes that the initial camera's tiles remain resident.

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin streaming_window -- --slots=240 --background-warm --interact-during-warm
```

The measured fixture has 3,887 nodes and 77 frames. On an Apple M1, the 240-slot atlas occupies 471.3 MiB. That is one fixed GPU allocation, not the process's total memory. A later four-worker asynchronous run completed background preparation in 3.89 s and presented a verified 120-position trace after preparation with no missing base or detail tiles. Its peak process memory footprint was 1.41 GiB. The seeded random trace found an eviction bug, now fixed, and a remaining 133 ms stale-detail streak in the best observed post-preparation configuration. Starting interaction at the first overview exposed a 1.27 s missing-base streak. The low-resolution overview is soft at full UltraWide resolution. The prototype is not ready to replace the editor's renderer yet.

See [the dated measurement report](RESULTS-2026-10-28.md) for methods, memory, effect parity, edit latency, comparison limitations, and outstanding gates. The prototype is not the editor's default renderer.

Use `--trace-repeats=10` to replay the same 120 positions ten times and check whether retained memory stabilizes. On macOS, prefix the command with `/usr/bin/time -l` to capture the process's peak resident set size and peak memory footprint. The Metal atlas estimate above is only one allocation and should not be compared directly with either process-wide number.

Run the controlled effect parity probe with:

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin effect_parity
```

The retained-tile scheduler and the broader parity probes can be repeated with:

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin streaming_window -- --slots=240 --background-warm --cpu-workers=4 --cpu-scheduler=async --verify
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin streaming_window -- --slots=240 --background-warm --cpu-workers=4 --cpu-scheduler=async --interact-during-warm
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml --bin corpus_parity
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml --bin shadow_parity
```

Keep the benchmark window frontmost during timed runs. `--diagnose-routes` prints the number of tiles requiring the CPU fidelity path. The corpus parity probe saves selected PNG pairs below the ignored `target/canvas-strategies/parity/` directory.

Use `--trace=random --trace-repeats=3` for the deterministic pointer-like pan and reversing-scroll stress trace. `--cpu-regions=2 --cpu-workers=4 --cpu-scheduler=async` enables faithful 2×2 CPU region jobs. `--region-inflight=2` limits simultaneous region work; `--slots=255 --detail-slots=95` gives the measured 500.6 MiB atlas that performed best in one sweep. The single texture-array atlas must fit the device's array-layer limit. See the report for the measured tradeoffs and remaining quality failures.

`cargo run --release --manifest-path benchmarks/canvas-strategies/Cargo.toml --bin geometry_overlay_parity` measures a sharp geometry pass over a soft effect preview. Add `-- --icon` to test the saved Lizard icon. The workbook improves, but the icon loses inner-effect fidelity, so this remains a diagnostic experiment.

The newer signed-residual experiments keep geometry sharp while approximating soft effects. Compare their output against the document renderer with:

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin gpu_residual_probe -- --factor=2
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin gpu_residual_probe -- --icon --factor=2 --save
```

For an actual Metal window and a pointer-like 75 Hz trace, run `gpu_residual_window --factor=2 --focus-delay-ms=2000`. Add `--trace-repeats=10` for 1,200 camera states, `--baked` to apply the correction once on the CPU, or `--direct` to draw to the swapchain without the intermediate texture. Run from the repository root and leave the benchmark window frontmost. The residual window is a separate prototype with a source image prepared for its known camera trace; it cannot represent arbitrary navigation or edits in Lizard yet. The [measurement report](RESULTS-2026-10-28.md) records fidelity, frame timing, memory and the remaining integration gates.

The finite-source experiment removes that advance knowledge. Use `streaming_window --sharp-underlay --direct-underlay --source-guard=512 --trace=random --trace-repeats=10 --background-warm --interact-during-warm --interact-at-preview` to measure sharp initial coverage and whole-viewport tile handoff. It prints `covered_cameras`; `--oracle-source` is for reproducing the older known-trace result only. `--defer-tiles-until-idle` isolates interaction timing by postponing faithful tile preparation until the trace ends. That mode cannot claim full visual coverage when `covered_cameras` is below the trace length.

`gpu_live_residual_window` tests full-viewport Vello vectors with a document-wide low-resolution effect correction. Run it with `LIZARD_GPU_NO_EFFECTS=1` and `--trace-repeats=10`. Add `--cache-guard=512` or `--cache-guard=1024` to use a finite sharp source around the initial camera and fall back to live vectors outside it. `--cached-two-pass` tests the cached source through an intermediate texture. `--low-cpu-base` computes the effect correction against the document renderer's low-resolution geometry; `--poll-background` keeps GPU callback polling off the presentation thread. `--compare-routes` compares cache, live vectors, and the faithful renderer at several covered pan and zoom positions; `--save-routes` writes the first cache/live image pair. These are benchmark-only paths, and neither has passed the editor integration gates.

The newer bounded geometry-atlas route is reproducible with:

```sh
LIZARD_GPU_NO_EFFECTS=1 CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin gpu_live_residual_window -- --geometry-atlas --atlas-slots=96 \
  --start-atlas-ready --initial-margin=2 --atlas-upload-limit=2 \
  --motion-delay-ms=4000 --warm-frame-interval-ms=500 \
  --poll-background --low-cpu-base --trace=random --trace-repeats=10
```

Change `--trace=random` to `--trace=excursion` to pan beyond the prepared area and exercise the live-vector fallback. `--start-atlas-ready` waits for initial coverage before the timed trace; omit it to test early interaction. `--verify-atlas` compares seven cameras against the document renderer after the trace, and `--save-atlas` writes the first actual/reference PNG pair if that camera is still resident. `--atlas-one-pass` measures the experimental fused compositor; the two-pass route is the measured baseline. The setup rejects an atlas too small for the requested initial margin and times out if initial preparation cannot finish. See the report for timing, memory, pixel differences, and the remaining readiness gates.

Run the focused regression tests with:

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo test --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin streaming_window
CARGO_TARGET_DIR=target/canvas-strategies/target cargo test --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --lib streaming_slot_moves_without_leaving_old_pixels_and_respects_zoom
```
