#!/usr/bin/env python3
"""Build the offline icon catalog from pinned, redistributable npm archives.

Run from the repository root. The resulting gzip has a fixed timestamp and sorted keys.
Only SVGs and their package licences are copied into the product.
"""

from __future__ import annotations

import gzip
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile


ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "assets" / "icon-packs"
SOURCES = (
    ("lucide", "lucide-static", "0.563.0", "https://registry.npmjs.org/lucide-static/-/lucide-static-0.563.0.tgz", "ee997d5aa86b13342156a54a450adb97af39130463400a62ac7177d3db8b8cb4"),
    ("feather", "feather-icons", "4.29.2", "https://registry.npmjs.org/feather-icons/-/feather-icons-4.29.2.tgz", "28b6cad56600feee25c53051d65326621f2675d30d6f3d9bd3175f6117f84476"),
    ("phosphor", "@phosphor-icons/core", "2.1.1", "https://registry.npmjs.org/@phosphor-icons/core/-/core-2.1.1.tgz", "313332be6190b724da24107addd781799b48bf76b13963f24501112ffe1baadd"),
    ("material", "@material-symbols/svg-400", "0.48.1", "https://registry.npmjs.org/@material-symbols/svg-400/-/svg-400-0.48.1.tgz", "b2bbc5c1d0ef6148437fb2ed82ed98444d4acbd64bae58a142c4f03d6390ac7d"),
)


def archive(name: str, url: str) -> bytes:
    cached = Path("/tmp") / f"lizard-icons-{name}.tgz"
    if cached.is_file():
        return cached.read_bytes()
    return subprocess.run(["curl", "-fsSL", "--max-time", "120", url], check=True, capture_output=True).stdout


def target(source: str, path: str) -> tuple[str, str] | None:
    parts = path.split("/")
    if source == "lucide" and len(parts) == 3 and parts[:2] == ["package", "icons"]:
        return "lucide", Path(parts[-1]).stem
    if source == "feather" and len(parts) == 4 and parts[:3] == ["package", "dist", "icons"]:
        return "feather", Path(parts[-1]).stem
    if source == "phosphor" and len(parts) == 4 and parts[:2] == ["package", "assets"]:
        return "phosphor", Path(parts[-1]).stem
    if source == "material" and len(parts) == 3 and parts[1] in ("outlined", "rounded", "sharp"):
        return f"material-symbols-{parts[1]}", Path(parts[-1]).stem
    return None


def main() -> None:
    icons: dict[str, dict[str, str]] = {}
    provenance = []
    DEST.mkdir(parents=True, exist_ok=True)
    for source, package, version, url, expected_sha256 in SOURCES:
        data = archive(source, url)
        digest = hashlib.sha256(data).hexdigest()
        if digest != expected_sha256:
            raise ValueError(f"archive digest mismatch: {package} {version}")
        provenance.append({"package": package, "version": version, "url": url, "sha256": digest})
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
            licence = tar.extractfile("package/LICENSE")
            if licence is None:
                raise ValueError(f"missing licence: {package}")
            (DEST / f"LICENSE-{source}").write_bytes(licence.read().replace(b"\r\n", b"\n"))
            for member in tar:
                if not member.isfile() or not member.name.endswith(".svg"):
                    continue
                key = target(source, member.name)
                if key is None:
                    continue
                file = tar.extractfile(member)
                if file is None:
                    raise ValueError(f"missing SVG: {member.name}")
                svg = file.read().decode("utf-8")
                if "<svg" not in svg or any(tag in svg.lower() for tag in ("<script", "<image", "<foreignobject")):
                    raise ValueError(f"unexpected SVG content: {member.name}")
                family, name = key
                if name in icons.setdefault(family, {}):
                    raise ValueError(f"duplicate icon: {family}/{name}")
                icons[family][name] = svg
    payload = json.dumps({"schema": 1, "sources": provenance, "icons": icons}, sort_keys=True, separators=(",", ":")).encode()
    (DEST / "catalog.json.gz").write_bytes(gzip.compress(payload, compresslevel=9, mtime=0))
    print(" ".join(f"{family}={len(entries)}" for family, entries in sorted(icons.items())))
    print(f"raw={len(payload)} gzip={(DEST / 'catalog.json.gz').stat().st_size}")


if __name__ == "__main__":
    main()
