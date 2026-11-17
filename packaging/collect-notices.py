"""Collect upstream notices for dependencies distributed in the AppImage."""

import json
import sys
from pathlib import Path

metadata = json.load(sys.stdin)
packages = {package["id"]: package for package in metadata["packages"]}
nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
app = next(package["id"] for package in packages.values() if package["name"] == "tenka")
queue = [app]
visited = set()

vendored = {
    "epaint_default_fonts": ["fonts"],
    "jpegxl-src": [
        "libjxl",
        "libjxl/third_party/brotli",
        "libjxl/third_party/highway",
        "libjxl/third_party/skcms",
    ],
    "libwebp-sys": ["vendor"],
}

while queue:
    current = queue.pop()
    if current in visited:
        continue
    visited.add(current)
    queue.extend(
        dep["pkg"]
        for dep in nodes[current]["deps"]
        if any(kind["kind"] is None for kind in dep["dep_kinds"])
        or (packages[current]["name"], packages[dep["pkg"]]["name"])
        == ("jpegxl-sys", "jpegxl-src")
    )

for package in sorted((packages[id_] for id_ in visited), key=lambda p: (p["name"], p["version"])):
    if package["source"] is None:
        continue
    root = Path(package["manifest_path"]).parent
    paths = (
        path
        for location in ["", *vendored.get(package["name"], [])]
        for path in (root / location).iterdir()
        if path.is_file() and path.name.upper().startswith(
            ("LICENSE", "LICENCE", "COPYING", "NOTICE", "PATENTS", "OFL", "UFL")
        )
    )
    print(f"## {package['name']} {package['version']} ({package['license']})\n")
    for path in sorted(paths):
        print(f"### {path.relative_to(root)}\n\n{path.read_text(errors='replace')}\n")
