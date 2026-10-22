# Development UI library

The interactive component catalog is available only in debug builds. Open **Help → UI Library (dev)**, or send `ui.set` with `{"componentLibrary":true}` through the control channel. Close it with its window close button or `{"componentLibrary":false}`.

The window has a left sidebar and 13 pages. Each page renders the production controls, not a separate visual copy: buttons (primary, secondary, disabled), number fields (value, mixed, auto, binding, sizing), text fields (committed, live, inline rename, combo, multiline), dropdowns (plain, icon, color dot), segmented controls (text and icon), checkbox and radio, paint swatches and transparency, sliders and progress, section headers and disclosures, property-panel menus, typography, icon families, and live theme tokens. Controls can be clicked, edited, scrubbed, and hovered without changing the document.

Implementation: `crates/ui-egui/src/component_library.rs`. Its module, `UiState` field, Help menu entry, control-channel toggle, and frame call are all gated with `#[cfg(debug_assertions)]`; release builds contain none of them. Changes to reusable controls should update the corresponding catalog page so it remains a useful visual regression surface.
