//! Format-agnostic summary of a gridded dataset's structure.
//!
//! Every format reader (NetCDF/HDF5, Zarr, Icechunk) produces a
//! [`DatasetSummary`]; the HTML and CDL renderers consume this model and
//! never see format-specific types.

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

/// Knobs for how much a reader should extract beyond the plain structure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SummarizeOptions {
    /// Also collect per-variable storage details ([`VarSummary::storage`])
    /// and file-level format details ([`DatasetSummary::file_info`]) — the
    /// information `ncdump -s` surfaces as "special" virtual attributes.
    /// Off by default because gathering it costs extra metadata reads (and,
    /// for NetCDF, a second read-only open of the file).
    pub storage_details: bool,
}

/// Top-level summary of one dataset / store / repo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetSummary {
    pub format: SourceFormat,
    pub root: GroupSummary,
    /// Present only for version-controlled stores (Icechunk).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_info: Option<VersionInfo>,
    /// File-level format details; only populated when
    /// [`SummarizeOptions::storage_details`] is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_info: Option<FileInfo>,
}

/// File-level format details (what `ncdump -k` and the global `ncdump -s`
/// specials report).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    /// The `ncdump -k` style kind string: `classic`, `64-bit offset`,
    /// `cdf5`, `netCDF-4`, `netCDF-4 classic model`, `Zarr v2`, `Zarr v3`,
    /// or `Icechunk`.
    pub kind: String,
    /// netCDF-4's hidden `_NCProperties` provenance attribute, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nc_properties: Option<String>,
    /// HDF5 superblock version of a netCDF-4 file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superblock_version: Option<i32>,
    /// Whether libnetcdf reports the file as a netCDF-4 (HDF5) file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_netcdf4: Option<bool>,
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
    /// Every variable name in the reader's native order (netCDF varid
    /// order; Zarr's is alphabetical), spanning both `coords` and
    /// `data_vars`, which are themselves sorted by name. Renderers that must
    /// preserve file order (CDL) iterate this instead.
    #[serde(default)]
    pub var_order: Vec<String>,
}

impl GroupSummary {
    /// Builds a [`GroupSummary`] from a flat list of variables and children,
    /// applying xarray's coordinate-classification heuristic and the
    /// deterministic-ordering conventions shared by every format reader.
    ///
    /// A variable is classified as a coordinate if its name matches one of
    /// its own dimensions ("dimension coordinate"), or if it is named in
    /// some sibling variable's `coordinates` attribute within `vars`.
    /// `coords`, `data_vars`, and `children` are all sorted by name;
    /// `var_order` records the names in the order `vars` was given.
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

        let var_order: Vec<String> = vars.iter().map(|v| v.name.clone()).collect();

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
            var_order,
        }
    }

    /// Looks a variable up by name in either `coords` or `data_vars`.
    pub fn variable(&self, name: &str) -> Option<&VarSummary> {
        self.coords
            .iter()
            .chain(self.data_vars.iter())
            .find(|v| v.name == name)
    }

    /// All variables in native (`var_order`) order. Falls back to
    /// coords-then-data_vars for summaries built without a `var_order`
    /// (e.g. hand-constructed or deserialized from an older snapshot).
    pub fn variables_in_order(&self) -> Vec<&VarSummary> {
        let ordered: Vec<&VarSummary> = self
            .var_order
            .iter()
            .filter_map(|name| self.variable(name))
            .collect();
        if ordered.len() == self.coords.len() + self.data_vars.len() {
            ordered
        } else {
            self.coords.iter().chain(self.data_vars.iter()).collect()
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
    /// How the variable is stored on disk (layout, compression, codecs,
    /// endianness, ...). Only populated when
    /// [`SummarizeOptions::storage_details`] is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageInfo>,
}

/// How a variable's bytes are laid out in the file/store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageLayout {
    Chunked,
    Contiguous,
    /// netCDF-4 "compact" storage (small variables stored in the header).
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Endianness {
    Little,
    Big,
}

/// One HDF5 filter in a netCDF-4 variable's pipeline, other than the
/// deflate/shuffle/fletcher32 trio which [`StorageInfo`] reports directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterInfo {
    pub id: u32,
    pub params: Vec<u32>,
}

/// One entry in a Zarr array's codec chain (v3 `codecs`; v2 `filters`
/// followed by `compressor`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodecInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<serde_json::Value>,
}

