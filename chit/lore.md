## Protected Areas

- Keep image decoding, automatic parameter selection, and quality measurements in the GUI-independent Rust library so other native apps and container jobs can reuse the same conversion behavior.

## Required Behavior

- Keep the user's chosen output format throughout automatic conversion.
- Keep input dimensions unless the user enables automatic resolution reduction.
- Keep original files and existing output files intact when saving conversions.
- Judge compression and resizing against the same original at a common scale so that the search accounts for detail lost during resizing.
- Run the same automatic conversion search in the Tests tab so its comparisons reflect what the conversion engine can produce at the selected quality.
- Name each saved test result with its requested quality as the final part of the file stem, including when resolving name collisions.
