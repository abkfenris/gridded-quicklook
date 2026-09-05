#!/usr/bin/env python3
"""Emit the GridLook app-icon candidates as SVG files next to this script.

Every candidate is a 1024x1024 canvas following Apple's macOS app-icon
template: an 824x824 rounded rectangle (corner radius ~185) centred on a
transparent canvas, so it sits level with system icons in the Dock. Each
file is organised as a few `<g id="layer-...">` groups (background,
midground, foreground) so the same artwork can later be split into an
Icon Composer `.icon` bundle for macOS 26, while the flat render is the
fallback for macOS 13 to 15.

No text, no photographic gradients, and a short flat palette: everything
has to survive being shrunk to 16 px in the Finder sidebar.

Run it directly (no dependencies) to regenerate the SVGs, then
`node render.mjs` to rasterize them and build the comparison sheet.
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

# --- palette (borrowed from the Gemini concept in issue #14) ----------------
NAVY = "#1B3A5C"  # outlines
INK = "#0F2540"  # dark backgrounds
BLUE = "#3E86C6"
BLUE_DEEP = "#2C6AA6"
TEAL = "#3FA9A0"
TEAL_DEEP = "#2E877F"
ORANGE = "#E89B4B"
ORANGE_DEEP = "#D07E2E"
SAND = "#F6C77A"
CREAM = "#F4EFE6"
WHITE = "#FFFFFF"
PALE = "#E6F0F7"  # light background


def svg_open(title: str) -> str:
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" '
        f'width="{CANVAS}" height="{CANVAS}" viewBox="0 0 {CANVAS} {CANVAS}">\n'
        f"  <title>{title}</title>\n"
        "  <defs>\n"
        f'    <clipPath id="icon-shape"><rect x="{INSET}" y="{INSET}" width="{ICON}" height="{ICON}" rx="{RADIUS}"/></clipPath>\n'
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


# --- isometric cube helpers -------------------------------------------------


def iso_cube(cx: float, cy: float, s: float):
    """Vertices of an isometric cube with edge `s`, centred on (cx, cy).

    Returns (top, left, right) faces as point lists, each starting at the
    shared front vertex where useful.
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
            f'stroke="{stroke}" stroke-width="{width}" stroke-linecap="round" opacity="{opacity}"/>\n'
        )
    return "".join(out)


def loupe(cx, cy, r, ring, handle_len, stroke=WHITE, outline=NAVY, glass=None) -> str:
    """A magnifying glass: thick ring plus a handle to the lower right."""
    ang = math.radians(45)
    hx0, hy0 = cx + (r + ring / 2) * math.cos(ang), cy + (r + ring / 2) * math.sin(ang)
    hx1, hy1 = cx + (r + handle_len) * math.cos(ang), cy + (r + handle_len) * math.sin(ang)
    out = []
    if glass:
        out.append(f'    <circle cx="{cx}" cy="{cy}" r="{r}" fill="{glass}"/>\n')
    # outline pass (slightly wider, in navy) then the white pass on top
    out.append(
        f'    <line x1="{hx0:.1f}" y1="{hy0:.1f}" x2="{hx1:.1f}" y2="{hy1:.1f}" stroke="{outline}" '
        f'stroke-width="{ring * 1.35 + 14}" stroke-linecap="round"/>\n'
    )
    out.append(f'    <circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{outline}" stroke-width="{ring + 14}"/>\n')
    out.append(
        f'    <line x1="{hx0:.1f}" y1="{hy0:.1f}" x2="{hx1:.1f}" y2="{hy1:.1f}" stroke="{stroke}" '
        f'stroke-width="{ring * 1.35}" stroke-linecap="round"/>\n'
    )
    out.append(f'    <circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{stroke}" stroke-width="{ring}"/>\n')
    return "".join(out)


# --- option A: the concept, simplified --------------------------------------


