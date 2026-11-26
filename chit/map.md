# Repository map

- `crates/tenka-core/src/lib.rs` exposes automatic conversion policy and the JPEG XL/WebP codec and measurement API.
- `crates/tenka-core/src/raster.rs` handles input validation, color and orientation normalization, resizing, and reference-scale SSIMULACRA2 scoring.
- `crates/tenka-core/src/jxl_header.rs` validates JPEG XL dimensions, bit depth, and animation before decoding files for comparison.
- `crates/tenka-gui/src/main.rs` implements the conversion window and routes batch work through a background worker.
- `crates/tenka-gui/src/jobs.rs` bridges the desktop interface to the library, scans imported folders, probes queued file sizes and dimensions, and handles file saving and file-manager requests.
- `crates/tenka-gui/src/estimate.rs` estimates remaining batch time from input byte sizes, pixel counts, and recent conversion timings.
- `crates/tenka-gui/src/summary.rs` accumulates conversion counts, saved output sizes, and active time across pause and resume.
- `crates/tenka-gui/src/comparison.rs` runs the comparison window and synchronizes zoom and pan between full-resolution images.
- `crates/tenka-gui/src/language.rs` loads the English and Russian catalogs from `languages/`.
- `crates/tenka-gui/src/theme.rs` follows system appearance through the Linux settings portal for both windows.
- `crates/tenka-gui/src/icons.rs` draws the language, appearance, and comparison buttons.
- `crates/tenka-gui/src/file_drop.rs` combines platform file drops with native Wayland drops from the clipboard worker in `vendor/smithay-clipboard`.
- `packaging/` builds the AppImage and update metadata. `.github/workflows/dev.yml` runs tests and updates the development draft.
