#!/usr/bin/env python3
"""Emit the GridLook app icon as SVG files next to this script.

The icon is an isometric data cube: gridded blue and orange side faces, a
contoured scalar field on top that warms toward the orange face, and a
Quick Look loupe over the back-right corner. Four files come out:

  gridlook.svg        the full artwork on Apple's macOS icon template
                      (824 px rounded rectangle centred on a 1024 canvas,
                      with the template's drop shadow)
  gridlook-small.svg  reduced artwork for the 16 and 32 pt icon slots:
                      two contour bands, no isolines, a 2x2 grid, heavier
                      outlines, a bigger loupe
  gridlook-bare.svg   the cube and loupe alone on a transparent canvas,
                      for an Icon Composer layer
  gridlook-badge.svg  the cube alone, no loupe, filling the canvas: the
                      badge the system composes onto the .zarr and
                      .icechunk document icons (UTTypeIcons in
                      apple/App/Info.plist)

Each SVG is grouped into `<g id="layer-...">` groups (shadow, background,
midground, foreground) so the artwork can be split into an Icon Composer
`.icon` bundle for macOS 26 later, with the flat PNGs as the fallback for
macOS 13 to 15. No text, no gradients, a short flat palette.

Run it directly (no dependencies), then `node assets.mjs` to render the
asset catalog. The design history is in the git log of the
docs/icon-options directory this replaced.
"""

from __future__ import annotations

import math
from pathlib import Path

HERE = Path(__file__).resolve().parent

# --- macOS icon template ----------------------------------------------------
CANVAS = 1024
ICON = 824
INSET = (CANVAS - ICON) / 2  # 100
RADIUS = 185.4  # Apple's macOS template corner radius at 1024 px

# --- palette ----------------------------------------------------------------
NAVY = "#1B3A5C"  # outlines on the light ground
INK = "#0F2540"  # loupe, isolines
BLUE = "#3E86C6"
BLUE_DEEP = "#2C6AA6"
ORANGE = "#E89B4B"
ORANGE_DEEP = "#D07E2E"
SAND = "#F6C77A"
WHITE = "#FFFFFF"
PALE = "#E6F0F7"  # tile background


def svg_open(title: str) -> str:
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" '
        f'width="{CANVAS}" height="{CANVAS}" viewBox="0 0 {CANVAS} {CANVAS}">\n'
        f"  <title>{title}</title>\n"
        "  <defs>\n"
        '    <filter id="dock-shadow" x="-10%" y="-10%" width="120%" height="130%">\n'
        '      <feGaussianBlur in="SourceAlpha" stdDeviation="12"/>\n'
        '      <feOffset dy="10" result="b"/>\n'
        '      <feComponentTransfer><feFuncA type="linear" slope="0.3"/></feComponentTransfer>\n'
        "    </filter>\n"
    )


def svg_close() -> str:
    return "</svg>\n"


def shadow_layer() -> str:
    """Apple's template drop shadow, kept on its own layer so it can be dropped."""
    return (
        '  <g id="layer-shadow">\n'
        f'    <rect x="{INSET}" y="{INSET}" width="{ICON}" height="{ICON}" rx="{RADIUS}" fill="#000" filter="url(#dock-shadow)"/>\n'
        "  </g>\n"
    )


def background_layer(fill: str) -> str:
    return (
        '  <g id="layer-background">\n'
        f'    <rect x="{INSET}" y="{INSET}" width="{ICON}" height="{ICON}" rx="{RADIUS}" fill="{fill}"/>\n'
        "  </g>\n"
    )


def pts(points) -> str:
    return " ".join(f"{x:.1f},{y:.1f}" for x, y in points)


