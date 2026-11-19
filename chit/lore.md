## Protected Areas

- Keep image decoding, automatic parameter selection, and quality measurements in the GUI-independent Rust library so other native apps and container jobs can reuse the same conversion behavior.

## Required Behavior

- Keep the user's chosen output format throughout automatic conversion.
- Keep input dimensions unless the user enables automatic resolution reduction.
- Keep original files and existing output files intact when saving conversions.
- Limit saved outputs to the selected format's full-resolution lossless size and require each saved output to be strictly smaller than its source.
- In Best mode, choose the highest similarity among eligible outputs, breaking ties in favor of the smaller file.
- Judge compression and resizing against the same original at a common scale so that the search accounts for detail lost during resizing.
- Keep the comparison window's zoom and pan tied to the same relative image coordinates at every rendered resolution.
- In Split mode, draw every image into one shared frame and clip each image to its assigned region.

## Compatibility Workarounds

- Use XWayland for both windows when X11 is available because Winit 0.30 does not deliver native Wayland file drops. Remove the fallback when Winit supports native Wayland file drops.
