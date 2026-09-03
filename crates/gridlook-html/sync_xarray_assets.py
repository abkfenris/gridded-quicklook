#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
#
# [tool.uv]
# exclude-newer = "2026-08-10T00:00:00Z"
# ///
"""Sync xarray's HTML repr assets (CSS + inline SVG icons) into this crate.

gridlook-html's renderer emits markup that mirrors xarray's `_repr_html_`
DOM structure and class names, so it reuses xarray's stylesheet verbatim.
This script fetches the assets from a pinned xarray release on GitHub and
writes them into `crates/gridlook-html/assets/`, prepending an attribution
header to the CSS.

Run with:

    ./crates/gridlook-html/sync_xarray_assets.py

(or: uv run --script crates/gridlook-html/sync_xarray_assets.py, or via
`mise run sync-xarray-assets`)
"""

from __future__ import annotations

import urllib.request
from pathlib import Path

CRATE_DIR = Path(__file__).resolve().parent
ASSETS_DIR = CRATE_DIR / "assets"

XARRAY_VERSION = "v2026.07.0"
XARRAY_RAW_BASE = (
    f"https://raw.githubusercontent.com/pydata/xarray/{XARRAY_VERSION}"
)
XARRAY_CSS_URL = f"{XARRAY_RAW_BASE}/xarray/static/css/style.css"
XARRAY_ICONS_URL = f"{XARRAY_RAW_BASE}/xarray/static/html/icons-svg-inline.html"

CSS_HEADER = f"""/*
 * This stylesheet is copied verbatim from xarray's HTML repr assets
 * (xarray/static/css/style.css, xarray {XARRAY_VERSION} (tag)), reproduced
 * here so gridlook's HTML Quick Look renderer -- whose emitted
 * markup mirrors xarray's `_repr_html_` DOM structure and class names --
 * is styled identically.
 *
 * Source: https://github.com/pydata/xarray
 * License: Apache License 2.0 (https://github.com/pydata/xarray/blob/main/LICENSE)
 * Copyright the xarray contributors.
 */

"""


def fetch(url: str) -> str:
    with urllib.request.urlopen(url) as response:  # noqa: S310
        return response.read().decode("utf-8")


def main() -> None:
    ASSETS_DIR.mkdir(parents=True, exist_ok=True)

    css_text = fetch(XARRAY_CSS_URL)
    (ASSETS_DIR / "xarray-style.css").write_text(
        CSS_HEADER + css_text, encoding="utf-8"
    )

    icons_text = fetch(XARRAY_ICONS_URL)
    (ASSETS_DIR / "xarray-icons-svg-inline.html").write_text(
        icons_text, encoding="utf-8"
    )

    print(f"Synced xarray {XARRAY_VERSION} assets into {ASSETS_DIR}")


if __name__ == "__main__":
    main()
