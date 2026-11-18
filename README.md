<div align="center">
<img src="packaging/io.github.yrmuq.tenka.png" width="112" alt="Tenka logo">

# Tenka

</div>

---

> [!NOTE]
> Tenka is under active development. Behavior may change, bugs may occur, and saved settings may become outdated.

---

Tenka is a Linux image optimizer. Its Rust engine selects JPEG XL compression and can resize each image to meet a file-size target. The engine is a separate Rust library for reuse in desktop apps and container workloads.

![Tenka's automatic JPEG XL conversion tab](docs/tenka.jpg)

---

## Features

- Batch conversion of still, 8-bit JPEG, PNG, and WebP images to JPEG XL.
- Per-image quality selection, optional maximum edge, and optional file-size target.
- Lossless encoding of prepared pixels, with source orientation applied and supported ICC profiles converted to sRGB.
- JPEG XL and WebP comparison with measured file size, quality, encode time, transparency, and previews.
- English and Russian interface with system, light, and dark themes.

---

<details>
<summary><h2>Controls &amp; Shortcuts</h2></summary>

| Input | Action |
| --- | --- |
| Drop files or select **Add images** | Add source images to the conversion queue. |
| Set a target size and enable **Resize if needed** | Try smaller dimensions if the target cannot be met at the starting resolution. |
| Select **Convert pending** | Save `.jxl` files beside their originals or in the chosen folder. Existing files are kept. |
| Select **Run comparison** in **Tests** | Measure JPEG XL and WebP variants of the selected source. |
| Select a codec result | Display it beside the source preview. |

On large images, quality is estimated from a full-frame preview and a native-resolution crop. The score measures encoding at the chosen dimensions.

</details>

---

## Installation & Usage

### Architecture & Format Support

| Format | x86_64 | arm64 |
| --- | --- | --- |
| AppImage | Development build in GitHub Actions | ❌ |
| Flatpak | ❌ | ❌ |

### AppImage

Download the x86_64 package from the [development build](https://github.com/yrmuq/tenka/actions/workflows/dev.yml). Open the `.AppImage` with [Gear Lever](https://flathub.org/apps/it.mijorus.gearlever). Linux desktop file dialogs use the desktop portal. X11 sessions require the system libxkbcommon-x11 library. The `.zsync` file supplies delta-update metadata for compatible update tools.

### Flatpak

Flatpak packaging is planned.

---

## License & Attribution

Tenka is distributed under the [GNU General Public License, version 3](LICENSE).

| Component | Upstream | License | Integration Details |
| --- | --- | --- | --- |
| egui / eframe | [emilk/egui](https://github.com/emilk/egui) | `MIT OR Apache-2.0` | Desktop interface. |
| jpegxl-rs | [inflation/jpegxl-rs](https://github.com/inflation/jpegxl-rs) | `GPL-3.0-or-later` | Rust bindings for JPEG XL. |
| libjxl | [libjxl/libjxl](https://github.com/libjxl/libjxl) | `BSD-3-Clause` | JPEG XL codec. |
| libwebp | [webmproject/libwebp](https://github.com/webmproject/libwebp) | `BSD-3-Clause` | WebP comparison encoder. |
| SSIMULACRA2 | [rust-av/ssimulacra2](https://github.com/rust-av/ssimulacra2) | `BSD-2-Clause` | Perceptual quality metric. |
