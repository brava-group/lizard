# Relatório de limpeza — 2026-11-02

Escopo: workspace Rust do Lizard, Workbench UI, apps, crates e ferramentas. O registro das remoções está em [CLEANUP_LOG.md](CLEANUP_LOG.md).

| Métrica | Antes | Depois |
| --- | ---: | ---: |
| Linhas Rust em `apps/`, `crates/`, `xtask/` | 301.002 | 300.665 |
| Dependências declaradas sem uso | 7 | 0 |
| Avisos distintos de Clippy no workspace | 9 | 0 |
| Arquivos Rust órfãos em `src/` | 0 | 0 |

A checagem com todos os targets e features, o Clippy com `-D warnings`, a checagem WASM e `cargo xtask layers` passam sem avisos. `cargo test --workspace --exclude lizard-ui-egui` passa integralmente. A suíte completa passa no engine (1.183 testes) e encontra 21 falhas na suíte de UI, em cenários de layout, foco e traduções que não foram alterados por esta limpeza. `cargo xtask ci` para no primeiro gate, `cargo fmt --all --check`, que ainda aponta diferenças preexistentes em arquivos fora do escopo desta remoção; formatar todo o workspace produziria alterações extensas sem relação com código morto.
