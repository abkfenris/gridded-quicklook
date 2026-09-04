use std::path::{Path, PathBuf};

use gridlook_meta::{GroupSummary, VarSummary, summarize_zarr};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/data")
        .join(name)
}

#[test]
fn simple_v3_zarr_snapshot() {
    let summary = summarize_zarr(&fixture("simple_v3.zarr")).expect("summarize simple_v3.zarr");
    insta::assert_json_snapshot!(summary);
}

#[test]
fn simple_v2_zarr_snapshot() {
    let summary = summarize_zarr(&fixture("simple_v2.zarr")).expect("summarize simple_v2.zarr");
    insta::assert_json_snapshot!(summary);
}

#[test]
fn tree_zarr_snapshot() {
    let summary = summarize_zarr(&fixture("tree.zarr")).expect("summarize tree.zarr");
    insta::assert_json_snapshot!(summary);
}

/// Projection of a group used to compare v2 and v3 readings of "the same"
/// dataset without depending on the `format` field, which necessarily
/// differs between them.
#[derive(Debug, PartialEq)]
struct GroupShape {
    name: String,
    dim_names: Vec<String>,
    coord_names: Vec<String>,
    data_var_names: Vec<String>,
    var_shapes: Vec<(String, String, Vec<u64>)>,
    children: Vec<GroupShape>,
}

fn var_shape(var: &VarSummary) -> (String, String, Vec<u64>) {
    (var.name.clone(), var.dtype.clone(), var.shape.clone())
}

fn group_shape(group: &GroupSummary) -> GroupShape {
    let mut dim_names: Vec<String> = group.dims.iter().map(|d| d.name.clone()).collect();
    dim_names.sort();

    let mut var_shapes: Vec<_> = group
        .coords
        .iter()
        .chain(group.data_vars.iter())
        .map(var_shape)
        .collect();
    var_shapes.sort();

    GroupShape {
        name: group.name.clone(),
        dim_names,
        coord_names: group.coords.iter().map(|v| v.name.clone()).collect(),
        data_var_names: group.data_vars.iter().map(|v| v.name.clone()).collect(),
        var_shapes,
        children: group.children.iter().map(group_shape).collect(),
    }
}

/// `simple_v2.zarr` and `simple_v3.zarr` are two encodings of the same
/// logical dataset; they should agree on dims, coord/data-var names, and
/// each variable's dtype/shape even though the on-disk `format` differs.
#[test]
fn v2_and_v3_simple_zarr_have_equivalent_structure() {
    let v2 = summarize_zarr(&fixture("simple_v2.zarr")).expect("summarize simple_v2.zarr");
    let v3 = summarize_zarr(&fixture("simple_v3.zarr")).expect("summarize simple_v3.zarr");

    assert_ne!(v2.format, v3.format);
    assert_eq!(group_shape(&v2.root), group_shape(&v3.root));
}

/// Cross-check against the same xarray reference used for the netCDF
/// fixture (`fixtures/reference/simple_nc.html`): time/x are coordinates,
/// temperature/salinity are data variables, y has no coordinate variable.
#[test]
fn simple_v3_coords_match_xarray_reference() {
    let summary = summarize_zarr(&fixture("simple_v3.zarr")).expect("summarize simple_v3.zarr");

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

    assert!(summary.root.dims.iter().any(|d| d.name == "y"));
    assert!(!coord_names.contains(&"y"));
}
