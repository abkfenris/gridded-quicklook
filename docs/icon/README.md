# App icon sources

The ndLook app icon
([#14](https://github.com/abkfenris/gridded-quicklook/issues/14)): an
isometric data cube with gridded blue and orange side faces, a contoured
scalar field on top that warms toward the orange face, and a Quick Look
loupe over the back-right corner.

| File                 | What it is                                                                                              |
| -------------------- | ------------------------------------------------------------------------------------------------------- |
| `generate.py`        | Emits the four SVGs below. Plain Python, no dependencies.                                               |
| `ndlook.svg`       | The full artwork on Apple's macOS icon template (824 px rounded rectangle on a 1024 canvas, with shadow). |
| `ndlook-small.svg` | Reduced artwork for the 16 and 32 pt slots: two contour bands, no isolines, 2x2 grid, heavier outlines, bigger loupe. |
| `ndlook-bare.svg`  | The cube and loupe alone on a transparent canvas, for an Icon Composer layer.                           |
| `ndlook-badge.svg` | The cube alone, no loupe, filling the canvas: the badge on the `.zarr` and `.icechunk` document icons.  |
| `assets.mjs`         | Renders the SVGs into `apple/App/Assets.xcassets` (the `AppIcon` icon set and the `DocumentBadge` image set) and writes their `Contents.json`. |

The top face is a real contour plot: a scalar field built from a few
anisotropic Gaussian bumps (`FIELD` in `generate.py`), sampled on a grid,
contoured with a small marching-squares routine, and projected onto the
isometric face. Moving a bump or a band level changes the map. Each SVG is
grouped into shadow, background, midground and foreground layers so it can
be split into an Icon Composer `.icon` bundle for macOS 26 later.

## Document icons

`.zarr` and `.icechunk` stores get a composed document icon rather than a
hand-drawn one: `UTTypeIcons` on the two exported UTIs in
`apple/App/Info.plist` names the `DocumentBadge` image set as the centre
badge and gives each type a short label (ZARR, ICECHUNK). macOS 11 and
later draws the standard document shape, scales the badge onto it and
renders the label, and adapts the result to dark mode and to the macOS 26
document style on its own. The badge is the cube without the loupe, in the
reduced styling, since it is shown small. NetCDF and HDF5 files keep their
system icons on purpose: those UTIs are imported, not exported, so this
app does not claim them.

## Regenerating the assets

```sh
mise run icons
```

runs `generate.py` and then `assets.mjs`. The latter rasterizes with
headless Chromium via Playwright, so it needs node and a playwright install
on the module path (`npx --yes playwright@1.56 install chromium` once). It
fills every macOS app-icon slot, 16 to 512 pt at 1x and 2x, using the
small artwork for 16 and 32 pt and the full artwork from 128 pt up, and
writes the badge image set at 1x and 2x. The PNG files are committed, so this
only needs re-running after the sources change.

The design history (the other candidates, the comparison sheets) lives in
the git log of the `docs/icon-options` directory this replaced.