/// Per-variable storage details — the payload behind `ncdump -s`'s
/// per-variable special attributes (`_Storage`, `_DeflateLevel`, ...).
///
/// Every field is optional/defaulted since each format fills in only what
/// it has: netCDF-4 knows about HDF5 filters and endianness, Zarr knows
/// about codec chains and metadata fill values, classic netCDF knows almost
/// nothing beyond the fill mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<StorageLayout>,
    /// zlib/deflate level (netCDF-4), when the deflate filter is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deflate_level: Option<u8>,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub fletcher32: bool,
    /// `None` when unknown, native, or meaningless (1-byte types, classic
    /// files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endianness: Option<Endianness>,
    /// netCDF "no fill" mode: unwritten values are not pre-filled.
    #[serde(default)]
    pub no_fill: bool,
    /// HDF5 filters other than deflate/shuffle/fletcher32 (netCDF-4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterInfo>,
    /// Zarr codec chain, in application order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codecs: Vec<CodecInfo>,
    /// Zarr metadata-level fill value (netCDF's lives in the `_FillValue`
    /// attribute already).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_value: Option<AttrValue>,
    /// Zarr v2 memory order, `'C'` or `'F'`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<char>,
    /// Zarr chunk key encoding, e.g. `default//`, `v2/.`, or for v2 stores
    /// just the dimension separator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_key_encoding: Option<String>,
}

/// Attribute values, preserving enough type fidelity for faithful display.
///
/// The first six variants are the "wide" ones every reader can produce (JSON
/// attributes only distinguish integer from float). The remaining variants
/// preserve the narrower numeric types netCDF attributes carry (`byte`,
/// `short`, `float`, unsigned variants, ...), so CDL output can print
/// ncdump's typed literal suffixes (`1.5f`, `2s`, `0b`, ...).
///
/// Serialized untagged, so the JSON shape is just the number/list/string
/// (NaN and infinities become `null`). Wide variants are declared first so
/// that untagged *deserialization* widens (`5` → `Int`, never `Int8`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    Text(String),
    /// A 64-bit integer: netCDF `int64`, or any JSON integer.
    Int(i64),
    /// A 64-bit float: netCDF `double`, or any JSON non-integer number.
    Float(f64),
    IntList(Vec<i64>),
    FloatList(Vec<f64>),
    TextList(Vec<String>),
    Int8(i8),
    UInt8(u8),
    Int16(i16),
    UInt16(u16),
    Int32(i32),
    UInt32(u32),
    UInt64(u64),
    Float32(f32),
    Int8List(Vec<i8>),
    UInt8List(Vec<u8>),
    Int16List(Vec<i16>),
    UInt16List(Vec<u16>),
    Int32List(Vec<i32>),
    UInt32List(Vec<u32>),
    UInt64List(Vec<u64>),
    Float32List(Vec<f32>),
}

/// The numeric type of an [`AttrValue`], for renderers that need to pick a
/// typed literal form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumKind {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

/// A borrowed view of one element of an [`AttrValue`], so renderers can
/// format scalars and list elements uniformly without matching 22 arms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttrScalar<'a> {
    Text(&'a str),
    Int(i64),
    UInt(u64),
    F32(f32),
    F64(f64),
}

impl AttrValue {
    /// The numeric type, or `None` for text values.
    pub fn num_kind(&self) -> Option<NumKind> {
        Some(match self {
            AttrValue::Text(_) | AttrValue::TextList(_) => return None,
            AttrValue::Int(_) | AttrValue::IntList(_) => NumKind::I64,
            AttrValue::Float(_) | AttrValue::FloatList(_) => NumKind::F64,
            AttrValue::Int8(_) | AttrValue::Int8List(_) => NumKind::I8,
            AttrValue::UInt8(_) | AttrValue::UInt8List(_) => NumKind::U8,
            AttrValue::Int16(_) | AttrValue::Int16List(_) => NumKind::I16,
            AttrValue::UInt16(_) | AttrValue::UInt16List(_) => NumKind::U16,
            AttrValue::Int32(_) | AttrValue::Int32List(_) => NumKind::I32,
            AttrValue::UInt32(_) | AttrValue::UInt32List(_) => NumKind::U32,
            AttrValue::UInt64(_) | AttrValue::UInt64List(_) => NumKind::U64,
            AttrValue::Float32(_) | AttrValue::Float32List(_) => NumKind::F32,
        })
    }

    /// `true` for the `*List` variants, regardless of their length.
    pub fn is_list(&self) -> bool {
        matches!(
            self,
            AttrValue::IntList(_)
                | AttrValue::FloatList(_)
                | AttrValue::TextList(_)
                | AttrValue::Int8List(_)
                | AttrValue::UInt8List(_)
                | AttrValue::Int16List(_)
                | AttrValue::UInt16List(_)
                | AttrValue::Int32List(_)
                | AttrValue::UInt32List(_)
                | AttrValue::UInt64List(_)
                | AttrValue::Float32List(_)
        )
    }

    /// Number of elements: 1 for scalar variants, the list length otherwise.
    pub fn len(&self) -> usize {
        self.scalars().len()
    }

