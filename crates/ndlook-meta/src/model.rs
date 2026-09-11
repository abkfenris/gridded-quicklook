//! Format-agnostic summary of a gridded dataset's structure.
//!
//! Every format reader (NetCDF/HDF5, Zarr, Icechunk) produces a
//! [`DatasetSummary`]; the HTML renderer consumes this model and never
//! sees format-specific types.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

/// Which reader produced the summary. Rendered as a format badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    NetCdf,
    Hdf5,
    ZarrV2,
    ZarrV3,
    Icechunk,
}

/// Top-level summary of one dataset / store / repo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetSummary {
    pub format: SourceFormat,
    pub root: GroupSummary,
    /// Present only for version-controlled stores (Icechunk).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_info: Option<VersionInfo>,
}

/// One group (netCDF-4/HDF5 group, Zarr group, DataTree node).
///
/// `children` makes the model a datatree: renderers must recurse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupSummary {
    /// Group name; empty string for the root group.
    pub name: String,
    pub dims: Vec<DimInfo>,
    /// Variables classified as coordinates (name ∈ dims, or listed in a
    /// `coordinates` attribute — xarray's heuristic).
    pub coords: Vec<VarSummary>,
    pub data_vars: Vec<VarSummary>,
    pub attrs: Vec<(String, AttrValue)>,
    pub children: Vec<GroupSummary>,
}

impl GroupSummary {
    /// Builds a [`GroupSummary`] from a flat list of variables and children,
    /// applying xarray's coordinate-classification heuristic and the
    /// deterministic-ordering conventions shared by every format reader.
    ///
    /// A variable is classified as a coordinate if its name matches one of
    /// its own dimensions ("dimension coordinate"), or if it is named in
    /// some sibling variable's `coordinates` attribute within `vars`.
    /// `coords`, `data_vars`, and `children` are all sorted by name.
    ///
    /// `dims` is `Some` for formats with a real group-level dimension
    /// registry (netCDF), which is used as given (including `is_unlimited`
    /// flags); it is `None` for formats with no such registry (Zarr,
    /// Icechunk), in which case the group's dims are derived from the union
    /// of its variables' own `(name, size)` pairs, with `is_unlimited`
    /// always `false` — Zarr arrays have no notion of an
    /// unlimited/appendable dimension distinct from `shape`.
    pub fn from_parts(
        name: String,
        dims: Option<Vec<DimInfo>>,
        attrs: Vec<(String, AttrValue)>,
        vars: Vec<VarSummary>,
        mut children: Vec<GroupSummary>,
    ) -> Self {
        let mut coord_names: HashSet<String> = HashSet::new();
        for var in &vars {
            if let Some((_, AttrValue::Text(names))) =
                var.attrs.iter().find(|(k, _)| k == "coordinates")
            {
                coord_names.extend(names.split_whitespace().map(str::to_owned));
            }
        }

        let mut coords = Vec::new();
        let mut data_vars = Vec::new();
        for var in vars {
            let is_dim_coord = var.dims.contains(&var.name);
            if is_dim_coord || coord_names.contains(&var.name) {
                coords.push(var);
            } else {
                data_vars.push(var);
            }
        }
        coords.sort_by(|a, b| a.name.cmp(&b.name));
        data_vars.sort_by(|a, b| a.name.cmp(&b.name));

        let dims = dims.unwrap_or_else(|| {
            let mut dims_map: BTreeMap<String, u64> = BTreeMap::new();
            for var in coords.iter().chain(data_vars.iter()) {
                for (dim_name, size) in var.dims.iter().zip(var.shape.iter()) {
                    dims_map.entry(dim_name.clone()).or_insert(*size);
                }
            }
            dims_map
                .into_iter()
                .map(|(name, size)| DimInfo {
                    name,
                    size,
                    is_unlimited: false,
                })
                .collect()
        });

        children.sort_by(|a, b| a.name.cmp(&b.name));

        GroupSummary {
            name,
            dims,
            coords,
            data_vars,
            attrs,
            children,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DimInfo {
    pub name: String,
    pub size: u64,
    pub is_unlimited: bool,
}

/// One variable/array's structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VarSummary {
    pub name: String,
    /// Human-readable dtype, e.g. `float32`, `int64`, `|S8`.
    pub dtype: String,
    pub dims: Vec<String>,
    pub shape: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<u64>>,
    pub attrs: Vec<(String, AttrValue)>,
    /// Short inline value peek for small variables (e.g. first few values of
    /// a 1-D coordinate), already formatted for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Attribute values, preserving enough type fidelity for faithful display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    Text(String),
    Int(i64),
    Float(f64),
    IntList(Vec<i64>),
    FloatList(Vec<f64>),
    TextList(Vec<String>),
}

/// One snapshot in a version-controlled store's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// RFC 3339 timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrote_at: Option<String>,
}

/// Version metadata for an Icechunk repo, scoped to the latest snapshot on
/// the default branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub branch: String,
    /// Newest first; the tip snapshot is `ancestry[0]`.
    pub ancestry: Vec<SnapshotInfo>,
    /// `true` if the ancestry walk was capped before reaching the repo's
    /// initial snapshot (see `ANCESTRY_LIMIT` in the Icechunk reader).
    pub truncated: bool,
}
