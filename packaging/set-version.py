import re
import sys
from pathlib import Path

current, version = sys.argv[1:]
version_file = Path("VERSION")
assert version_file.read_text().strip() == current

manifest = Path("Cargo.toml")
content, count = re.subn(
    rf'(?m)^(\[workspace\.package\]\nversion = "){re.escape(current)}"$',
    lambda match: f'{match[1]}{version}"',
    manifest.read_text(),
)
assert count == 1, "Workspace version does not match VERSION"
lockfile = Path("Cargo.lock")
lock, count = re.subn(
    rf'(?m)^(name = "(?:tenka|tenka-core)"\nversion = "){re.escape(current)}"$',
    lambda match: f'{match[1]}{version}"',
    lockfile.read_text(),
)
assert count == 2, "Package versions in Cargo.lock do not match VERSION"
manifest.write_text(content)
lockfile.write_text(lock)
version_file.write_text(f"{version}\n")
