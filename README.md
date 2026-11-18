<div align="center">
<img src="packaging/io.github.yrmuq.tenka.png" width="112" alt="Tenka logo">

# Tenka

</div>

---

> [!NOTE]
> Tenka is under active development. Behavior may change, bugs may occur, and saved settings may become outdated.

---

Tenka is a Linux image optimizer. Its Rust engine automatically selects compression and optionally reduces resolution while keeping the saved image visually close to the original. The engine is a separate Rust library for reuse in desktop apps and container workloads.

![Tenka's automatic conversion tab](docs/tenka.jpg)

---

## Features

- Batch conversion of still, 8-bit JPEG, PNG, and WebP images to JPEG XL or WebP.
- Automatic compression selection and optional resolution reduction based on similarity to the original.
- Lossless encoding of prepared pixels, with source orientation applied and supported ICC profiles converted to sRGB.
- Batch tests at up to eleven quality thresholds in the selected codec, with saved results and previews.
- English and Russian interface with system, light, and dark themes.

---

<details>
<summary><h2>Controls &amp; Shortcuts</h2></summary>

| Input | Action |
| --- | --- |
| Drop files or select **Add images** | Add source images to the conversion queue. |
| **Output format** | Choose JPEG XL or WebP. |
| Enable **Automatically reduce resolution** | Also test smaller dimensions for visual similarity to the original. |
| Select **Convert pending** | Save files in the selected format beside their originals or in the chosen folder. Existing files are kept. |
| Set **Quality, %** in **Tests** and select **+** | Add up to ten extra thresholds from 1 to 100. At 100, Tenka encodes losslessly at the original resolution. |
| Select **Run tests and save** in **Tests** | Save one result per threshold in the selected format, next to the source or in the chosen folder. The file name includes the threshold, such as `photo-90.webp`. |
| Select a test result | Display it beside the source preview. |

Tenka compares each result with the original at the same scale and checks local pixel differences to catch loss of fine detail. For large images, it estimates quality from a full-frame preview and several native-scale crops.

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
| libwebp | [webmproject/libwebp](https://github.com/webmproject/libwebp) | `BSD-3-Clause` | WebP encoder for conversion and comparison. |
| SSIMULACRA2 | [rust-av/ssimulacra2](https://github.com/rust-av/ssimulacra2) | `BSD-2-Clause` | Perceptual quality metric. |
