## Protected Areas

- Keep image decoding, automatic parameter selection, and quality measurements in the GUI-independent Rust library so other native apps and container jobs can reuse the same conversion behavior.

## Required Behavior

- Keep input dimensions by default and reduce them only with a maximum edge or when the user explicitly allows resizing for a byte target.
- Keep original files and existing output files intact when saving conversions.
- Report whether a byte target was met and preserve the selected quality floor when the target is too small.
