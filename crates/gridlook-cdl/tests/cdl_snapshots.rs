//! CDL output for every fixture, plain (`-h`) and with specials (`-hs`),
//! pinned with insta. Fixtures come from `fixtures/generate.py`.

use std::path::{Path, PathBuf};

use gridlook_cdl::{CdlOptions, render_cdl};
use gridlook_meta::{DatasetSummary, SummarizeOptions, summarize_path};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/data")
        .join(name)
}

fn summarize(name: &str, details: bool) -> DatasetSummary {
    let opts = SummarizeOptions {
        storage_details: details,
    };
    summarize_path(&fixture(name), None, &opts).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn header(name: &str) -> String {
    render_cdl(&summarize(name, false), &CdlOptions::new(stem(name))).unwrap()
}

fn header_with_specials(name: &str) -> String {
    let mut opts = CdlOptions::new(stem(name));
    opts.specials = true;
    render_cdl(&summarize(name, true), &opts).unwrap()
}

/// Values that change whenever the fixture generator's pinned libraries
/// move (library versions) or on every regeneration (Icechunk ids/times).
fn volatile_filters() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            r#"_NCProperties = "[^"]*""#,
            r#"_NCProperties = "[nc-properties]""#,
        ),
        (
            r#"_IcechunkSnapshot = "[^"]*""#,
            r#"_IcechunkSnapshot = "[snapshot-id]""#,
        ),
        (
            r#"_IcechunkWroteAt = "[^"]*""#,
            r#"_IcechunkWroteAt = "[timestamp]""#,
        ),
    ]
}

macro_rules! cdl_snapshot_tests {
    ($($plain:ident, $special:ident => $file:literal;)*) => {
        $(
            #[test]
            fn $plain() {
                insta::assert_snapshot!(header($file));
            }

            #[test]
            fn $special() {
                insta::with_settings!({ filters => volatile_filters() }, {
                    insta::assert_snapshot!(header_with_specials($file));
                });
            }
        )*
    };
}

cdl_snapshot_tests! {
    simple_nc, simple_nc_specials => "simple.nc";
    groups_nc, groups_nc_specials => "groups.nc";
    simple_classic_nc, simple_classic_nc_specials => "simple_classic.nc";
    simple_v3_zarr, simple_v3_zarr_specials => "simple_v3.zarr";
    simple_v2_zarr, simple_v2_zarr_specials => "simple_v2.zarr";
    tree_zarr, tree_zarr_specials => "tree.zarr";
    icechunk_repo, icechunk_repo_specials => "icechunk_repo.icechunk";
    special_nc, special_nc_specials => "special.nc";
    codecs_v3_zarr, codecs_v3_zarr_specials => "codecs_v3.zarr";
    filters_v2_zarr, filters_v2_zarr_specials => "filters_v2.zarr";
}

/// `special.nc` covers the rest of ncdump's vocabulary: an unlimited
/// dimension, every typed literal suffix, deflate/shuffle/fletcher32, a
/// big-endian variable, char and string variables, a scalar with fill
/// disabled, and an escaped newline.
#[test]
fn special_nc_covers_ncdump_vocabulary() {
    let text = header_with_specials("special.nc");
    for needle in [
        "\trecord = UNLIMITED ; // (3 currently)\n",
        "\t\ttemperature:_FillValue = -9999.f ;\n",
        "\t\ttemperature:scale_factor = 0.01f ;\n",
        "\t\ttemperature:_DeflateLevel = 4 ;\n",
        "\t\ttemperature:_Shuffle = \"true\" ;\n",
        "\t\ttemperature:_Fletcher32 = \"true\" ;\n",
        "\tshort counts(x) ;\n",
        "\t\tcounts:valid_range = -50s, 50s ;\n",
        "\t\tcounts:flag_values = 1b, 2b, 3b ;\n",
        "\t\tcounts:ubyte_attr = 255UB ;\n",
        "\t\tcounts:ushort_attr = 65535US ;\n",
        "\t\tcounts:uint_attr = 7U ;\n",
        "\t\tcounts:int64_attr = 9LL ;\n",
        "\t\tcounts:uint64_attr = 18446744073709551614ULL ;\n",
        "\t\tcounts:double_attr = 0.1 ;\n",
        "\t\tcounts:_Endianness = \"big\" ;\n",
        "\tchar station_name(x, name_strlen) ;\n",
        "\tstring notes(x) ;\n",
        "\tint crs ;\n",
        "\t\tcrs:_NoFill = \"true\" ;\n",
        "\t\t:history = \"created by generate.py\\nsecond line\" ;\n",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
}

/// Zarr storage settings surface as `-s` specials.
#[test]
fn zarr_codec_fixtures_report_storage() {
    let v3 = header_with_specials("codecs_v3.zarr");
    assert!(v3.contains("pressure:_Endianness = \"big\" ;\n"), "{v3}");
    assert!(v3.contains("\"zstd({\\\"level\\\":3"), "{v3}");
    assert!(v3.contains("pressure:_FillValue = NaNf ;\n"), "{v3}");

    let v2 = header_with_specials("filters_v2.zarr");
    assert!(v2.contains("counts:_Order = \"F\" ;\n"), "{v2}");
    assert!(v2.contains("counts:_FillValue = -1 ;\n"), "{v2}");
    assert!(v2.contains("\"delta("), "{v2}");
    assert!(v2.contains("\tchar labels(x) ;\n"), "{v2}");
    assert!(v2.contains("labels:_StringLength = 6 ;\n"), "{v2}");
}

/// The header of `simple.nc` in the exact shape ncdump prints it.
#[test]
fn simple_nc_matches_ncdump_layout() {
    let text = header("simple.nc");
    assert!(text.starts_with("netcdf simple {\ndimensions:\n\ttime = 4 ;\n"));
    assert!(text.contains("\nvariables:\n"));
    assert!(text.contains("\tfloat temperature(time, x, y) ;\n"));
    assert!(text.contains("\t\ttemperature:_FillValue = NaNf ;\n"));
    assert!(text.contains("\t\ttemperature:units = \"degC\" ;\n"));
    assert!(text.contains("\n// global attributes:\n\t\t:title = \"gridlook simple fixture\" ;\n"));
    assert!(text.ends_with("}\n"));
}

#[test]
fn specials_report_format_and_chunking() {
    let text = header_with_specials("simple.nc");
    assert!(text.contains("\t\ttemperature:_Storage = \"chunked\" ;\n"));
    assert!(text.contains("\t\ttemperature:_ChunkSizes = 2, 3, 2 ;\n"));
    assert!(text.contains("\t\tsalinity:_Storage = \"contiguous\" ;\n"));
    assert!(text.contains("\t\t:_Format = \"netCDF-4\" ;\n"));

    let classic = header_with_specials("simple_classic.nc");
    assert!(classic.contains("\t\t:_Format = \"classic\" ;\n"));
    assert!(
        !classic.contains("_Storage"),
        "classic files have no HDF5 layout"
    );

    let zarr = header_with_specials("simple_v3.zarr");
    assert!(zarr.contains("\t\t:_Format = \"Zarr v3\" ;\n"));
    assert!(
        zarr.contains("temperature:_Codecs = \"bytes({\\\"endian\\\":\\\"little\\\"})\", \"zstd(")
    );

    let icechunk = header_with_specials("icechunk_repo.icechunk");
    assert!(icechunk.contains("\t\t:_Format = \"Icechunk\" ;\n"));
    assert!(icechunk.contains("\t\t:_IcechunkBranch = \"main\" ;\n"));
    assert!(icechunk.contains("\t\t:_IcechunkMessage = \"update global attrs\" ;\n"));
}
