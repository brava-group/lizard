"""Package Trunk's WASM below static-host per-file limits.

The page decompresses it with the browser's DecompressionStream before wasm-bindgen starts.
The source dist remains unchanged so `trunk serve` keeps its normal development behavior.
"""

import gzip
import re
import sys
from pathlib import Path


def main(site: Path) -> None:
    wasm_files = list(site.glob("*_bg.wasm"))
    if len(wasm_files) != 1:
        raise SystemExit(f"expected exactly one Trunk WASM file in {site}")
    wasm = wasm_files[0]
    page = site / "index.html"
    html = page.read_text()
    source = f"const wasm = await init({{ module_or_path: './{wasm.name}' }});"
    if html.count(source) != 1:
        raise SystemExit("Trunk's WASM initializer changed; update compress_wasm.py")
    replacement = f"""const compressed = await fetch('./{wasm.name}.gz');
if (!compressed.ok) throw new Error(`WASM download failed: ${{compressed.status}}`);
const bytes = await new Response(compressed.body.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer();
const wasm = await init({{ module_or_path: bytes }});"""
    html = html.replace(source, replacement)
    preload = re.compile(rf'<link rel="preload" href="\./{re.escape(wasm.name)}"[^>]*>')
    html, count = preload.subn("", html)
    if count != 1:
        raise SystemExit("Trunk's WASM preload changed; update compress_wasm.py")
    output = wasm.with_name(wasm.name + ".gz")
    with wasm.open("rb") as original, output.open("wb") as encoded:
        with gzip.GzipFile(fileobj=encoded, mode="wb", compresslevel=9, mtime=0, filename="") as compressed:
            while chunk := original.read(1024 * 1024):
                compressed.write(chunk)
    page.write_text(html)
    wasm.unlink()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: compress_wasm.py <staged-site-directory>")
    main(Path(sys.argv[1]))
