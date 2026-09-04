#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["xarray", "netcdf4", "h5py", "zarr", "icechunk", "pandas", "numpy"]
#
# [tool.uv]
# exclude-newer = "2026-08-10T00:00:00Z"
# ///
"""Generate deterministic, tiny test fixtures for gridlook.

Re-runnable: wipes and recreates ``fixtures/data/`` and
``fixtures/reference/`` on every run. Every generated file is kept well
under 400 KB by using tiny dimensions (4x3x2), float32 data, fixed random
seeds, and only the light compression the ``-s`` fixtures need.

If an ``ncdump`` binary is on PATH, reference ``.cdl`` headers are written
to ``fixtures/reference/`` for every NetCDF fixture so ``gridlook dump``
output can be diffed against the real thing; otherwise that step is skipped.

Run with:

    ./fixtures/generate.py

(or: uv run --script fixtures/generate.py)
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import h5py
import icechunk
import netCDF4
import numcodecs
import numpy as np
import pandas as pd
import xarray as xr
import zarr
from zarr.codecs import BytesCodec, ZstdCodec

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


def write_plain_hdf5_fixture(ds: xr.Dataset) -> None:
    """An HDF5 file written by h5py, *not* by the netCDF-4 library.

    libnetcdf opens it all the same (datasets become variables over
    ``phony_dim_N`` dimensions), but it carries none of netCDF-4's markers
    (``_NCProperties``, dimension scales), which is what the reader keys on
    to report it as HDF5 rather than netCDF.
    """
    with h5py.File(DATA_DIR / "plain.h5", "w") as f:
        f.attrs["title"] = "gridlook plain HDF5 fixture"
        f.attrs["written_by"] = "h5py"
        temperature = f.create_dataset(
            "temperature", data=ds["temperature"].values, chunks=(2, N_X, N_Y)
        )
        temperature.attrs["units"] = "degC"
        f.create_dataset("x", data=ds["x"].values)
        diagnostics = f.create_group("diagnostics")
        diagnostics.create_dataset("salinity", data=ds["salinity"].values)


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


def write_special_netcdf() -> None:
    """A netCDF-4 file exercising everything ``ncdump -s`` reports.

    Written with the netCDF4 library directly (not xarray) so the storage
    details are chosen explicitly: an unlimited dimension, zlib level 4 with
    shuffle and fletcher32 on a chunked variable, a big-endian variable,
    attributes of every narrow numeric type (the CDL literal suffixes), a
    ``char`` variable with a string-length dimension, a variable-length
    ``string`` variable, a scalar variable with fill disabled, and a global
    attribute with an embedded newline.
    """
    rng = np.random.default_rng(SEED)
    with netCDF4.Dataset(DATA_DIR / "special.nc", "w", format="NETCDF4") as nc:
        nc.createDimension("record", None)  # unlimited
        nc.createDimension("x", N_X)
        nc.createDimension("name_strlen", 8)
        nc.title = "gridlook special fixture"
        nc.history = "created by generate.py\nsecond line"

        temperature = nc.createVariable(
            "temperature",
            "f4",
            ("record", "x"),
            zlib=True,
            complevel=4,
            shuffle=True,
            fletcher32=True,
            chunksizes=(1, N_X),
            fill_value=np.float32(-9999.0),
        )
        temperature.set_auto_maskandscale(False)
        temperature.units = "degC"
        temperature.scale_factor = np.float32(0.01)
        temperature[:3, :] = rng.normal(15.0, 2.0, size=(3, N_X)).astype(np.float32)

        counts = nc.createVariable("counts", ">i2", ("x",), endian="big")
        counts.valid_range = np.array([-50, 50], dtype=np.int16)
        counts.flag_values = np.array([1, 2, 3], dtype=np.int8)
        counts.flag_meanings = "low mid high"
        counts.ubyte_attr = np.uint8(255)
        counts.ushort_attr = np.uint16(65535)
        counts.uint_attr = np.uint32(7)
        counts.int64_attr = np.int64(9)
        counts.uint64_attr = np.uint64(2**64 - 2)
        counts.double_attr = np.float64(0.1)
        counts[:] = np.array([1, 2, 3], dtype=np.int16)

        station = nc.createVariable("station_name", "S1", ("x", "name_strlen"))
        station.long_name = "station name"
        names = np.array(["alpha", "beta", "gamma"], dtype="S8")
        station[:] = names.view("S1").reshape(N_X, 8)

        notes = nc.createVariable("notes", str, ("x",))
        notes[:] = np.array(["first", "second", "third"], dtype=object)

        crs = nc.createVariable("crs", "i4", (), fill_value=False)
        crs.epsg_code = np.int32(4326)
        crs[...] = 4326


def write_zarr_codec_fixtures() -> None:
    """Zarr stores with non-default storage settings for ``-s`` output.

    ``codecs_v3.zarr``: big-endian ``bytes`` codec plus zstd, a NaN fill
    value, and *inline consolidated metadata* in the root ``zarr.json`` (so
    the store can be read without listing, as over plain HTTP).
    ``filters_v2.zarr``: a Delta filter in front of Blosc, Fortran order, an
    explicit fill value, and a fixed-width bytes array.
    """
    rng = np.random.default_rng(SEED)

    v3 = zarr.open_group(DATA_DIR / "codecs_v3.zarr", mode="w", zarr_format=3)
    v3.attrs["title"] = "gridlook codecs fixture"
    pressure = v3.create_array(
        "pressure",
        shape=(N_X, N_Y),
        chunks=(N_X, 1),
        dtype="float32",
        serializer=BytesCodec(endian="big"),
        compressors=ZstdCodec(level=3),
        fill_value=np.float32("nan"),
        dimension_names=("x", "y"),
        attributes={"units": "hPa"},
    )
    pressure[:] = rng.normal(1013.0, 5.0, size=(N_X, N_Y)).astype(np.float32)
    zarr.consolidate_metadata(v3.store)

    v2 = zarr.open_group(DATA_DIR / "filters_v2.zarr", mode="w", zarr_format=2)
    v2.attrs["title"] = "gridlook filters fixture"
    counts = v2.create_array(
        "counts",
        shape=(N_X, N_Y),
        chunks=(N_X, N_Y),
        dtype="int16",
        filters=[numcodecs.Delta(dtype="int16")],
        compressors=numcodecs.Blosc(cname="zstd", clevel=3),
        order="F",
        fill_value=-1,
        attributes={"_ARRAY_DIMENSIONS": ["x", "y"], "units": "count"},
    )
    counts[:] = np.arange(N_X * N_Y, dtype=np.int16).reshape(N_X, N_Y)
    labels = v2.create_array(
        "labels",
        shape=(N_X,),
        chunks=(N_X,),
        dtype="S6",
        attributes={"_ARRAY_DIMENSIONS": ["x"]},
    )
    labels[:] = np.array([b"alpha", b"beta", b"gamma"], dtype="S6")


def write_reference_cdl() -> None:
    """``ncdump -h`` / ``-hs`` headers for every NetCDF fixture, when an
    ``ncdump`` binary is available (e.g. ``brew install netcdf``), so the
    CLI's output can be diffed against the reference implementation."""
    ncdump = shutil.which("ncdump")
    if ncdump is None:
        print("ncdump not found on PATH; skipping reference CDL")
        return
    for nc_file in sorted(DATA_DIR.glob("*.nc")):
        for flags, suffix in (("-h", ".cdl"), ("-hs", ".s.cdl")):
            result = subprocess.run(
                [ncdump, flags, str(nc_file)], check=True, capture_output=True, text=True
            )
            (REFERENCE_DIR / f"{nc_file.stem}{suffix}").write_text(result.stdout)


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
    write_plain_hdf5_fixture(ds)
    write_special_netcdf()
    write_zarr_fixtures(ds, dt)
    write_zarr_codec_fixtures()
    write_icechunk_fixture(ds)
    write_reference_html(ds, dt)
    write_reference_cdl()

    report_sizes()
    print(f"Wrote fixtures to {DATA_DIR}")
    print(f"Wrote reference HTML to {REFERENCE_DIR}")


if __name__ == "__main__":
    main()