    /// `true` only for an empty list variant.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every element as a uniform borrowed scalar (one element for the
    /// scalar variants).
    pub fn scalars(&self) -> Vec<AttrScalar<'_>> {
        match self {
            AttrValue::Text(s) => vec![AttrScalar::Text(s)],
            AttrValue::TextList(v) => v.iter().map(|s| AttrScalar::Text(s)).collect(),
            AttrValue::Int(i) => vec![AttrScalar::Int(*i)],
            AttrValue::IntList(v) => v.iter().map(|&i| AttrScalar::Int(i)).collect(),
            AttrValue::Int8(i) => vec![AttrScalar::Int((*i).into())],
            AttrValue::Int8List(v) => v.iter().map(|&i| AttrScalar::Int(i.into())).collect(),
            AttrValue::Int16(i) => vec![AttrScalar::Int((*i).into())],
            AttrValue::Int16List(v) => v.iter().map(|&i| AttrScalar::Int(i.into())).collect(),
            AttrValue::Int32(i) => vec![AttrScalar::Int((*i).into())],
            AttrValue::Int32List(v) => v.iter().map(|&i| AttrScalar::Int(i.into())).collect(),
            AttrValue::UInt8(u) => vec![AttrScalar::UInt((*u).into())],
            AttrValue::UInt8List(v) => v.iter().map(|&u| AttrScalar::UInt(u.into())).collect(),
            AttrValue::UInt16(u) => vec![AttrScalar::UInt((*u).into())],
            AttrValue::UInt16List(v) => v.iter().map(|&u| AttrScalar::UInt(u.into())).collect(),
            AttrValue::UInt32(u) => vec![AttrScalar::UInt((*u).into())],
            AttrValue::UInt32List(v) => v.iter().map(|&u| AttrScalar::UInt(u.into())).collect(),
            AttrValue::UInt64(u) => vec![AttrScalar::UInt(*u)],
            AttrValue::UInt64List(v) => v.iter().map(|&u| AttrScalar::UInt(u)).collect(),
            AttrValue::Float32(f) => vec![AttrScalar::F32(*f)],
            AttrValue::Float32List(v) => v.iter().map(|&f| AttrScalar::F32(f)).collect(),
            AttrValue::Float(f) => vec![AttrScalar::F64(*f)],
            AttrValue::FloatList(v) => v.iter().map(|&f| AttrScalar::F64(f)).collect(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, dims: &[&str]) -> VarSummary {
        VarSummary {
            name: name.to_owned(),
            dtype: "float32".to_owned(),
            dims: dims.iter().map(|d| (*d).to_owned()).collect(),
            shape: dims.iter().map(|_| 2).collect(),
            chunks: None,
            attrs: Vec::new(),
            preview: None,
            storage: None,
        }
    }

    #[test]
    fn from_parts_records_native_variable_order() {
        let group = GroupSummary::from_parts(
            String::new(),
            None,
            Vec::new(),
            vec![var("zeta", &["x"]), var("x", &["x"]), var("alpha", &["x"])],
            Vec::new(),
        );
        assert_eq!(group.var_order, vec!["zeta", "x", "alpha"]);
        let names: Vec<&str> = group
            .variables_in_order()
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(names, vec!["zeta", "x", "alpha"]);
        // ...while the classified lists stay sorted.
        assert_eq!(group.coords[0].name, "x");
        assert_eq!(group.data_vars[0].name, "alpha");
    }

    #[test]
    fn variables_in_order_falls_back_without_var_order() {
        let mut group = GroupSummary::from_parts(
            String::new(),
            None,
            Vec::new(),
            vec![var("b", &["x"]), var("a", &["x"])],
            Vec::new(),
        );
        group.var_order.clear();
        let names: Vec<&str> = group
            .variables_in_order()
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn attr_scalars_flatten_every_variant() {
        assert_eq!(
            AttrValue::Int8List(vec![-1, 2]).scalars(),
            vec![AttrScalar::Int(-1), AttrScalar::Int(2)]
        );
        assert_eq!(
            AttrValue::UInt64(u64::MAX).scalars(),
            vec![AttrScalar::UInt(u64::MAX)]
        );
        assert_eq!(
            AttrValue::Float32(1.5).scalars(),
            vec![AttrScalar::F32(1.5)]
        );
        assert_eq!(
            AttrValue::Text("a".into()).scalars(),
            vec![AttrScalar::Text("a")]
        );
        assert!(!AttrValue::Int(1).is_list());
        assert!(AttrValue::IntList(Vec::new()).is_list());
        assert!(AttrValue::IntList(Vec::new()).is_empty());
        assert_eq!(AttrValue::UInt16List(vec![1, 2, 3]).len(), 3);
        assert_eq!(AttrValue::Int16(1).num_kind(), Some(NumKind::I16));
        assert_eq!(AttrValue::Text("t".into()).num_kind(), None);
    }

    #[test]
    fn narrow_variants_serialize_like_wide_ones() {
        assert_eq!(
            serde_json::to_string(&AttrValue::Float32(1.5)).unwrap(),
            "1.5"
        );
        assert_eq!(
            serde_json::to_string(&AttrValue::Float32(f32::NAN)).unwrap(),
            "null"
        );
        assert_eq!(
            serde_json::to_string(&AttrValue::Int8List(vec![1, 2])).unwrap(),
            "[1,2]"
        );
        // Untagged deserialization widens to the first matching variant.
        let widened: AttrValue = serde_json::from_str("5").unwrap();
        assert_eq!(widened, AttrValue::Int(5));
    }
}
