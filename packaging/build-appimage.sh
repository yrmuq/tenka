#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
dist="$PWD/dist"
mkdir -p "$dist"
appdir="$(mktemp -d "$dist/.appimage.XXXXXX")"
trap 'rm -rf "$appdir"' EXIT

source packaging/appimage-pins.env
tool_url="https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_TAG/appimagetool-x86_64.AppImage"
runtime_url="https://github.com/AppImage/type2-runtime/releases/download/$APPIMAGE_RUNTIME_TAG/runtime-x86_64"
curl --retry 3 --fail --silent --show-error --location "$tool_url" --output "$appdir/appimagetool"
curl --retry 3 --fail --silent --show-error --location "$runtime_url" --output "$appdir/runtime"
(
    cd "$appdir"
    printf '%s  appimagetool\n%s  runtime\n' "$APPIMAGETOOL_SHA" "$APPIMAGE_RUNTIME_SHA" |
        sha256sum --check
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
files=("$asset")
options=()
case "${TENKA_CHANNEL:-dev}" in
    stable) options=(-u "$update"); files+=("$asset.zsync") ;;
    dev|deps) ;;
    *) printf 'Unknown AppImage channel: %s\n' "$TENKA_CHANNEL" >&2; exit 1 ;;
esac
export ARCH=x86_64
cd "$appdir"
"$appdir/appimagetool" --appimage-extract-and-run --runtime-file "$appdir/runtime" \
    "${options[@]}" "$appdir/Tenka.AppDir" "$asset"

if [[ "${TENKA_CHANNEL:-dev}" == stable ]]; then
    test -s "$asset.zsync"
    grep -aqF "$update" "$asset"
    grep -aqxF "Filename: $asset" "$asset.zsync"
    grep -aqxF "Length: $(stat --format=%s "$asset")" "$asset.zsync"
    grep -aqxF "SHA-1: $(sha1sum "$asset" | cut -d' ' -f1)" "$asset.zsync"
elif [[ -e "$asset.zsync" ]] || grep -aqF "$update" "$asset"; then
    echo 'A non-stable AppImage contains stable update metadata.' >&2
    exit 1
fi
sha256sum "${files[@]}" > SHA256SUMS
sha256sum --check SHA256SUMS
mv -- "${files[@]}" SHA256SUMS "$dist/"
