#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Download real-world sample files that ndlook's tests read.

Unlike ``generate.py``, which synthesizes tiny fixtures locally, some
formats are only worth testing against genuine output from the tools that
produce them. GRIB2 is one: hand-rolling a message would exercise our
reader against our own idea of the format rather than against NCEP's.

Re-runnable: wipes and recreates ``fixtures/samples/`` on every run.

Source: the NOAA GFS forecast archive in the AWS Open Data registry
(``noaa-gfs-bdp-pds``). NOAA products are US Government works in the public
domain. Each GRIB2 file ships with a ``.idx`` sidecar listing every
message's byte offset, so we pull just five messages with HTTP ``Range``
requests instead of the ~500 MB whole file. GRIB messages are
self-delimiting (each carries its own length in its indicator section), so
concatenating the ranges in index order yields a valid multi-message file.

Run with:

    ./fixtures/download_samples.py

(or: uv run --script fixtures/download_samples.py)
"""

from __future__ import annotations

import shutil
import urllib.request
from pathlib import Path

FIXTURES_DIR = Path(__file__).resolve().parent
SAMPLES_DIR = FIXTURES_DIR / "samples"

# A fixed, archived GFS cycle: a stable URL that keeps the fixture (and the
# snapshots taken from it) reproducible. NCEP's own products carry no file
# extension; the index sidecar is the same name plus `.idx`.
GRIB_URL = (
    "https://noaa-gfs-bdp-pds.s3.amazonaws.com"
    "/gfs.20260101/00/atmos/gfs.t00z.pgrb2.0p25.f003"
)
GRIB_NAME = "gfs.t00z.pgrb2.0p25.f003.sample.grib2"

# Five messages chosen to give the reader something to group: two variables
# that share the 500 mb isobaric level, one 2 m and two 10 m
# height-above-ground fields (so both a shared level and a shared variable
# family appear), all on the same 0.25 degree global grid.
WANTED = (
    "HGT:500 mb",
    "TMP:500 mb",
    "TMP:2 m above ground",
    "UGRD:10 m above ground",
    "VGRD:10 m above ground",
)
FORECAST = ":3 hour fcst:"


def fetch(url: str, byte_range: tuple[int, int | None] | None = None) -> bytes:
    """GET ``url``, optionally as a half-open ``[start, end)`` byte range."""
    request = urllib.request.Request(url)
    if byte_range is not None:
        start, end = byte_range
        suffix = "" if end is None else str(end - 1)
        request.add_header("Range", f"bytes={start}-{suffix}")
    with urllib.request.urlopen(request) as response:
        return response.read()


def parse_index(text: str) -> list[tuple[int, int | None, str]]:
    """Parse a wgrib2-style ``.idx`` into ``(start, end, description)``.

    Index lines look like ``1:0:d=2026010100:HGT:500 mb:3 hour fcst:``. A
    message runs from its own offset to the next line's offset; the last
    message runs to the end of the file (``end`` is ``None``).
    """
    records = []
    for line in text.splitlines():
        if not line.strip():
            continue
        fields = line.split(":")
        records.append((int(fields[1]), ":".join(fields[3:])))

    return [
        (start, records[i + 1][0] if i + 1 < len(records) else None, description)
        for i, (start, description) in enumerate(records)
    ]


def select(index: list[tuple[int, int | None, str]]) -> list[tuple[int, int | None, str]]:
    """Keep the wanted messages, in index (i.e. file) order."""
    selected = [
        record
        for record in index
        if FORECAST in record[2]
        and any(record[2].startswith(f"{wanted}:") for wanted in WANTED)
    ]

    found = {
        wanted
        for wanted in WANTED
        for record in selected
        if record[2].startswith(f"{wanted}:")
    }
    missing = sorted(set(WANTED) - found)
    if missing:
        raise SystemExit(f"Messages not found in the GRIB index: {missing}")

    return selected


def write_grib_sample() -> Path:
    index = parse_index(fetch(f"{GRIB_URL}.idx").decode("utf-8"))
    selected = select(index)

    out_path = SAMPLES_DIR / GRIB_NAME
    with out_path.open("wb") as out:
        for start, end, description in selected:
            print(f"  {description}")
            out.write(fetch(GRIB_URL, (start, end)))
    return out_path


def main() -> None:
    if SAMPLES_DIR.exists():
        shutil.rmtree(SAMPLES_DIR)
    SAMPLES_DIR.mkdir(parents=True)

    print(f"Downloading GRIB2 messages from {GRIB_URL}")
    grib_path = write_grib_sample()
    print(f"Wrote {grib_path} ({grib_path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
