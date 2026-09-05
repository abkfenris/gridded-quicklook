# App icon sources

The GridLook app icon
([#14](https://github.com/abkfenris/gridded-quicklook/issues/14)): an
isometric data cube with gridded blue and orange side faces, a contoured
scalar field on top that warms toward the orange face, and a Quick Look
loupe over the back-right corner.

| File                 | What it is                                                                                              |
| -------------------- | ------------------------------------------------------------------------------------------------------- |
| `generate.py`        | Emits the three SVGs below. Plain Python, no dependencies.                                              |
| `gridlook.svg`       | The full artwork on Apple's macOS icon template (824 px rounded rectangle on a 1024 canvas, with shadow). |
| `gridlook-small.svg` | Reduced artwork for the 16 and 32 pt slots: two contour bands, no isolines, 2x2 grid, heavier outlines, bigger loupe. |
| `gridlook-bare.svg`  | The cube and loupe alone on a transparent canvas, for an Icon Composer layer or a document-icon badge.  |
| `appiconset.mjs`     | Renders the SVGs into `apple/App/Assets.xcassets/AppIcon.appiconset` and writes its `Contents.json`.    |

The top face is a real contour plot: a scalar field built from a few
anisotropic Gaussian bumps (`FIELD` in `generate.py`), sampled on a grid,
contoured with a small marching-squares routine, and projected onto the
isometric face. Moving a bump or a band level changes the map. Each SVG is
grouped into shadow, background, midground and foreground layers so it can
be split into an Icon Composer `.icon` bundle for macOS 26 later.

## Regenerating the icon set

```sh
mise run icons
```

runs `generate.py` and then `appiconset.mjs`. The latter rasterizes with
headless Chromium via Playwright, so it needs node and a playwright install
on the module path (`npx --yes playwright@1.56 install chromium` once). It
fills every macOS slot, 16 to 512 pt at 1x and 2x, using the small artwork
for 16 and 32 pt and the full artwork from 128 pt up. The PNGs are
committed, so this only needs re-running after the sources change.

The design history (the other candidates, the comparison sheets) lives in
the git log of the `docs/icon-options` directory this replaced.
