#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p dist
appdir="$(mktemp -d)"
trap 'rm -rf "$appdir"' EXIT

tool_url=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
runtime_url=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
curl --retry 3 --fail --silent --show-error --location "$tool_url" --output "$appdir/appimagetool"
curl --retry 3 --fail --silent --show-error --location "$runtime_url" --output "$appdir/runtime"
(
    cd "$appdir"
    sha256sum --check <<'CHECKSUMS'
ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0  appimagetool
2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d  runtime
CHECKSUMS
)
chmod +x "$appdir/appimagetool"

install -D target/release/tenka "$appdir/Tenka.AppDir/usr/bin/tenka"
install -D -m 644 LICENSE "$appdir/Tenka.AppDir/usr/share/doc/tenka/LICENSE"
cargo metadata --locked --offline --format-version 1 --filter-platform x86_64-unknown-linux-gnu |
    python3 packaging/collect-notices.py > "$appdir/Tenka.AppDir/usr/share/doc/tenka/THIRD_PARTY_NOTICES"
install -m 644 packaging/io.github.yrmuq.tenka.{desktop,png} "$appdir/Tenka.AppDir/"
ln -s usr/bin/tenka "$appdir/Tenka.AppDir/AppRun"

asset=Tenka-x86_64.AppImage
update="gh-releases-zsync|yrmuq|tenka|latest|$asset.zsync"
export ARCH=x86_64
cd dist
"$appdir/appimagetool" --appimage-extract-and-run --runtime-file "$appdir/runtime" \
    -u "$update" "$appdir/Tenka.AppDir" "$asset"

test -s "$asset.zsync"
grep -aqF "$update" "$asset"
grep -aqxF "Filename: $asset" "$asset.zsync"
grep -aqxF "Length: $(stat --format=%s "$asset")" "$asset.zsync"
grep -aqxF "SHA-1: $(sha1sum "$asset" | cut -d' ' -f1)" "$asset.zsync"
sha256sum "$asset" "$asset.zsync" > SHA256SUMS
sha256sum --check SHA256SUMS
