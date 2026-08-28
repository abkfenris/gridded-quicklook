//! Format-agnostic summary of a gridded dataset's structure.
//!
//! Every format reader (NetCDF/HDF5, Zarr, Icechunk) produces a
//! [`DatasetSummary`]; the HTML renderer consumes this model and never
//! sees format-specific types.

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

/// Version metadata for an Icechunk repo, scoped to the latest snapshot on
/// the default branch. Ancestry is ids/messages only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub snapshot_id: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// RFC 3339 timestamp of the snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrote_at: Option<String>,
    pub n_snapshots: u64,
    /// (snapshot id, message) pairs, newest first, including the current one.
    pub ancestry: Vec<(String, Option<String>)>,
}
