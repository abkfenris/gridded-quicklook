# App icon candidates

Design options for the GridLook app icon
([#14](https://github.com/abkfenris/gridded-quicklook/issues/14)), and the
sources of the one that was picked: A7 is the app icon, generated into
`apple/App/Assets.xcassets/AppIcon.appiconset/` from the SVGs here (see
"The app icon" below). The rest are kept for comparison.

![All candidates at 128, 64, 32 and 16 px on light and dark grounds](comparison.png)

## The options

All of them riff on the Gemini concept attached to the issue (an isometric
gridded data cube with a colour field on top and a Quick Look loupe), pared
down to what survives 16 px.

| File                      | Motif                                                                                                                                                                        | Reads at 16 px as                     |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| `a-cube-loupe.svg`        | Closest to the concept: gridded blue/orange side faces, a colour field on top, a white loupe over the top-right corner. Light ground. The play button is dropped (it says "media player"). First draft: the top is three concentric bands, which reads as a bullseye; kept for reference. | Blue/orange cube with a white ring    |
| `a2-cube-loupe-dark.svg`  | Same artwork on a dark navy ground, for comparison against the other dark options and against the dark Dock.                                                                | Same, on navy                         |
| `a3-anomaly.svg`          | A-series, contoured top: a diverging anomaly map on a cream face. Warm bands in the right face's oranges, cold bands in the left face's blues, thin isolines. Like a sea-surface-temperature anomaly. | Cube with a cream top                 |
| `a4-twin-peaks.svg`       | A-series, contoured top: a sequential field from deep blue through sand to orange. The warm peak sits in the front-right half, toward the orange face, and the field cools toward the back-left corner above the blue face. | Cube with a blue/orange top           |
| `a5-gridded-field.svg`    | A-series, contoured top: a broad warm ridge across a blue field, with the 3x3 grid faintly continued over the top so all three faces read as gridded.                       | Cube with a mostly blue top           |
| `a6-isolines.svg`         | A-series, contoured top: the A3 field as unfilled isolines only, orange and blue on cream. Topographic-map look; at 16 px the top is plain cream.                            | Cube with a cream top                 |
| `a7-warm-edge.svg`        | A4 pushed further: the warm peak hugs the right edge so the orange bands run into the orange face along their shared edge, with a small island and deep blue at the back-left. The loupe is ink with a thin white edge so it separates from both faces. | Cube with a blue/orange top and a dark ring |
| `a7-warm-edge-small.svg`  | A7's reduced artwork for the 16 and 32 px slots: two contour bands and no isolines, a 2x2 grid, heavier outlines, a bigger loupe. See "Detail per size" below.             | Same, bolder                          |
| `a7-warm-edge-bare.svg`   | A7 without the rounded rectangle or its shadow: the cube and loupe alone on a transparent canvas, scaled up. For an Icon Composer layer, a document icon, or the README.    | Cube and ring, no tile                |
| `b-tiles-loupe.svg`       | Flat 4x4 field of rounded chunk tiles in a stepped cool-to-warm ramp, with the loupe zooming a 2x2 block. The "chunked array" reading of Zarr/Icechunk, no perspective.      | Coloured grid with a white circle     |
| `c-globe.svg`             | Graticule globe (parallels and meridians) with one grid cell lit orange. The earth-science reading; no loupe.                                                               | Blue disc with white lines            |
| `d-bold-cube.svg`         | Isometric cube with a 3x3 grid on every face and one hot chunk per face. Boldest silhouette; risks a Rubik's cube association.                                              | Three-colour cube                     |
| `e-slices.svg`            | Three gridded slabs stacked in perspective (time steps or chunks along a third dimension), the top one carrying a hot region. The "multi-dimensional" reading.              | Stack of diamonds                     |

Rendered 1024 px masters for each live in `masters/`; the one that gets
picked is the `AppIcon` master, as is.

## Detail per size

A macOS `AppIcon.appiconset` is not limited to one 1024 master: Xcode
accepts separate PNGs for 16, 32, 128, 256 and 512 pt at 1x and 2x, and
Finder picks the closest. So the artwork can change with size. The
`-small` variants are drawn for the 16 and 32 pt slots (16, 32 and 64 px
files): fewer contour bands, no isolines, a 2x2 grid instead of 3x3,
heavier outlines, a bigger loupe. The full artwork covers 128 pt and up.
The comparison sheet's "(multi-size)" row shows the combination as Finder
would display it.

## What every option shares

- **macOS shape.** 1024 canvas, 824 px rounded rectangle (radius 185.4)
  centred on a transparent ground, with the template's drop shadow, so the
  icon sits level with system icons in the Dock.
- **Flat, few colours, no text.** One navy/ink, one blue, one teal, one
  orange, one sand, plus white. No gradients; the "colour field" on the cube
  tops is three flat bands.
- **Layered.** Each SVG is grouped into `layer-shadow`, `layer-background`,
  `layer-midground`, and (where there is a loupe) `layer-foreground`, so the
  same artwork can be split into an Icon Composer `.icon` bundle for
  macOS 26 later, with the flat 1024 PNG as the fallback for macOS 13 to 15.

## Regenerating

The SVGs are emitted by `generate.py` (plain Python, no dependencies) so a
colour or geometry tweak is a one-line change. The contoured top faces are
real contours: each is a scalar field built from a few anisotropic Gaussian
bumps (`FIELD_*` at the top of the script), sampled on a grid, contoured
with a small marching-squares routine, and projected onto the isometric
face. Moving a bump or a level changes the map.

```sh
python3 docs/icon-options/generate.py
```

`render.mjs` rasterizes them with headless Chromium via Playwright (the
1024 masters into `masters/`, the smaller sizes into the untracked
`renders/`) and rebuilds `comparison.html` / `comparison.png`:

```sh
npx --yes playwright@1.56 install chromium   # once
node docs/icon-options/render.mjs
```

## The app icon

A7 (`a7-warm-edge`) is the app icon. `appiconset.mjs` renders it into
every macOS slot of `apple/App/Assets.xcassets/AppIcon.appiconset/` and
writes the catalog's `Contents.json`, using the `-small` artwork for the
16 and 32 pt slots and the full artwork from 128 pt up. `mise run icons`
runs `generate.py` and then that script; the resulting PNGs are committed,
so it only needs re-running after the sources change. `apple/project.yml`
sets `ASSETCATALOG_COMPILER_APPICON_NAME: AppIcon`, and the catalog is
picked up as a resource because it sits under the `App` source path.

Still open: `.zarr` / `.icechunk` document icons derived from the same
artwork, referenced with `UTTypeIconFile` on the exported UTIs in
`apple/App/Info.plist`. `a7-warm-edge-bare.svg` is the starting point for
those.
