# Hosting Lizard for the web

`lizard-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `lizard-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `lizard-web-<hash>.js` | wasm-bindgen glue (generated, ES module) |
| `lizard-web-<hash>_bg.wasm.gz` | The app, precompressed for hosts with a 25 MiB per-file limit |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code. The package adds a small loader to Trunk's generated page that
decompresses the WASM with the browser's `DecompressionStream('gzip')` before wasm-bindgen starts.
Upload the folder's contents anywhere that serves static files; do not configure the host to
decompress `.wasm.gz` itself.

The editor runs entirely on the visitor's device. The web build starts with one new document;
there is no desktop TCP control server, and native file dialogs aren't wired up yet. A browser
with WebGPU or WebGL2 is required.

## Live sessions

The web build can join a live session hosted by the desktop app (File ▸ Join live session…, then
paste the invite link); it can't host one. Desktop-hosted sessions use plain `ws://`, which
browsers block from a page served over `https://` except to the same computer. To join sessions on
other machines, serve the site over `http://` on a trusted network (or use the desktop app);
`wss://` comes with the standalone server.

## Sizes

Measured on the 2026-10-30 WebGPU build (`trunk build --release`; Python gzip level 9):

| File | Raw development build | Packaged gzip |
|---|---:|---:|
| `lizard-web-<hash>_bg.wasm` | 30,760,930 bytes (29.3 MiB) | 14,142,387 bytes (13.5 MiB) |

Per-file upload limits: Cloudflare Pages and Workers static assets reject any file over
**25 MiB** (26,214,400 bytes). `packaging/web/package.sh` checks the packaged `.wasm.gz` against
a 24 MiB threshold. `trunk serve` keeps the uncompressed WASM for development.

## Any path works

All URLs in `index.html` are relative (`public_url = "./"` in `apps/lizard-web/Trunk.toml`),
so the site works at a domain root (`https://example.com/`), under a prefix
(`https://example.com/tools/lizard/`) and from a CDN bucket. The asset names carry a content
hash, so they can be cached forever. Only `index.html` needs revalidation.

## Required server settings

- **MIME type:** serve `.js` as `text/javascript`. The `.wasm.gz` file may use
  `application/gzip` or `application/octet-stream`; the loader decompresses it explicitly.
- **Compression:** do not set `Content-Encoding: gzip` for `.wasm.gz`, since the loader already
  decompresses it. The host can compress `.js` and `.html` normally.
- **Caching:** `Cache-Control: public, max-age=31536000, immutable` on the hashed `.wasm.gz` and
  `.js` files, and `no-cache` on `index.html`.
- **HTTPS:** WebGPU (and the clipboard) only work in a secure context, which means `https://`
  or `http://localhost`. Over plain HTTP elsewhere, the app falls back to WebGL2.
- **No special isolation headers:** Lizard doesn't use `SharedArrayBuffer`, so it doesn't
  need `Cross-Origin-Opener-Policy` or `Cross-Origin-Embedder-Policy`. If your site already sends
  COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or `cross-origin`
  when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /lizard/ {
    types { application/gzip gz; text/javascript js; text/html html; }
    gzip on;
    gzip_types text/javascript text/html;
    location ~* \.(wasm\.gz|js)$ { add_header Cache-Control "public, max-age=31536000, immutable"; }
    location ~* index\.html$ { add_header Cache-Control "no-cache"; }
}
```

Local test: `python3 -m http.server 8767` inside the folder, then open http://localhost:8767/.

## Embedding in a page (iframe)

```html
<iframe
  src="https://example.com/lizard/"
  title="Lizard interface design tool"
  style="width: 100%; height: 720px; border: 0;"
  allow="fullscreen; clipboard-read; clipboard-write"
  allowfullscreen>
</iframe>
```

- The app fills the iframe and follows its size, so size the iframe and not the app.
- Keyboard shortcuts go to the iframe after the user clicks into it, as with any embedded app.
- **Sandboxed iframes** need at least `sandbox="allow-scripts allow-same-origin allow-popups"`
  (`allow-popups` lets Help links open in a new tab).
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## Renderer selection and fallback flags

Lizard renders supported canvas scenes directly with Vello on **WebGPU**. Unsupported scenes and
the **WebGL2** backend use the CPU canvas. Browser raster exports still use the CPU renderer.
A URL query flag overrides the backend, and it works on the iframe `src` too:

| Flag | Effect |
|---|---|
| *(none)* | WebGPU if available, otherwise WebGL2 |
| `?webgl` | Force the WebGL2 backend (useful when a WebGPU driver misbehaves, and in headless browsers) |

For example: `<iframe src="https://example.com/lizard/?webgl" ...>`.

A browser with neither WebGPU nor WebGL2 gets a message in place of the app.

## Smoke test (headless Chrome)

Serve `dist/web` locally, start Chrome with a DevTools port, then run the browser regression test:

```sh
python3 -m http.server 8767 --directory dist/web
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new \
  --remote-debugging-port=9224 --enable-unsafe-webgpu --disable-gpu-sandbox \
  "http://127.0.0.1:8767/"
node packaging/web/smoke-webgpu.mjs
```

The test requires Chrome to report its `BrowserWebGpu` backend, waits for the canvas, and fails
on a browser exception or GPU canvas error. Run the same test against the unpacked release zip to
exercise the compressed loader. For the WebGL2 fallback, open `?webgl` and pass `Gl` as the
optional third argument to the script.