def option_a_cube_loupe(background: str = PALE, label: str = "A") -> str:
    """Isometric data cube with a contour top face and a Quick Look loupe.

    The closest reading of the Gemini concept: gridded side faces, a stepped
    colour field on top, a loupe over one corner. The play button is gone
    (it reads as a media app) and the field is three flat bands rather than
    a gradient so it survives 16 px.
    """
    s = 330
    cx, cy = 500, 560
    top, left, right = iso_cube(cx, cy, s)
    n, e, sv, wv = top
    outline_col = NAVY if background == PALE else INK
    out = [svg_open(f"GridLook icon, option {label}: data cube with loupe")]
    out.append(f'    <clipPath id="top-face"><polygon points="{pts(top)}"/></clipPath>\n')
    out.append("  </defs>\n")
    out.append(shadow_layer())
    out.append(background_layer(background))
    out.append('  <g id="layer-midground">\n')
    # faces
    out.append(f'    <polygon points="{pts(left)}" fill="{BLUE}"/>\n')
    out.append(f'    <polygon points="{pts(right)}" fill="{ORANGE}"/>\n')
    out.append(f'    <polygon points="{pts(top)}" fill="{TEAL}"/>\n')
    # Stepped contour field on the top face. A circle drawn on the cube's top
    # projects to an axis-aligned ellipse with ry/rx = tan 30deg, so nested
    # ellipses with that ratio look like contour rings lying *on* the face.
    k = math.tan(math.radians(30))
    tcx, tcy = (n[0] + sv[0]) / 2, (n[1] + sv[1]) / 2
    out.append('    <g clip-path="url(#top-face)">\n')
    for dx, dy, rx, fill in ((70, 20, 250, BLUE_DEEP), (70, 20, 180, ORANGE), (70, 20, 105, SAND)):
        out.append(f'      <ellipse cx="{tcx + dx}" cy="{tcy + dy}" rx="{rx}" ry="{rx * k:.1f}" fill="{fill}"/>\n')
    out.append("    </g>\n")
    # grid lines on the two side faces only (the top face carries the field)
    out.append(polyline_group(face_grid_lines(left, 3), outline_col, 10, 0.5))
    out.append(polyline_group(face_grid_lines(right, 3), outline_col, 10, 0.5))
    # cube outline
    outline = [n, e, (e[0], e[1] + s), (sv[0], sv[1] + s), (wv[0], wv[1] + s), wv]
    out.append(f'    <polygon points="{pts(outline)}" fill="none" stroke="{outline_col}" stroke-width="22" stroke-linejoin="round"/>\n')
    for a, b in ((wv, sv), (sv, e), (sv, (sv[0], sv[1] + s))):
        out.append(f'    <line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}" stroke="{outline_col}" stroke-width="22" stroke-linecap="round"/>\n')
    out.append("  </g>\n")
    out.append('  <g id="layer-foreground">\n')
    out.append(loupe(690, 330, 120, 36, 160, outline=outline_col, glass="rgba(255,255,255,0.18)"))
    out.append("  </g>\n")
    out.append(svg_close())
    return "".join(out)


# --- option B: flat tile field with loupe -----------------------------------

# A 4x4 field of chunk values (0..4) that reads as a warm blob on a cool field.
FIELD = [
    [0, 0, 1, 1],
    [0, 1, 2, 3],
    [1, 2, 4, 3],
    [1, 2, 3, 2],
]
RAMP = [INK, BLUE_DEEP, TEAL, ORANGE, SAND]


def option_b_tiles_loupe() -> str:
    out = [svg_open("GridLook icon, option B: chunked field with loupe")]
    lens_cx, lens_cy, lens_r = 620, 620, 170
    out.append(f'    <clipPath id="lens"><circle cx="{lens_cx}" cy="{lens_cy}" r="{lens_r}"/></clipPath>\n')
    out.append("  </defs>\n")
    out.append(shadow_layer())
    out.append(background_layer(NAVY))
    out.append('  <g id="layer-midground">\n')
    n = 4
    gap = 14
    area = 640
    x0 = y0 = (CANVAS - area) / 2
    tile = (area - gap * (n - 1)) / n
    for r, row in enumerate(FIELD):
        for c, v in enumerate(row):
            x = x0 + c * (tile + gap)
            y = y0 + r * (tile + gap)
            out.append(f'    <rect x="{x:.1f}" y="{y:.1f}" width="{tile:.1f}" height="{tile:.1f}" rx="22" fill="{RAMP[v]}"/>\n')
    out.append("  </g>\n")
    out.append('  <g id="layer-foreground">\n')
    # Magnified view inside the lens: the 2x2 block under it at 1.6x, with
    # the tile intersection nudged off the lens centre (a centred gap reads
    # as a crosshair rather than a zoomed-in field).
    out.append('    <g clip-path="url(#lens)">\n')
    out.append(f'      <circle cx="{lens_cx}" cy="{lens_cy}" r="{lens_r}" fill="{NAVY}"/>\n')
    zoom = 1.6
    big = tile * zoom
    bgap = gap * zoom
    bx0 = lens_cx - big - bgap / 2 - 45
    by0 = lens_cy - big - bgap / 2 - 45
    for r in range(2):
        for c in range(2):
            v = FIELD[2 + r][2 + c]
            x = bx0 + c * (big + bgap)
            y = by0 + r * (big + bgap)
            out.append(f'      <rect x="{x:.1f}" y="{y:.1f}" width="{big:.1f}" height="{big:.1f}" rx="44" fill="{RAMP[v]}"/>\n')
    out.append("    </g>\n")
    out.append(loupe(lens_cx, lens_cy, lens_r, 40, 150, outline=NAVY))
    out.append("  </g>\n")
    out.append(svg_close())
    return "".join(out)


