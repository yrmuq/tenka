import json
import re
import subprocess
from pathlib import Path


def version(tag):
    return tuple(int(part) for part in tag.split("."))


pins = Path("packaging/appimage-pins.env")
values = dict(line.split("=", 1) for line in pins.read_text().splitlines())
for key, repository, asset_name, tag_pattern in (
    ("APPIMAGETOOL", "AppImage/appimagetool", "appimagetool-x86_64.AppImage", r"\d+\.\d+\.\d+"),
    ("APPIMAGE_RUNTIME", "AppImage/type2-runtime", "runtime-x86_64", r"\d{8}"),
):
    releases = json.loads(
        subprocess.check_output(["gh", "api", f"repos/{repository}/releases?per_page=20"])
    )
    candidates = [
        (release["tag_name"], asset["digest"])
        for release in releases
        if not release["draft"] and not release["prerelease"]
        and re.fullmatch(tag_pattern, release["tag_name"])
        for asset in release["assets"]
        if asset["name"] == asset_name and re.fullmatch(r"sha256:[0-9a-f]{64}", asset.get("digest") or "")
    ]
    if candidates:
        tag, digest = max(candidates, key=lambda candidate: version(candidate[0]))
        if version(tag) > version(values[f"{key}_TAG"]):
            values[f"{key}_TAG"] = tag
            values[f"{key}_SHA"] = digest.removeprefix("sha256:")

pins.write_text("".join(f"{key}={value}\n" for key, value in values.items()))
