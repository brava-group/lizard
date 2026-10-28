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

The measured fixture has 3,887 nodes and 77 frames. On an Apple M1, the current 240-slot atlas occupies 471.3 MiB. That is one fixed GPU allocation, not the process's total memory. The prototype's 60-trace run presented 7,199 of 7,200 scheduled camera states after preparation, with no missing base tiles or stale zoom-detail frames, one coalesced input, a 1,091 MiB peak macOS memory footprint, and a 36.54 ms p95 GPU queue completion. Initial preparation took 8.69 s in that run because drop shadows require a CPU fidelity fallback. The initial low-resolution preview also uses that fallback when needed: a later ten-trace run generated it in 36.48 ms and submitted the first visual frame at 0.77 s. Early zoom during preparation still showed 28 stale-detail frames in a separate run.

See [the dated measurement report](RESULTS-2026-10-28.md) for methods, memory, effect parity, edit latency, comparison limitations, and outstanding gates. The prototype is not the editor's default renderer.

Use `--trace-repeats=10` to replay the same 120 positions ten times and check whether retained memory stabilizes. On macOS, prefix the command with `/usr/bin/time -l` to capture the process's peak resident set size and peak memory footprint. The Metal atlas estimate above is only one allocation and should not be compared directly with either process-wide number.

Run the controlled effect parity probe with:

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo run --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin effect_parity
```

Run the focused regression tests with:

```sh
CARGO_TARGET_DIR=target/canvas-strategies/target cargo test --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --bin streaming_window
CARGO_TARGET_DIR=target/canvas-strategies/target cargo test --release \
  --manifest-path benchmarks/canvas-strategies/Cargo.toml \
  --lib streaming_slot_moves_without_leaving_old_pixels_and_respects_zoom
```
