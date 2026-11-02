# Limpeza de código — 2026-11-02

Referências conferidas em `apps/`, `crates/`, `xtask/`, `benchmarks/` e `plugins/` antes das remoções. O histórico Git preserva o código excluído.

- **Workspace:** removida a dependência `lizard-tools`, que apontava para um diretório inexistente, e as dependências diretas sem uso `lizard-pathops` (SVG), `kurbo` (color), `rustls-pki-types` (collab-net), `serde` (geom) e `log` (shader e text).
- **Engine:** removidos `changed_nodes`, `live_presence`, `target_at`, `outline_in_parent`, `render_node_pixels_at` e dois stubs `unused`. As fixtures de seleção e painéis deixaram de armazenar campos não lidos; a variante `Missing` do teste de atalhos foi eliminada porque nenhum caso a usava.
- **Modelo, formato e infraestrutura:** removidos `fit_ancestors`, `starts_word`, `EXTENSION`, `layout_up`, `encode_webp`, `scan_system`, `set_history_limit`, `redo_labels`, `in_flight_bytes`, `host_handle` e o campo `Hosting::in_flight` sem leituras.
- **Interface:** removidos `key_tip`, `variable_row`, `open_at_pointer`, `visible_panel_contains` e as reexportações de `SurfaceVariant` sem consumidores. Simplificados ramos redundantes no painel de agentes e avisos de Clippy em serviços, diálogos, barra de ferramentas e testes.
- **GPU:** removidos wrappers sem chamadas em `vello_scene` (`make_scene`, `has_effects_in_view`, `has_unsupported_effects_in_view`, `register_images_at` e um auxiliar) e em `compositor` (`from_gpu`, `from_gpu_layers`, `replace_gpu_tile`, `patch_cpu_layer`), além dos tipos usados apenas por esses wrappers. O atlas de geometria foi mantido porque os benchmarks o usam.
- **Comentários e configuração:** removida uma opção comentada de `default-members`, uma regra de Clippy desconhecida da toolchain e uma nota antiga de PR; corrigida a descrição do título de janela no Windows.
- **Targets:** a varredura de fontes do sistema, os arquivos locais do diretório de plugins e o estado de reconexão exclusivo do desktop passaram a ser compilados apenas no target nativo, eliminando avisos de código não usado no WASM.
- **Teste atualizado:** `duplicating_a_variant_creates_an_instance_outside_its_set` agora verifica a regra vigente de `edit.duplicate`; sua expectativa anterior ainda exigia um terceiro componente no conjunto.
