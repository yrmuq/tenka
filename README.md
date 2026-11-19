<div align="center">
<img src="packaging/io.github.yrmuq.tenka.png" width="112" alt="Tenka logo">

# Tenka

</div>

---

> [!NOTE]
> Tenka is under active development. Behavior may change, bugs may occur, and saved settings may become outdated.

---

Tenka is a Linux image optimizer. Its Rust engine automatically selects compression and optionally reduces resolution while keeping the saved image visually close to the original. The engine is a separate Rust library for reuse in desktop apps and container workloads.

---

## Features

- Batch conversion of still, 8-bit JPEG, PNG, and WebP images up to 36 megapixels to JPEG XL or WebP.
- Best, Balance, and Cheap conversion modes with optional resolution reduction.
- Lossless encoding of prepared pixels, with source orientation applied and supported ICC profiles converted to sRGB.
- Full-resolution image comparison with shared zoom and pan.
- English and Russian interface with system, light, and dark themes.

---

<details>
<summary><h2>Controls &amp; Shortcuts</h2></summary>

| Input | Action |
| --- | --- |
| Drop files or select **Add images** | Add source images to the conversion queue. |
| **Output format** | Choose JPEG XL or WebP. |
| **Mode** | **Best** prioritizes similarity. **Balance** weighs similarity and proportional size savings. **Cheap** picks the smallest result that passes the perceptual and local detail checks. |
| Enable **Automatically reduce resolution** | Consider smaller dimensions while comparing visual similarity to the original. |
| Select **Convert** | Save results in the chosen format and mode beside the originals or in the chosen folder. Existing files are kept. |
| Right-click a size in **Output** | Open the folder containing the converted image. |
| Select **Compare** | Drop files into the comparison window or select **Add images**. **Split** is enabled by default and composes one view from corresponding regions of the images. Toggle it to show each image separately. Scroll to zoom at the pointer and drag with the left mouse button to pan together. |
| Select the globe | Toggle between the system language and English. |
| Select the T-shirt | Cycle through system appearance, light, and dark. The shirt shows an **A** in system mode. |

Tenka saves only results that are smaller than the source and no larger than a full-resolution lossless encode in the selected format. It compares each result with the original at the same scale. Automatic conversion also checks local pixel differences to catch loss of fine detail. For large images, Tenka estimates quality from a full-frame preview and several native-scale crops.

</details>

---

## Installation & Usage

### Architecture & Format Support

| Format | x86_64 | arm64 |
| --- | --- | --- |
| AppImage | Development build in GitHub Actions | ❌ |
| Flatpak | ❌ | ❌ |

### AppImage

Download the x86_64 package from the [development build](https://github.com/yrmuq/tenka/actions/workflows/dev.yml). Open the `.AppImage` with [Gear Lever](https://flathub.org/apps/it.mijorus.gearlever). Linux desktop file dialogs use the desktop portal. File drops work on native Wayland and X11. X11 sessions require the system libxkbcommon-x11 library. The `.zsync` file supplies delta-update metadata for compatible update tools.

### Flatpak

Flatpak packaging is planned.

---

## License & Attribution

Tenka is distributed under the [GNU General Public License, version 3](LICENSE).

| Component | Upstream | License | Integration Details |
| --- | --- | --- | --- |
| egui / eframe | [emilk/egui](https://github.com/emilk/egui) | `MIT OR Apache-2.0` | Desktop interface. |
| Winit | [rust-windowing/winit](https://github.com/rust-windowing/winit) | `Apache-2.0` | Desktop windows with a [native Wayland file-drop backport](https://github.com/MixaDoDs/photocraft/commit/3f766332a7212f2a5fe5f6b3da7ee12e4ab2f05d). |
| jpegxl-rs | [inflation/jpegxl-rs](https://github.com/inflation/jpegxl-rs) | `GPL-3.0-or-later` | Rust bindings for JPEG XL. |
| libjxl | [libjxl/libjxl](https://github.com/libjxl/libjxl) | `BSD-3-Clause` | JPEG XL codec. |
| libwebp | [webmproject/libwebp](https://github.com/webmproject/libwebp) | `BSD-3-Clause` | WebP encoder for conversion and comparison. |
| SSIMULACRA2 | [rust-av/ssimulacra2](https://github.com/rust-av/ssimulacra2) | `BSD-2-Clause` | Perceptual quality metric. |
