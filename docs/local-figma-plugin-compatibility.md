# Local Figma plugin compatibility check

Checked on macOS on 2026-10-18 with ten user-supplied local packages. Their JavaScript, HTML,
images and manifests stay outside this repository. Lizard reads each package through
`plugin.figma.import` and verifies its digest again before execution.
With a synthetic image selected and the relevant menu command for the two background-removal
plugins, all ten returned an open HTML UI with no startup warning.

| Plugin | Startup result | Additional check |
|---|---|---|
| html.to.design | HTML UI requested | Full workflow pending |
| Iconify | Native HTML UI rendered its icon-set catalogue | An icon-import UI message created an editable SVG node; live search and click insertion remain unverified |
| Unsplash | Native HTML UI rendered its editorial images | Search and image placement remain unverified |
| Remove BG | Native HTML UI rendered its API key screen from the **Set API Key** menu command | The **Run** command needs the user's remove.bg API key before remote image processing can be checked |
| Material Design Icons | HTML UI requested | Icon insertion pending |
| Icons8 — icons, illustrations, photos | HTML UI requested | Search and insertion pending |
| Mockup | HTML UI requested and rendered in the macOS WebView | Remote service workflow pending |
| Figma to Code | HTML UI rendered in the macOS WebView | A selected 300×180 frame with a coloured rectangle and text produced HTML code and a preview; larger or more complex designs remain unverified |
| Anima | HTML UI requested | Export and account workflow pending |
| Background Remover by Icons8 | Native HTML UI opened with a selected synthetic image and rendered its suggestions | The plugin received the selected image bytes; actual remote background removal remains unverified |

These are startup and targeted interaction checks, **not full compatibility claims**. The
runtime now supports embedded and manifest HTML, message exchange, timers, manifest-scoped HTTPS
access, persistent client storage, nested selected layers, editable SVG insertion, image paints and
bounded access to the bytes of selected images.
Plugins can still request Figma APIs not implemented in Lizard. External services may require
accounts, API keys or availability outside Lizard's control. The macOS WebView is the only native
UI host implemented so far.

The native Figma to Code check used a synthetic frame, rectangle and text. Its preview and code
were visible in the plugin window; the HTML kept the children's positions and the rectangle's
colour. Local screenshots are `/tmp/lizard-plugin-test-native.png`,
`/tmp/lizard-icons8-remover-ready.png`, `/tmp/lizard-iconify-native.png`,
`/tmp/lizard-unsplash-native.png`, and `/tmp/lizard-removebg-key-native.png`; they are not repo
assets.

To try a package: open **Plugins ▸ Manage plugins ▸ Figma local**, import its
`lizard-import/manifest.json`, then run it from the plugin's row. If its manifest has menu
commands, choose the command shown there. The folder and the user's credentials remain local.
