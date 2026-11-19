# Winit 0.30.13 and native Wayland file drops

The library sources and Apache-2.0 license are from [Winit 0.30.13](https://github.com/rust-windowing/winit/tree/v0.30.13). The three Wayland source changes are from [MixaDoDs/photocraft at 3f76633](https://github.com/MixaDoDs/photocraft/commit/3f766332a7212f2a5fe5f6b3da7ee12e4ab2f05d), also Apache-2.0. The vendored manifest uses the published crate's dependency declarations and includes the library target.

Remove this directory and the Cargo patch when eframe forwards native Wayland file drops upstream.
