//! Snapshot tests for the full HTML page renderer.
//!
//! Fixtures are built in code (not read from `fixtures/`, which is the
//! netcdf-reader crate's domain) so these tests stay independent of that
//! work. Snapshots are managed with `cargo insta`.

use gridlook_meta::model::{
    AttrValue, DatasetSummary, DimInfo, GroupSummary, SnapshotInfo, SourceFormat, VarSummary,
    VersionInfo,
};

fn dim(name: &str, size: u64) -> DimInfo {
    DimInfo {
        name: name.to_string(),
        size,
        is_unlimited: false,
    }
}

fn coord(name: &str, dtype: &str, dims: &[&str], preview: &str) -> VarSummary {
    let shape = dims.iter().map(|_| 1).collect();
    VarSummary {
        name: name.to_string(),
        dtype: dtype.to_string(),
        dims: dims.iter().map(|d| d.to_string()).collect(),
        shape,
        chunks: None,
        attrs: Vec::new(),
        preview: Some(preview.to_string()),
    }
}

fn empty_group(name: &str) -> GroupSummary {
    GroupSummary {
        name: name.to_string(),
        dims: Vec::new(),
        coords: Vec::new(),
        data_vars: Vec::new(),
        attrs: Vec::new(),
        children: Vec::new(),
    }
}

/// (a) A flat dataset resembling `simple.nc`: two coords with previews, two
/// data vars (one chunked), and global attrs.
fn flat_dataset() -> DatasetSummary {
    let root = GroupSummary {
        name: String::new(),
        dims: vec![dim("time", 4), dim("x", 3), dim("y", 2)],
        coords: vec![
            coord(
                "time",
                "datetime64[us]",
                &["time"],
                "2024-01-01 ... 2024-01-04",
            ),
            coord("x", "float32", &["x"], "-10.0 0.0 10.0"),
        ],
        data_vars: vec![
            VarSummary {
                name: "temperature".to_string(),
                dtype: "float32".to_string(),
                dims: vec!["time".to_string(), "x".to_string(), "y".to_string()],
                shape: vec![4, 3, 2],
                chunks: Some(vec![4, 3, 2]),
                attrs: vec![
                    ("units".to_string(), AttrValue::Text("degC".to_string())),
                    (
                        "long_name".to_string(),
                        AttrValue::Text("Sea Water Temperature".to_string()),
                    ),
                ],
                preview: Some("11.37 15.14 13.86 ... 17.82 17.03".to_string()),
            },
            VarSummary {
                name: "salinity".to_string(),
                dtype: "float32".to_string(),
                dims: vec!["time".to_string(), "x".to_string(), "y".to_string()],
                shape: vec![4, 3, 2],
                chunks: None,
                attrs: vec![("units".to_string(), AttrValue::Text("psu".to_string()))],
                preview: Some("35.62 35.13 35.17 ... 34.77 34.66".to_string()),
            },
        ],
        attrs: vec![
            (
                "title".to_string(),
                AttrValue::Text("gridlook simple fixture".to_string()),
            ),
            (
                "institution".to_string(),
                AttrValue::Text("NERACOOS".to_string()),
            ),
            (
                "conventions".to_string(),
                AttrValue::Text("CF-1.8".to_string()),
            ),
            (
                "valid_range".to_string(),
                AttrValue::FloatList(vec![0.0, 100.0]),
            ),
            ("flags".to_string(), AttrValue::IntList(vec![1, 2, 3])),
            (
                "history".to_string(),
                AttrValue::TextList(vec!["created".to_string(), "regridded".to_string()]),
            ),
        ],
        children: Vec::new(),
    };
    DatasetSummary {
        format: SourceFormat::NetCdf,
        root,
        version_info: None,
    }
}

/// (b) A nested tree: root + 2 children + 1 grandchild.
fn nested_tree() -> DatasetSummary {
    let grandchild = GroupSummary {
        name: "child_a/grandchild".to_string(),
        dims: vec![dim("z", 5)],
        coords: vec![],
        data_vars: vec![VarSummary {
            name: "pressure".to_string(),
            dtype: "float64".to_string(),
            dims: vec!["z".to_string()],
            shape: vec![5],
            chunks: None,
            attrs: Vec::new(),
            preview: Some("1013.0 ... 1000.0".to_string()),
        }],
        attrs: Vec::new(),
        children: Vec::new(),
    };

    let child_a = GroupSummary {
        name: "child_a".to_string(),
        dims: vec![dim("z", 5)],
        coords: vec![coord("z", "float32", &["z"], "0.0 ... 100.0")],
        data_vars: Vec::new(),
        attrs: vec![(
            "note".to_string(),
            AttrValue::Text("first child".to_string()),
        )],
        children: vec![grandchild],
    };

    let child_b = GroupSummary {
        name: "child_b".to_string(),
        dims: vec![dim("q", 2)],
        coords: vec![],
        data_vars: vec![VarSummary {
            name: "flag".to_string(),
            dtype: "int32".to_string(),
            dims: vec!["q".to_string()],
            shape: vec![2],
            chunks: None,
            attrs: Vec::new(),
            preview: None,
        }],
        attrs: Vec::new(),
        children: Vec::new(),
    };

    let root = GroupSummary {
        name: String::new(),
        dims: vec![dim("time", 10)],
        coords: vec![coord("time", "datetime64[us]", &["time"], "2024 ... 2025")],
        data_vars: Vec::new(),
        attrs: vec![(
            "title".to_string(),
            AttrValue::Text("tree fixture".to_string()),
        )],
        children: vec![child_a, child_b],
    };

    DatasetSummary {
        format: SourceFormat::ZarrV3,
        root,
        version_info: None,
    }
}

/// (c) A dataset with `VersionInfo` populated (Icechunk).
fn versioned_dataset() -> DatasetSummary {
    let mut ds = flat_dataset();
    ds.format = SourceFormat::Icechunk;
    ds.version_info = Some(VersionInfo {
        branch: "main".to_string(),
        truncated: false,
        ancestry: vec![
            SnapshotInfo {
                id: "ABCDEF0123456789".to_string(),
                message: Some("initial commit".to_string()),
                wrote_at: Some("2026-08-28T12:00:00Z".to_string()),
            },
            SnapshotInfo {
                id: "0011223344556677".to_string(),
                message: None,
                wrote_at: None,
            },
            SnapshotInfo {
                id: "99887766554433221".to_string(),
                message: Some("repo created".to_string()),
                wrote_at: None,
            },
        ],
    });
    ds
}

/// (d) An empty dataset edge case: no dims, no coords, no data vars, no attrs.
fn empty_dataset() -> DatasetSummary {
    DatasetSummary {
        format: SourceFormat::ZarrV2,
        root: empty_group(""),
        version_info: None,
    }
}

#[test]
fn flat_dataset_snapshot() {
    let ds = flat_dataset();
    let html = gridlook_html::render_page(&ds, "simple.nc", Some(236));
    insta::assert_snapshot!(html);
}

#[test]
fn nested_tree_snapshot() {
    let ds = nested_tree();
    let html = gridlook_html::render_page(&ds, "tree.zarr", Some(4096));
    insta::assert_snapshot!(html);
}

#[test]
fn versioned_dataset_snapshot() {
    let ds = versioned_dataset();
    let html = gridlook_html::render_page(&ds, "icechunk_repo", None);
    insta::assert_snapshot!(html);
}

#[test]
fn empty_dataset_snapshot() {
    let ds = empty_dataset();
    let html = gridlook_html::render_page(&ds, "empty.zarr", Some(0));
    insta::assert_snapshot!(html);
}