def lerp(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


# --- isometric cube ---------------------------------------------------------


def iso_cube(cx: float, cy: float, s: float):
    """Vertices of an isometric cube with edge `s`, centred on (cx, cy).

    Returns (top, left, right) faces as point lists. The top face is
    [n, e, sv, wv]: back, right, front and left corners.
    """
    w = s * math.cos(math.radians(30))
    h = s * math.sin(math.radians(30))
    top_y = cy - s
    n = (cx, top_y)
    e = (cx + w, top_y + h)
    sv = (cx, top_y + 2 * h)  # front-top vertex
    wv = (cx - w, top_y + h)
    down = lambda p: (p[0], p[1] + s)  # noqa: E731
    top = [n, e, sv, wv]
    left = [wv, sv, down(sv), down(wv)]
    right = [sv, e, down(e), down(sv)]
    return top, left, right


def face_grid_lines(face, divisions: int):
    """Grid lines for a parallelogram face given as [p0, p1, p2, p3]."""
    p0, p1, p2, p3 = face
    lines = []
    for k in range(1, divisions):
        t = k / divisions
        lines.append((lerp(p0, p1, t), lerp(p3, p2, t)))
        lines.append((lerp(p0, p3, t), lerp(p1, p2, t)))
    return lines


def polyline_group(lines, stroke, width, opacity=1.0) -> str:
    out = []
    for a, b in lines:
        out.append(
            f'    <line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}" '
            f'stroke="{stroke}" stroke-width="{width:.1f}" stroke-linecap="round" opacity="{opacity}"/>\n'
        )
    return "".join(out)


def loupe(cx, cy, r, ring, handle_len, stroke, outline, glass=None) -> str:
    """A magnifying glass: thick ring plus a handle to the lower right, with a
    thin `outline` edge so the ring separates from whatever is behind it."""
    ang = math.radians(45)
    hx0, hy0 = cx + (r + ring / 2) * math.cos(ang), cy + (r + ring / 2) * math.sin(ang)
    hx1, hy1 = cx + (r + handle_len) * math.cos(ang), cy + (r + handle_len) * math.sin(ang)
    out = []
    if glass:
        out.append(f'    <circle cx="{cx:.1f}" cy="{cy:.1f}" r="{r:.1f}" fill="{glass}"/>\n')
    for colour, extra in ((outline, 14), (stroke, 0)):
        out.append(
            f'    <line x1="{hx0:.1f}" y1="{hy0:.1f}" x2="{hx1:.1f}" y2="{hy1:.1f}" stroke="{colour}" '
            f'stroke-width="{ring * 1.35 + extra:.1f}" stroke-linecap="round"/>\n'
        )
        out.append(
            f'    <circle cx="{cx:.1f}" cy="{cy:.1f}" r="{r:.1f}" fill="none" stroke="{colour}" stroke-width="{ring + extra:.1f}"/>\n'
        )
    return "".join(out)


# --- scalar field and contours (pure Python marching squares) ---------------
#
# The top face carries a "variable": a scalar field built from a few
# anisotropic Gaussian bumps, contoured into filled bands. Computing real
# contours (rather than drawing concentric ellipses) is what makes it read
# as gridded data instead of a bullseye. Everything is in the top face's own
# (u, v) coordinates in [0, 1]^2 and projected onto the isometric face at the
# end, so the field looks like it is lying on the cube.


def gaussian(amp, cu, cv, su, sv, theta_deg):
    """An anisotropic Gaussian bump: amplitude, centre, sigmas, rotation."""
    th = math.radians(theta_deg)
    c, s = math.cos(th), math.sin(th)

    def f(u, v):
        du, dv = u - cu, v - cv
        a = (du * c + dv * s) / su
        b = (-du * s + dv * c) / sv
        return amp * math.exp(-(a * a + b * b))

    return f


def window(u, v, inner=0.05, outer=0.4):
    """1 inside the face (plus a margin), fading to 0 well outside it, so every
    contour closes inside the padded grid and the face clip does the rest."""

    def w(x):
        d = max(-x, x - 1.0, 0.0)  # distance outside [0, 1]
        if d <= inner:
            return 1.0
        if d >= outer:
            return 0.0
        t = (d - inner) / (outer - inner)
        return 0.5 * (1 + math.cos(math.pi * t))

    return w(u) * w(v)


def sample(bumps, n=80, lo=-0.4, hi=1.4):
    """Sample sum(bumps) * window on an n x n grid over [lo, hi]^2."""
    coords = [lo + (hi - lo) * i / (n - 1) for i in range(n)]
    grid = []
    for v in coords:
        row = []
        for u in coords:
            val = sum(b(u, v) for b in bumps) * window(u, v)
            row.append(val)
        grid.append(row)
    return coords, grid


# Marching squares. Corner bits: a=(i,j)=1, b=(i+1,j)=2, c=(i+1,j+1)=4,
# d=(i,j+1)=8. Edges are named by the cell edge they cross: T (a-b), R (b-c),
# B (d-c), L (a-d). Saddles (5, 10) are resolved with the cell centre value.
_CASES = {
    1: [("L", "T")],
    2: [("T", "R")],
    3: [("L", "R")],
    4: [("R", "B")],
    6: [("T", "B")],
    7: [("L", "B")],
    8: [("B", "L")],
    9: [("T", "B")],
    11: [("R", "B")],
    12: [("L", "R")],
    13: [("T", "R")],
    14: [("L", "T")],
}


def contours(coords, grid, level):
    """Closed contour polygons (lists of (u, v)) of `grid` at `level`."""
    n = len(coords)

    def edge_key(i, j, side):
        # A shared edge gets the same key from both cells that touch it.
        if side == "T":
            return ("h", i, j)
        if side == "B":
            return ("h", i, j + 1)
        if side == "L":
            return ("v", i, j)
        return ("v", i + 1, j)

    def edge_point(key):
        kind, i, j = key
        if kind == "h":
            v0, v1 = grid[j][i], grid[j][i + 1]
            t = (level - v0) / (v1 - v0)
            return (coords[i] + (coords[i + 1] - coords[i]) * t, coords[j])
        v0, v1 = grid[j][i], grid[j + 1][i]
        t = (level - v0) / (v1 - v0)
        return (coords[i], coords[j] + (coords[j + 1] - coords[j]) * t)

    segments = []
    for j in range(n - 1):
        for i in range(n - 1):
            a, b, c, d = grid[j][i], grid[j][i + 1], grid[j + 1][i + 1], grid[j + 1][i]
            case = (a >= level) | (b >= level) << 1 | (c >= level) << 2 | (d >= level) << 3
            if case in (0, 15):
                continue
            if case in (5, 10):
                centre_high = (a + b + c + d) / 4 >= level
                if (case == 5) == centre_high:
                    pairs = [("T", "R"), ("B", "L")]
                else:
                    pairs = [("L", "T"), ("R", "B")]
            else:
                pairs = _CASES[case]
            for s0, s1 in pairs:
                segments.append((edge_key(i, j, s0), edge_key(i, j, s1)))

    # Link segments into loops via their shared edge keys.
    by_key = {}
    for idx, (k0, k1) in enumerate(segments):
        by_key.setdefault(k0, []).append(idx)
        by_key.setdefault(k1, []).append(idx)
    used = [False] * len(segments)
    polygons = []
    for start in range(len(segments)):
        if used[start]:
            continue
        used[start] = True
        k_first, k = segments[start]
        loop = [k_first, k]
        while k != k_first:
            nxt = next((s for s in by_key[k] if not used[s]), None)
            if nxt is None:
                break  # open polyline (should not happen with the window)
            used[nxt] = True
            k0, k1 = segments[nxt]
            k = k1 if k0 == k else k0
            loop.append(k)
        polygons.append([edge_point(key) for key in loop])
    return polygons


def project(face, u, v):
    """Map top-face coordinates (u, v) in [0, 1]^2 onto the isometric face."""
    n, e, sv, wv = face
    return (
        n[0] + u * (e[0] - n[0]) + v * (wv[0] - n[0]),
        n[1] + u * (e[1] - n[1]) + v * (wv[1] - n[1]),
    )


def contour_path(face, polygons) -> str:
    parts = []
    for poly in polygons:
        pts_xy = [project(face, u, v) for u, v in poly]
        parts.append("M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in pts_xy) + " Z")
    return " ".join(parts)


def contour_field_svg(face, bumps, bands, line_color, line_width=6, line_opacity=0.45) -> str:
    """Filled contour bands (ascending levels painted on top of each other)
    plus thin isolines, for the top face. `bands` is [(level, colour), ...]."""
    coords, grid = sample(bumps)
    out = []
    for level, colour in bands:
        d = contour_path(face, contours(coords, grid, level))
        if not d:
            continue
        out.append(f'      <path d="{d}" fill="{colour}" fill-rule="evenodd"/>\n')
        if line_opacity > 0:
            out.append(
                f'      <path d="{d}" fill="none" stroke="{line_color}" stroke-width="{line_width}" '
                f'stroke-linejoin="round" opacity="{line_opacity}"/>\n'
            )
    return "".join(out)


# The field. Centres and sigmas are in face coordinates: u runs along the
# back-right edge (the one the orange face hangs from), v along the
# back-left edge (the blue face). The main peak hugs the u = 1 edge so the
# warm bands run into the orange face, and the field cools toward the
# back-left corner above the blue face.
FIELD = {
    "base": BLUE_DEEP,
    "bumps": [
        gaussian(1.0, 0.90, 0.72, 0.44, 0.24, 80),
        gaussian(0.7, 0.68, 0.18, 0.26, 0.15, -20),
        gaussian(0.35, 0.28, 0.62, 0.18, 0.11, 40),
    ],
    "bands": [(0.16, BLUE), (0.40, SAND), (0.62, ORANGE), (0.85, ORANGE_DEEP)],
}

# The same field with two bands and no island, for the 16 and 32 pt artwork.
FIELD_SMALL = {
    "base": BLUE_DEEP,
    "bumps": FIELD["bumps"][:2],
    "bands": [(0.22, SAND), (0.62, ORANGE_DEEP)],
}


# --- the icon ---------------------------------------------------------------


def icon(bare: bool = False, small: bool = False, with_loupe: bool = True) -> str:
    """Isometric data cube with a contoured field on top and a Quick Look
    loupe over the back-right corner.

    `bare` drops the rounded-rectangle background and its shadow and lets
    the cube use more of the canvas. `small` is the reduced artwork for the
    16 and 32 pt slots of the icon set: a coarser grid, heavier outlines, a
    bigger loupe, the two-band field. Without the loupe (`with_loupe=False`,
    only meaningful with `bare`) the cube is centred and fills the canvas:
    that is the document-icon badge.
    """
    if bare and not with_loupe:
        s, cx, cy = 480, 512, 512
    elif bare:
        s, cx, cy = 400, 500, 540
    else:
        s, cx, cy = 330, 500, 560
    top, left, right = iso_cube(cx, cy, s)
    n, e, sv, wv = top
    stroke_w = s / (11 if small else 15)
    grid_div = 2 if small else 3
    grid_w = s / (20 if small else 33)
    field = FIELD_SMALL if small else FIELD

    title = "GridLook icon" + (" (bare)" if bare else "") + (" (small sizes)" if small else "")
    if not with_loupe:
        title = "GridLook document badge"
    out = [svg_open(title)]
    out.append(f'    <clipPath id="top-face"><polygon points="{pts(top)}"/></clipPath>\n')
    out.append("  </defs>\n")
    if not bare:
        out.append(shadow_layer())
        out.append(background_layer(PALE))
    out.append('  <g id="layer-midground">\n')
    # side faces, then the field on the top face
    out.append(f'    <polygon points="{pts(left)}" fill="{BLUE}"/>\n')
    out.append(f'    <polygon points="{pts(right)}" fill="{ORANGE}"/>\n')
    out.append(f'    <polygon points="{pts(top)}" fill="{field["base"]}"/>\n')
    out.append('    <g clip-path="url(#top-face)">\n')
    out.append(contour_field_svg(top, field["bumps"], field["bands"], INK, line_opacity=0.0 if small else 0.45))
    out.append("    </g>\n")
    # grid lines on the two side faces only (the top face carries the field)
    out.append(polyline_group(face_grid_lines(left, grid_div), NAVY, grid_w, 0.5))
    out.append(polyline_group(face_grid_lines(right, grid_div), NAVY, grid_w, 0.5))
    # cube outline
    outline = [n, e, (e[0], e[1] + s), (sv[0], sv[1] + s), (wv[0], wv[1] + s), wv]
    out.append(
        f'    <polygon points="{pts(outline)}" fill="none" stroke="{NAVY}" stroke-width="{stroke_w:.1f}" stroke-linejoin="round"/>\n'
    )
    for a, b in ((wv, sv), (sv, e), (sv, (sv[0], sv[1] + s))):
        out.append(
            f'    <line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}" '
            f'stroke="{NAVY}" stroke-width="{stroke_w:.1f}" stroke-linecap="round"/>\n'
        )
    out.append("  </g>\n")
    # Loupe over the back-right corner, placed relative to that corner so it
    # follows the cube when the cube is rescaled. Ink ring, thin white edge.
    if with_loupe:
        out.append('  <g id="layer-foreground">\n')
        lx, ly = e[0] - 0.29 * s, e[1] - 0.20 * s
        r = s * (0.40 if small else 0.36)
        ring = s * (0.14 if small else 0.11)
        out.append(loupe(lx, ly, r, ring, 0.48 * s, stroke=INK, outline=WHITE, glass="rgba(255,255,255,0.18)"))
        out.append("  </g>\n")
    out.append(svg_close())
    return "".join(out)


OUTPUTS = {
    "gridlook": lambda: icon(),
    "gridlook-small": lambda: icon(small=True),
    "gridlook-bare": lambda: icon(bare=True),
    # The badge is composed onto a document shape and shown small, so it
    # uses the reduced (2x2 grid, two-band) styling.
    "gridlook-badge": lambda: icon(bare=True, small=True, with_loupe=False),
}


def main() -> None:
    for name, fn in OUTPUTS.items():
        path = HERE / f"{name}.svg"
        path.write_text(fn())
        print(f"wrote {path.relative_to(HERE.parent.parent)}")


if __name__ == "__main__":
    main()
