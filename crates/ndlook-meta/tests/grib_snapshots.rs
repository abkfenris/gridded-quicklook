use std::path::{Path, PathBuf};

use ndlook_meta::{SourceFormat, summarize_grib};

/// The GRIB sample is downloaded, not generated: see
/// `fixtures/download_samples.py` (mise task `samples`).
fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/samples")
        .join(name)
}

const GFS_SAMPLE: &str = "gfs.t00z.pgrb2.0p25.f003.sample.grib2";

#[test]
fn gfs_sample_snapshot() {
    let summary = summarize_grib(&sample(GFS_SAMPLE)).expect("summarize the GFS sample");
    insta::assert_json_snapshot!(summary);
}

#[test]
fn gfs_sample_is_reported_as_grib() {
    let summary = summarize_grib(&sample(GFS_SAMPLE)).expect("summarize the GFS sample");
    assert_eq!(summary.format, SourceFormat::Grib);
}

/// The sample holds five single-level, single-time messages, so every one
/// of them should reshape into its own variable and nothing should land in
/// `unaligned`. Temperature appears at two level types (500 mb and 2 m
/// above ground), which must not collapse into one variable.
#[test]
fn gfs_sample_messages_become_one_variable_each() {
    let summary = summarize_grib(&sample(GFS_SAMPLE)).expect("summarize the GFS sample");

    let names: Vec<&str> = summary
        .root
        .data_vars
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "hgt",
            "tmp_heightAboveGround",
            "tmp_isobaricInhPa",
            "ugrd",
            "vgrd"
        ]
    );
    assert!(
        summary.root.children.is_empty(),
        "a clean single-grid file must not produce an `unaligned` group"
    );
}

/// Every message is one 0.25 degree global field, so each variable is
/// latitude/longitude only: its single time, step and level become scalar
/// coordinates rather than length-1 dimensions, exactly as cfgrib reports
/// them.
#[test]
fn gfs_sample_variables_are_latitude_longitude_fields() {
    let summary = summarize_grib(&sample(GFS_SAMPLE)).expect("summarize the GFS sample");

    for var in &summary.root.data_vars {
        assert_eq!(var.dims, vec!["latitude", "longitude"], "{}", var.name);
        assert_eq!(var.shape, vec![721, 1440], "{}", var.name);
        assert_eq!(var.dtype, "float64", "{}", var.name);
    }

    let dims: Vec<&str> = summary.root.dims.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(dims, vec!["latitude", "longitude"]);
}

/// The scalar time/step/level variables are named in each data variable's
/// `coordinates` attribute, which is what promotes them out of the data
/// variables and into the coordinates.
#[test]
fn gfs_sample_scalar_levels_are_classified_as_coordinates() {
    let summary = summarize_grib(&sample(GFS_SAMPLE)).expect("summarize the GFS sample");

    let coords: Vec<&str> = summary
        .root
        .coords
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    for expected in ["time", "step", "latitude", "longitude", "isobaricInhPa"] {
        assert!(coords.contains(&expected), "missing coordinate {expected}");
    }

    // 2 m temperature and the 10 m winds both sit on `heightAboveGround`
    // at different heights, so the second value set gets its own name
    // rather than silently taking over the first.
    assert!(coords.contains(&"heightAboveGround"));
    assert!(coords.contains(&"heightAboveGround_2"));

    let two_metre = summary
        .root
        .coords
        .iter()
        .find(|v| v.name == "heightAboveGround")
        .expect("heightAboveGround coordinate");
    assert!(two_metre.dims.is_empty(), "single-valued levels are scalar");
    assert_eq!(two_metre.preview.as_deref(), Some("2.0"));
}

/// cfgrib-style provenance attributes survive onto every variable.
#[test]
fn gfs_sample_variables_carry_grib_attributes() {
    let summary = summarize_grib(&sample(GFS_SAMPLE)).expect("summarize the GFS sample");

    let hgt = summary
        .root
        .data_vars
        .iter()
        .find(|v| v.name == "hgt")
        .expect("hgt variable");
    let attr = |key: &str| {
        hgt.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| format!("{v:?}"))
            .unwrap_or_else(|| panic!("missing attribute {key}"))
    };

    assert!(attr("GRIB_shortName").contains("HGT"));
    assert!(attr("GRIB_typeOfLevel").contains("isobaricInhPa"));
    assert!(attr("GRIB_stepType").contains("instant"));
    assert!(attr("GRIB_gridType").contains("regular_ll"));
    assert!(attr("GRIB_numberOfMessages").contains('1'));
}
