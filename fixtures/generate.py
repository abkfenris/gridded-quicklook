#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["xarray", "netcdf4", "zarr", "icechunk", "pandas", "numpy"]
#
# [tool.uv]
# exclude-newer = "2026-08-10T00:00:00Z"
# ///
"""Generate deterministic, tiny test fixtures for gridlook.

Re-runnable: wipes and recreates ``fixtures/data/`` and
``fixtures/reference/`` on every run. Every generated file is kept well
under 400 KB by using tiny dimensions (4x3x2), float32 data, fixed random
seeds, and no exotic compression.

Run with:

    ./fixtures/generate.py

(or: uv run --script fixtures/generate.py)
"""

from __future__ import annotations

import shutil
from pathlib import Path

import icechunk
import numpy as np
import pandas as pd
import xarray as xr

FIXTURES_DIR = Path(__file__).resolve().parent
DATA_DIR = FIXTURES_DIR / "data"
REFERENCE_DIR = FIXTURES_DIR / "reference"

# Tiny, deterministic dimensions shared by all fixtures.
N_TIME = 4
N_X = 3
N_Y = 2

SEED = 20260828


def make_simple_dataset() -> xr.Dataset:
    """A small Dataset with coords, data vars, attrs, and one encoded var."""
    rng = np.random.default_rng(SEED)

    time = pd.date_range("2024-01-01", periods=N_TIME, freq="D")
    x = np.linspace(-10.0, 10.0, N_X, dtype=np.float32)

    temperature = rng.normal(loc=15.0, scale=2.0, size=(N_TIME, N_X, N_Y)).astype(
        np.float32
    )
    salinity = rng.normal(loc=35.0, scale=0.5, size=(N_TIME, N_X, N_Y)).astype(
        np.float32
    )

    ds = xr.Dataset(
        data_vars={
            "temperature": (
                ("time", "x", "y"),
                temperature,
                {"units": "degC", "long_name": "Sea Water Temperature"},
            ),
            "salinity": (
                ("time", "x", "y"),
                salinity,
                {"units": "psu", "long_name": "Sea Water Salinity"},
            ),
        },
        coords={
            "time": time,
            "x": ("x", x, {"units": "km", "long_name": "Cross-shore distance"}),
        },
        attrs={
            "title": "gridlook simple fixture",
            "institution": "NERACOOS",
            "conventions": "CF-1.8",
        },
    )
    # Give temperature an explicit chunk encoding so the reader has to deal
    # with chunked/encoded variables even in this tiny fixture.
    ds["temperature"].encoding["chunksizes"] = (2, N_X, N_Y)
    return ds


def make_tree(ds: xr.Dataset) -> xr.DataTree:
    """A DataTree with root data, two child groups, and a nested grandchild."""
    child_a = ds[["temperature"]] * 1.01
    child_b = ds[["salinity"]] * 0.99
    grandchild = ds[["temperature"]] * 1.02

    return xr.DataTree.from_dict(
        {
            "/": ds,
            "/group_a": child_a,
            "/group_b": child_b,
            "/group_a/nested": grandchild,
        }
    )


def write_netcdf_fixtures(ds: xr.Dataset, dt: xr.DataTree) -> None:
    ds.to_netcdf(DATA_DIR / "simple.nc", engine="netcdf4")
    dt.to_netcdf(DATA_DIR / "groups.nc", engine="netcdf4")
    # Classic/CDF-1 format: no group API (single flat root) and a narrower
    # dtype repertoire than netCDF-4 (notably no int64/uint* types — writing
    # one raises rather than silently downcasting). `make_simple_dataset`
    # only uses float32 data/coord vars plus a datetime64 `time` coord (CF
    # semantics), so nothing needs to be dropped here; a future dataset that
    # adds int64/uint variables would need a classic-safe variant instead.
    ds.to_netcdf(
        DATA_DIR / "simple_classic.nc", engine="netcdf4", format="NETCDF3_CLASSIC"
    )


def write_zarr_fixtures(ds: xr.Dataset, dt: xr.DataTree) -> None:
    ds.to_zarr(
        DATA_DIR / "simple_v3.zarr", mode="w", zarr_format=3, consolidated=False
    )
    ds.to_zarr(
        DATA_DIR / "simple_v2.zarr",
        mode="w",
        zarr_format=2,
        consolidated=True,
    )
    dt.to_zarr(
        DATA_DIR / "tree.zarr", mode="w", zarr_format=3, consolidated=False
    )


def write_icechunk_fixture(ds: xr.Dataset) -> None:
    repo_path = DATA_DIR / "icechunk_repo.icechunk"
    storage = icechunk.local_filesystem_storage(str(repo_path))
    repo = icechunk.Repository.create(storage)

    session = repo.writable_session("main")
    ds.to_zarr(session.store, mode="w", zarr_format=3)
    session.commit("initial data")

    session = repo.writable_session("main")
    modified = ds.copy()
    modified.attrs["revision_note"] = "updated global attrs"
    modified.to_zarr(session.store, mode="w", zarr_format=3)
    session.commit("update global attrs")


def write_reference_html(ds: xr.Dataset, dt: xr.DataTree) -> None:
    (REFERENCE_DIR / "simple_nc.html").write_text(ds._repr_html_())
    (REFERENCE_DIR / "tree.html").write_text(dt._repr_html_())


def report_sizes() -> None:
    max_bytes = 400 * 1024
    over_limit = []
    for path in sorted(DATA_DIR.rglob("*")):
        if path.is_file():
            size = path.stat().st_size
            if size > max_bytes:
                over_limit.append((path, size))
    if over_limit:
        lines = "\n".join(f"  {p} ({s} bytes)" for p, s in over_limit)
        raise SystemExit(f"Fixture files exceed 400 KB limit:\n{lines}")


def main() -> None:
    for directory in (DATA_DIR, REFERENCE_DIR):
        if directory.exists():
            shutil.rmtree(directory)
        directory.mkdir(parents=True)

    ds = make_simple_dataset()
    dt = make_tree(ds)

    write_netcdf_fixtures(ds, dt)
    write_zarr_fixtures(ds, dt)
    write_icechunk_fixture(ds)
    write_reference_html(ds, dt)

    report_sizes()
    print(f"Wrote fixtures to {DATA_DIR}")
    print(f"Wrote reference HTML to {REFERENCE_DIR}")


if __name__ == "__main__":
    main()