# --- option C: graticule globe with a highlighted chunk ---------------------


def option_c_globe() -> str:
    out = [svg_open("GridLook icon, option C: graticule globe")]
    cx, cy, R = 512, 512, 300
    out.append(f'    <clipPath id="globe"><circle cx="{cx}" cy="{cy}" r="{R}"/></clipPath>\n')
    # The highlighted cell sits on the drawn graticule, front and centre so
    # it is not foreshortened: between the equator and the 30 degree
    # parallel, and between the central meridian and the 30 degree meridian
    # to its east.
    lat_a, lat_b = math.radians(0), math.radians(30)
    lon_b = math.radians(30)
    ya, yb = cy - R * math.sin(lat_a), cy - R * math.sin(lat_b)
    out.append(f'    <clipPath id="band"><rect x="{cx}" y="{yb:.1f}" width="{R}" height="{ya - yb:.1f}"/></clipPath>\n')
    out.append("  </defs>\n")
    out.append(shadow_layer())
    out.append(background_layer(INK))
    out.append('  <g id="layer-midground">\n')
    out.append(f'    <circle cx="{cx}" cy="{cy}" r="{R}" fill="{BLUE_DEEP}"/>\n')
    # highlighted chunk: the inside of the 30 degree meridian ellipse,
    # clipped to the latitude band and to the eastern half of the globe
    rxb = R * math.sin(lon_b)
    out.append('    <g clip-path="url(#globe)"><g clip-path="url(#band)">\n')
    out.append(f'      <ellipse cx="{cx}" cy="{cy}" rx="{rxb:.1f}" ry="{R}" fill="{ORANGE}"/>\n')
    out.append("    </g></g>\n")
    out.append('    <g clip-path="url(#globe)" fill="none" stroke="#FFFFFF" stroke-width="16" opacity="0.92">\n')
    # parallels
    for deg in (-60, -30, 0, 30, 60):
        y = cy - R * math.sin(math.radians(deg))
        out.append(f'      <line x1="{cx - R}" y1="{y:.1f}" x2="{cx + R}" y2="{y:.1f}"/>\n')
    # meridians
    for deg in (30, 60):
        rx = R * math.sin(math.radians(deg))
        out.append(f'      <ellipse cx="{cx}" cy="{cy}" rx="{rx:.1f}" ry="{R}"/>\n')
    out.append(f'      <line x1="{cx}" y1="{cy - R}" x2="{cx}" y2="{cy + R}"/>\n')
    out.append("    </g>\n")
    out.append(f'    <circle cx="{cx}" cy="{cy}" r="{R}" fill="none" stroke="#FFFFFF" stroke-width="24"/>\n')
    out.append("  </g>\n")
    out.append(svg_close())
    return "".join(out)


# --- option D: bold gridded cube, no loupe ----------------------------------


def option_d_bold_cube() -> str:
    s = 310
    cx, cy = 512, 530
    top, left, right = iso_cube(cx, cy, s)
    n, e, sv, wv = top
    out = [svg_open("GridLook icon, option D: bold gridded cube")]
    out.append("  </defs>\n")
    out.append(shadow_layer())
    out.append(background_layer(INK))
    out.append('  <g id="layer-midground">\n')
    out.append(f'    <polygon points="{pts(top)}" fill="{SAND}"/>\n')
    out.append(f'    <polygon points="{pts(left)}" fill="{BLUE}"/>\n')
    out.append(f'    <polygon points="{pts(right)}" fill="{TEAL}"/>\n')
    # one "hot" chunk on the top face and one on each side, like a chunk being read
    tl = face_grid_lines(top, 3)
    _ = tl
    p0, p1, p2, p3 = top

    def cell(face, i, j, div=3):
        a, b, c, d = face
        u0, u1 = i / div, (i + 1) / div
        v0, v1 = j / div, (j + 1) / div

        def at(u, v):
            top_pt = lerp(a, b, u)
            bot_pt = lerp(d, c, u)
            return lerp(top_pt, bot_pt, v)

        return [at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)]

    out.append(f'    <polygon points="{pts(cell(top, 1, 1))}" fill="{ORANGE}"/>\n')
    out.append(f'    <polygon points="{pts(cell(left, 1, 0))}" fill="{BLUE_DEEP}"/>\n')
    out.append(f'    <polygon points="{pts(cell(right, 1, 0))}" fill="{TEAL_DEEP}"/>\n')
    out.append("  </g>\n")
    out.append('  <g id="layer-foreground">\n')
    for face in (top, left, right):
        out.append(polyline_group(face_grid_lines(face, 3), INK, 18))
    outline = [n, e, (e[0], e[1] + s), (sv[0], sv[1] + s), (wv[0], wv[1] + s), wv]
    out.append(f'    <polygon points="{pts(outline)}" fill="none" stroke="{INK}" stroke-width="26" stroke-linejoin="round"/>\n')
    for a, b in ((wv, sv), (sv, e), (sv, (sv[0], sv[1] + s))):
        out.append(f'    <line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}" stroke="{INK}" stroke-width="26" stroke-linecap="round"/>\n')
    out.append("  </g>\n")
    out.append(svg_close())
    return "".join(out)


