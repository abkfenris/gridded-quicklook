use std::path::{Path, PathBuf};

use gridded_meta::summarize_netcdf;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/data")
        .join(name)
}

#[test]
fn simple_nc_snapshot() {
    let summary = summarize_netcdf(&fixture("simple.nc")).expect("summarize simple.nc");
    insta::assert_json_snapshot!(summary);
}

#[test]
fn groups_nc_snapshot() {
    let summary = summarize_netcdf(&fixture("groups.nc")).expect("summarize groups.nc");
    insta::assert_json_snapshot!(summary);
}

/// Cross-check against xarray's own view of `simple.nc`
/// (`fixtures/reference/simple_nc.html`), which reports:
///   Coordinates: time, x
///   Data variables: temperature, salinity
///   Dimensions without coordinates: y
#[test]
fn simple_nc_coords_match_xarray_reference() {
    let summary = summarize_netcdf(&fixture("simple.nc")).expect("summarize simple.nc");

    let coord_names: Vec<&str> = summary
        .root
        .coords
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    let data_var_names: Vec<&str> = summary
        .root
        .data_vars
        .iter()
        .map(|v| v.name.as_str())
        .collect();

    assert_eq!(coord_names, vec!["time", "x"]);
    assert_eq!(data_var_names, vec!["salinity", "temperature"]);

    // `y` has no coordinate variable at all.
    assert!(summary.root.dims.iter().any(|d| d.name == "y"));
    assert!(!coord_names.contains(&"y"));
}
