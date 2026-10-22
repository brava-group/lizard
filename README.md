# Lizard

Lizard is a native interface design editor built in Rust. It includes a vector canvas, auto layout, components, styles, prototyping, plugins, and an agent panel that can work with the open document.

This project continues from [FigCraft](https://github.com/storytold/figcraft) with its own interface, canvas performance work, and development direction. The original authors and contributors retain their copyright; see [NOTICE](NOTICE), [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE), and [ATTRIBUTION.md](ATTRIBUTION.md).

## Run locally

```sh
cargo run -p figcraft --bin lizard
```

The package and library crate names still use `figcraft` internally. The desktop executable is `lizard`. Existing `.figcraft` documents remain supported, and their extension has not changed.

The CLI and web build remain available through the existing workspace packages:

```sh
cargo run -p figcraft-cli -- --help
cargo xtask wasm
```

## Development

- [Roadmap](ROADMAP.md) and [ranked gaps](docs/gaps.md)
- [Architecture](docs/architecture.md)
- [Control protocol](docs/control-protocol.md) and [MCP](docs/mcp.md)
- [Plugin support](docs/plugins.md)
- [Contributor guide](AGENTS.md)

Run `cargo xtask ci` before committing. The app supports macOS, Windows, Linux, FreeBSD, and a WASM browser build. Platform packaging still carries some legacy FigCraft identifiers and is being migrated separately from the desktop rebrand.

## License

MIT OR Apache-2.0. See [NOTICE](NOTICE) for the original project attribution and third-party asset terms.
