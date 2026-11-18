# Repository map

- `crates/tenka-core/src/lib.rs` exposes automatic conversion policy and the JPEG XL/WebP codec and measurement API.
- `crates/tenka-core/src/raster.rs` handles input validation, color and orientation normalization, resizing, and reference-scale SSIMULACRA2 scoring.
- `crates/tenka-gui/src/main.rs` implements both `eframe` tabs and routes interactive work through background workers.
- `crates/tenka-gui/src/jobs.rs` bridges the desktop interface to the library and handles file I/O.
- `crates/tenka-gui/src/language.rs` loads the English and Russian catalogs from `languages/`.
- `packaging/` builds the AppImage and update metadata. `.github/workflows/dev.yml` runs tests and updates the development draft.
