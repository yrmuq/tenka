## Protected Areas

- Keep image decoding, automatic parameter selection, and quality measurements in the GUI-independent Rust library so other native apps and container jobs can reuse the same conversion behavior.

## Required Behavior

- Keep the user's chosen output format throughout automatic conversion.
- Keep input dimensions in the desktop conversion window while automatic resolution reduction is hidden.
- Keep original files and existing output files intact when saving conversions.
- Limit saved outputs to the selected format's full-resolution lossless size.
- Use reversible transcoding alone for large JPEG-to-JPEG XL jobs in Best mode, and keep it as the lossless fallback when Balance or Cheap tests lossy encodes.
- If desktop conversion finds no smaller JPEG XL, save the smallest tested JPEG XL that meets the selected mode's quality checks and flag any size increase.
- Require each saved WebP to be strictly smaller than its source.
- In Best mode, choose the highest similarity among smaller outputs, breaking ties in favor of the smaller file.
- Judge compression and resizing against the same original at a common scale so that the search accounts for detail lost during resizing.
- Keep the comparison window's zoom and pan tied to the same relative image coordinates at every rendered resolution.
- In Split mode, draw every image into one shared frame and clip each image to its assigned region.

## Compatibility Workarounds

- Apply the main window resize constraints after creation, since Winit 0.30's fallback Wayland decorations are created after the startup size hints.
- Keep the packaged desktop ID and Wayland window IDs identical so the compositor can look up the icon while Winit lacks support for setting a native Wayland window icon.