# --- option E: stacked slices (chunks / time steps) -------------------------


def option_e_slices() -> str:
    out = [svg_open("GridLook icon, option E: stacked gridded slices")]
    out.append("  </defs>\n")
    out.append(shadow_layer())
    out.append(background_layer(INK))
    out.append('  <g id="layer-midground">\n')
    cx = 512
    w, h = 330, 190  # half extents of each diamond
    thick = 46
    gap = 60
    fills = (BLUE_DEEP, TEAL, SAND)
    side_fills = (INK, TEAL_DEEP, ORANGE_DEEP)
    base_y = 512 + (thick + gap)  # centre of the lowest slab's top face
    for i in range(3):
        ycen = base_y - i * (thick + gap)
        n = (cx, ycen - h)
        e = (cx + w, ycen)
        sv = (cx, ycen + h)
        wv = (cx - w, ycen)
        top = [n, e, sv, wv]
        left = [wv, sv, (sv[0], sv[1] + thick), (wv[0], wv[1] + thick)]
        right = [sv, e, (e[0], e[1] + thick), (sv[0], sv[1] + thick)]
        out.append(f'    <polygon points="{pts(left)}" fill="{side_fills[i]}"/>\n')
        out.append(f'    <polygon points="{pts(right)}" fill="{side_fills[i]}"/>\n')
        out.append(f'    <polygon points="{pts(top)}" fill="{fills[i]}"/>\n')
        if i == 2:
            # highlight two chunks on the top slice
            def cell(i2, j2, div=3):
                a, b, c, d = top
                u0, u1 = i2 / div, (i2 + 1) / div
                v0, v1 = j2 / div, (j2 + 1) / div

                def at(u, v):
                    return lerp(lerp(a, b, u), lerp(d, c, u), v)

                return [at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)]

            out.append(f'    <polygon points="{pts(cell(1, 1))}" fill="{ORANGE}"/>\n')
            out.append(f'    <polygon points="{pts(cell(2, 1))}" fill="{ORANGE}"/>\n')
            out.append(f'    <polygon points="{pts(cell(1, 2))}" fill="{ORANGE}"/>\n')
        out.append(polyline_group(face_grid_lines(top, 3), INK, 14, 0.9 if i == 2 else 0.5))
        outline = [n, e, (e[0], e[1] + thick), (sv[0], sv[1] + thick), (wv[0], wv[1] + thick), wv]
        out.append(f'    <polygon points="{pts(outline)}" fill="none" stroke="{INK}" stroke-width="18" stroke-linejoin="round"/>\n')
        out.append(f'    <polyline points="{pts([wv, sv, e])}" fill="none" stroke="{INK}" stroke-width="18" stroke-linejoin="round"/>\n')
        out.append(f'    <line x1="{sv[0]}" y1="{sv[1]}" x2="{sv[0]}" y2="{sv[1] + thick}" stroke="{INK}" stroke-width="18"/>\n')
    out.append("  </g>\n")
    out.append(svg_close())
    return "".join(out)


OPTIONS = {
    "a-cube-loupe": option_a_cube_loupe,
    "a2-cube-loupe-dark": lambda: option_a_cube_loupe(background=INK, label="A2"),
    "b-tiles-loupe": option_b_tiles_loupe,
    "c-globe": option_c_globe,
    "d-bold-cube": option_d_bold_cube,
    "e-slices": option_e_slices,
}


def main() -> None:
    for name, fn in OPTIONS.items():
        path = HERE / f"{name}.svg"
        path.write_text(fn())
        print(f"wrote {path.relative_to(HERE.parent.parent)}")


if __name__ == "__main__":
    main()
