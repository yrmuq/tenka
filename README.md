<div align="center">
<img src="packaging/io.github.yrmuq.tenka.png" width="112" alt="Tenka logo">

# Tenka

</div>

---

> [!NOTE]
> Tenka is under active development. Behavior may change, bugs may occur, and saved settings may become outdated.

---

Tenka is a Linux image optimizer. It automatically selects compression to save smaller images while keeping them visually close to their originals. The engine is a separate Rust library for reuse in desktop apps and container workloads.

---

## Features

- Batch conversion of still, 8-bit JPEG, PNG, and WebP images up to 36 megapixels to JPEG XL or WebP.
- JPEG-to-JPEG XL conversion up to 80 megapixels. Best uses reversible transcoding above 36 megapixels. Balance and Cheap also evaluate lossy encodes at the original resolution.
- Best, Balance, and Cheap conversion modes that preserve image dimensions.
- Lossless encoding of prepared pixels, with source orientation applied and supported ICC profiles converted to sRGB.
- Full-resolution image comparison up to 36 megapixels with shared zoom and pan.
- English and Russian interface with system, light, and dark themes.

---

<details>
<summary><h2>Controls &amp; Shortcuts</h2></summary>

On Wayland, use **+** to add files or folders in either window.

| Input | Action |
| --- | --- |
| Drop files or folders, or select **+** | Add source images to the conversion queue. A folder import includes supported images in its subfolders. |
| Format selector in the header | Choose JPEG XL or WebP. |
| Mode selector in the header | **Best** prioritizes similarity. **Balance** weighs similarity and proportional size savings. **Cheap** picks the smallest result that passes the perceptual and local detail checks. |
| Select **Output** | Choose where to save converted images. Results from an imported folder keep its name and subfolder structure in this destination. |
| Select **Convert** | Save results in the chosen format and mode beside the originals or in the chosen folder. Existing files are kept. |
| Select **Cancel** during conversion | Stop the batch. Select **Convert** to continue with the pending files. |
| Finish a conversion queue | See file counts, size savings, and time spent converting. Open the error panel or the common destination folder when available. |
| Select **Clear** | Remove all entries from the conversion queue and reset the output folder. |
| Select the red **!** in the footer | View conversion errors. Select an error to jump to its queue row, or select the share icon to copy all errors. |
| Select the red cross on a row | Remove that entry from the conversion queue. |
| Select the split circle on a completed row | Compare the original on the left with the converted image on the right. |
| Drag the divider between **File** and **Output** | Resize both columns. |
| Select a folder icon in **File** or **Output** | Open the containing folder with the original or converted file selected. The **Output** size context menu also reveals the converted file. |
| Select **Compare** | Drop files into the comparison window or select **+**. Press **Mode** to switch between a shared split view and separate side-by-side images. Scroll to zoom at the pointer and drag with the left mouse button to pan together. |
| Select the globe | Toggle between the system language and English. |
| Select the T-shirt | Cycle through system appearance, light, and dark. The shirt shows an **A** in system mode. |
| Select the information icon | Open About to see the installed version, project links, and components. **Check for updates** checks for a newer stable release on GitHub. |

Tenka looks for results smaller than the source and caps the output at the full-resolution lossless size. If no smaller JPEG XL meets the mode's quality checks, it saves the smallest acceptable tested JPEG XL and marks any size increase next to the output size. WebP conversion skips files without a smaller result. Tenka compares each result with the original at the same scale. Automatic conversion also checks local pixel differences to catch loss of fine detail. For large images, Tenka estimates quality from a full-frame preview and several native-scale crops.

During conversion, the footer estimates the remaining time from up to ten completed files in the current batch. It scales reading time by input bytes and processing time by image pixel count. The queue reads file sizes and image headers in the background and uses the per-file average while either measurement is unavailable.

</details>

---

## Installation & Usage

### Architecture & Format Support

| Format | x86_64 | arm64 |
| --- | --- | --- |
| AppImage | ✅ | ❌ |
| Flatpak | ❌ | ❌ |

### Release scheme

Stable versions follow `X.Y.0`. Versions `X.Y.Z` with `Z > 0` are automatic dependency prereleases. See the [latest stable release](https://github.com/yrmuq/tenka/releases/latest) or the [full release list](https://github.com/yrmuq/tenka/releases).

### AppImage

Download the x86_64 `.AppImage` from the [stable download page](https://github.com/yrmuq/tenka/releases/latest) or try an [AppImage development build](https://github.com/yrmuq/tenka/actions/workflows/dev.yml). AppImages are built on Ubuntu 24.04 LTS. File dialogs use the Linux desktop portal. X11 sessions require the system `libxkbcommon-x11` library.

[Gear Lever](https://flathub.org/apps/it.mijorus.gearlever) can install and integrate the stable AppImage and apply supported updates. Import the `.AppImage` in Gear Lever. The stable AppImage embeds update metadata, and its companion `.zsync` enables delta updates for compatible tools.

### Flatpak

Flatpak packaging is planned.

---

## License & Attribution

Tenka is distributed under the [GNU General Public License, version 3](LICENSE).

| Component | Upstream | License | Integration Details |
| --- | --- | --- | --- |
| egui / eframe | [emilk/egui](https://github.com/emilk/egui) | `MIT OR Apache-2.0` | Desktop interface. |
| Smithay Clipboard | [Smithay/smithay-clipboard](https://github.com/Smithay/smithay-clipboard) | `MIT` | Wayland clipboard. |
| jpegxl-rs | [inflation/jpegxl-rs](https://github.com/inflation/jpegxl-rs) | `GPL-3.0-or-later` | Rust bindings for JPEG XL. |
| libjxl | [libjxl/libjxl](https://github.com/libjxl/libjxl) | `BSD-3-Clause` | JPEG XL codec. |
| libwebp | [webmproject/libwebp](https://github.com/webmproject/libwebp) | `BSD-3-Clause` | WebP encoder for conversion and comparison. |
| SSIMULACRA2 | [rust-av/ssimulacra2](https://github.com/rust-av/ssimulacra2) | `BSD-2-Clause` | Perceptual quality metric. |
| ureq | [algesten/ureq](https://github.com/algesten/ureq) | `MIT OR Apache-2.0` | HTTPS checks for updates. |
