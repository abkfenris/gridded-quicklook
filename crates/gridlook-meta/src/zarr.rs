//! Zarr directory-store metadata reader.
//!
//! Walks a Zarr v2 or v3 directory store's node metadata (`zarr.json`, or
//! `.zgroup`/`.zarray`/`.zattrs`, or a consolidated `.zmetadata`) into a
//! format-agnostic [`DatasetSummary`]. Only the small per-node JSON metadata
//! files are read; chunk data is never opened, so previews are always
//! omitted (see [`summarize_zarr`] for details).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::Bound;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use zarrs_metadata::v2::{ArrayMetadataV2, DataTypeMetadataV2};
use zarrs_metadata::v3::{ArrayMetadataV3, GroupMetadataV3, MetadataV3, NodeMetadataV3};

use crate::error::MetaError;
use crate::model::{AttrValue, DatasetSummary, GroupSummary, SourceFormat, VarSummary};

/// Summarize the structure of a Zarr directory store at `path`.
///
/// Detects the store's flavor from what's present at `path`:
/// - `zarr.json` at the root → Zarr v3 (walks the directory tree; every
///   subdirectory with its own `zarr.json` is a group or array node).
/// - `.zmetadata` at the root → Zarr v2 with consolidated metadata; all node
///   metadata is read from that single file and the tree is **not** walked
///   further.
/// - `.zgroup` (or `.zarray`, for a store whose root is a single array) at
///   the root, with no consolidated metadata → Zarr v2, walked node by node
///   via each node's own `.zgroup`/`.zarray`/`.zattrs` files.
///
/// Variable `preview`s are always `None`: previewing would mean reading and
/// decompressing chunk data (Zarr has no NetCDF-style "just read the small
/// array inline" path), which this metadata-only reader deliberately avoids.
pub fn summarize_zarr(path: &Path) -> Result<DatasetSummary, MetaError> {
    let v3_root = path.join("zarr.json");
    if v3_root.is_file() {
        let node: NodeMetadataV3 = read_json_as(&v3_root)?;
        let root = match node {
            NodeMetadataV3::Group(group) => {
                walk_v3_group(path, String::new(), group, &mut WalkGuard::default())?
            }
            NodeMetadataV3::Array(array) => {
                let var = v3_var_summary(root_array_name(path), &array, &v3_root)?;
                // The array's attributes belong to the variable (attached
                // by `v3_var_summary`); the synthetic root group wrapping
                // it has none of its own, same as the v2 root-array case.
                build_group_summary(
                    String::new(),
                    &serde_json::Map::new(),
                    vec![var],
                    Vec::new(),
                )
            }
        };
        return Ok(DatasetSummary {
            format: SourceFormat::ZarrV3,
            root,
            version_info: None,
        });
    }

    if path.join(".zmetadata").is_file() {
        return summarize_v2_consolidated(path);
    }

    if path.join(".zgroup").is_file() {
        let root = walk_v2_group(path, String::new(), &mut WalkGuard::default())?;
        return Ok(DatasetSummary {
            format: SourceFormat::ZarrV2,
            root,
            version_info: None,
        });
    }

    if path.join(".zarray").is_file() {
        let var = v2_var_summary(root_array_name(path), path)?;
        let root = build_group_summary(
            String::new(),
            &serde_json::Map::new(),
            vec![var],
            Vec::new(),
        );
        return Ok(DatasetSummary {
            format: SourceFormat::ZarrV2,
            root,
            version_info: None,
        });
    }

    Err(MetaError::Invalid {
        path: path.to_path_buf(),
        message: "not a recognizable Zarr store (missing zarr.json/.zgroup/.zarray/.zmetadata)"
            .to_owned(),
    })
}

/// Derives a variable name for a store whose *root* is a single array (no
/// enclosing group), rather than the usual empty string used for a node at
/// the root path: an array node otherwise has no name of its own to fall
/// back on, unlike a child array which takes its directory's name.
///
/// Uses the store directory's file stem, e.g. `root.zarr` → `root`, matching
/// how such single-array stores are conventionally named; falls back to
/// `"array"` when the path has no usable stem (e.g. `/`, `.`, or a name that
/// is only an extension like `.zarr`).
fn root_array_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "array".to_owned())
}

/// Guards the recursive directory walkers (v2 and v3) against stores whose
/// directory graph isn't a tree.
///
/// Both walkers descend into every subdirectory carrying node metadata, and
/// `Path::is_dir` follows symlinks, so a symlink loop inside a group (`loop
/// -> .` makes `loop/zarr.json` resolve to the group's own metadata) sends
/// the walk back into a directory it is already inside. The kernel's
/// per-lookup symlink limit eventually stops that (`ELOOP` after 40
/// traversals on Linux, 32 on macOS), but not before the walk has produced
/// that many nested phantom copies of the group. So each group's canonical
/// path is checked against its ancestors', and a child that resolves to an
/// enclosing group is reported as a malformed store rather than walked.
///
/// A plain depth cap backs that up: no real hierarchy is anywhere near
/// [`MAX_GROUP_DEPTH`] levels deep, and bounding the recursion keeps a
/// pathological store from exhausting the (small) stack of whatever thread
/// Quick Look renders on.
#[derive(Default)]
struct WalkGuard {
    /// Canonical paths of the groups currently being walked, root first.
    ancestors: Vec<std::path::PathBuf>,
}

/// See [`WalkGuard`].
const MAX_GROUP_DEPTH: usize = 64;

impl WalkGuard {
    /// Registers `dir` as the group now being walked; call [`Self::leave`]
    /// once its children are done.
    fn enter(&mut self, dir: &Path) -> Result<(), MetaError> {
        // If the path can't be canonicalized (odd filesystem), fall back to
        // the raw path: loop detection degrades, the depth cap still holds.
        let canonical = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if self.ancestors.contains(&canonical) {
            return Err(MetaError::Invalid {
                path: dir.to_path_buf(),
                message: format!(
                    "symlink loop: resolves to the enclosing group {}",
                    canonical.display()
                ),
            });
        }
        if self.ancestors.len() >= MAX_GROUP_DEPTH {
            return Err(MetaError::Invalid {
                path: dir.to_path_buf(),
                message: format!("group nesting deeper than {MAX_GROUP_DEPTH} levels"),
            });
        }
        self.ancestors.push(canonical);
        Ok(())
    }

    fn leave(&mut self) {
        self.ancestors.pop();
    }
}

// ---------------------------------------------------------------------
// Shared group-building logic (used by both v2 and v3 readers)
// ---------------------------------------------------------------------

/// Thin wrapper around [`GroupSummary::from_parts`] for Zarr's JSON-shaped
/// attributes: Zarr has no group-level dimension registry like netCDF, so
/// `dims` is always derived (`None`) rather than supplied.
pub(crate) fn build_group_summary(
    name: String,
    attrs: &serde_json::Map<String, Value>,
    vars: Vec<VarSummary>,
    children: Vec<GroupSummary>,
) -> GroupSummary {
    GroupSummary::from_parts(name, None, json_object_to_attrs(attrs), vars, children)
}

/// Converts a JSON attributes object into the format-agnostic attr list.
/// `_ARRAY_DIMENSIONS` (Zarr v2's xarray-convention dim-name attribute) is
/// dropped here since it's surfaced as `VarSummary::dims` instead, matching
/// how xarray itself hides it from a variable's displayed attributes.
///
/// Entries are explicitly sorted by key for deterministic output.
/// `serde_json::Map` is normally `BTreeMap`-backed (alphabetical iteration
/// for free), but `zarrs_metadata` pulls in serde_json's `preserve_order`
/// feature, which — via Cargo's workspace-wide feature unification —
/// switches every `serde_json::Map` in this build over to an
/// insertion-order-preserving `IndexMap`, so ordering can no longer be
/// assumed and must be done here instead.
fn json_object_to_attrs(map: &serde_json::Map<String, Value>) -> Vec<(String, AttrValue)> {
    let mut attrs: Vec<(String, AttrValue)> = map
        .iter()
        .filter(|(k, _)| k.as_str() != "_ARRAY_DIMENSIONS")
        .map(|(k, v)| {
            let attr = decode_base64_float_attr(k, v).unwrap_or_else(|| json_value_to_attr(v));
            (k.clone(), attr)
        })
        .collect();
    attrs.sort_by(|a, b| a.0.cmp(&b.0));
    attrs
}

/// zarr-python's V3 JSON encoder writes non-finite float attribute values
/// (NaN/±Inf, most commonly a `_FillValue` of NaN) as the base64 of the
/// float's little-endian bytes, e.g. `"AAAAAAAA+H8="` for a float64 NaN.
/// Decode that back to the number it means so Zarr v3 and Icechunk previews
/// show `NaN` like the NetCDF reader does, instead of base64 noise. Scoped
/// to `_FillValue` only: any short string can *look* like base64, and only
/// this attribute is known to carry the encoding.
fn decode_base64_float_attr(key: &str, value: &Value) -> Option<AttrValue> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    if key != "_FillValue" {
        return None;
    }
    let bytes = STANDARD.decode(value.as_str()?).ok()?;
    let float = match bytes.len() {
        4 => f32::from_le_bytes(bytes.try_into().ok()?) as f64,
        8 => f64::from_le_bytes(bytes.try_into().ok()?),
        _ => return None,
    };
    Some(AttrValue::Float(float))
}

/// Widens a JSON attribute value into [`AttrValue`]. JSON has no distinct
/// integer/float split the way `AttrValue` does, so a number is reported as
/// `Int` when it round-trips through `i64`, else `Float`; a list is reported
/// as the narrowest homogeneous list variant it fits, else stringified.
/// Booleans and objects have no direct `AttrValue` counterpart and are
/// stringified as text.
fn json_value_to_attr(value: &Value) -> AttrValue {
    match value {
        Value::String(s) => AttrValue::Text(s.clone()),
        Value::Bool(b) => AttrValue::Text(b.to_string()),
        Value::Number(n) => match n.as_i64() {
            Some(i) => AttrValue::Int(i),
            None => AttrValue::Float(n.as_f64().unwrap_or_default()),
        },
        Value::Array(items) => {
            if let Some(ints) = items.iter().map(Value::as_i64).collect::<Option<Vec<_>>>() {
                AttrValue::IntList(ints)
            } else if let Some(floats) = items.iter().map(Value::as_f64).collect::<Option<Vec<_>>>()
            {
                AttrValue::FloatList(floats)
            } else if let Some(texts) = items
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
            {
                AttrValue::TextList(texts)
            } else {
                AttrValue::Text(value.to_string())
            }
        }
        Value::Object(_) | Value::Null => AttrValue::Text(value.to_string()),
    }
}

/// Entries of `map` whose key lies strictly under `prefix` (starts with it
/// and is longer). Sorted maps keep every key sharing a prefix contiguous,
/// so this is a range scan that stops at the first key past the block
/// rather than a filter over the whole map.
fn keys_under<'a, V>(
    map: &'a BTreeMap<String, V>,
    prefix: &'a str,
) -> impl Iterator<Item = (&'a String, &'a V)> + 'a {
    map.range::<str, _>((Bound::Excluded(prefix), Bound::Unbounded))
        .take_while(move |(key, _)| key.starts_with(prefix))
}

// ---------------------------------------------------------------------
// JSON file I/O helpers
// ---------------------------------------------------------------------

fn read_json_as<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, MetaError> {
    let text = fs::read_to_string(path).map_err(|source| MetaError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| MetaError::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// Like [`read_json_as`], but returns `Ok(None)` if the file is simply
/// absent (e.g. an optional `.zattrs`) rather than treating that as an I/O
/// error.
fn read_json_value_opt(path: &Path) -> Result<Option<Value>, MetaError> {
    if !path.is_file() {
        return Ok(None);
    }
    read_json_as::<Value>(path).map(Some)
}

/// Reads a v2 node's optional `.zattrs` file, if present, defaulting to an
/// empty attribute map. Shared by both the direct v2 walker and consolidated
/// v2 reading paths, which each read a node's attributes this same way.
fn read_v2_attrs(dir: &Path) -> Result<serde_json::Map<String, Value>, MetaError> {
    Ok(read_json_value_opt(&dir.join(".zattrs"))?
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default())
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<fs::DirEntry>, MetaError> {
    let mut entries = fs::read_dir(dir)
        .map_err(|source| MetaError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| MetaError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

// ---------------------------------------------------------------------
// Zarr v3
// ---------------------------------------------------------------------

/// Extracts a node's `attributes` map regardless of whether it's an array
/// or a group node — both carry one, just on different underlying structs.
///
/// Since [`walk_v3_group`] now takes an already-narrowed [`GroupMetadataV3`]
/// (see its doc comment), this is only reached from `icechunk.rs`, which
/// still deals in the un-narrowed [`NodeMetadataV3`].
#[cfg_attr(not(feature = "icechunk"), allow(dead_code))]
pub(crate) fn v3_node_attrs(node: &NodeMetadataV3) -> &serde_json::Map<String, Value> {
    match node {
        NodeMetadataV3::Array(array) => &array.attributes,
        NodeMetadataV3::Group(group) => &group.attributes,
    }
}

/// An attribute-less group node, used as a stand-in for a store whose root
/// group carries no metadata of its own.
#[cfg_attr(not(feature = "icechunk"), allow(dead_code))]
pub(crate) fn empty_v3_group_node() -> NodeMetadataV3 {
    NodeMetadataV3::Group(GroupMetadataV3::default())
}

/// Builds a group tree from a flat map of *relative* node paths (`""` for
/// the root, `"a/b"` for nested nodes; no leading or trailing slash) to
/// parsed v3 node metadata. That is the shape an Icechunk snapshot's node
/// list comes in, so the hierarchy is recovered here rather than per
/// reader. A missing root entry is treated as an attribute-less group.
///
/// `store_root` only serves to name the offending node in errors.
///
/// Each group finds its children with a range scan ([`keys_under`]) over
/// the sorted map, so the whole build costs O(N · depth) key comparisons
/// instead of the O(N · groups) of filtering the entire map once per group.
#[cfg_attr(not(feature = "icechunk"), allow(dead_code))]
pub(crate) fn build_v3_tree(
    nodes: &BTreeMap<String, NodeMetadataV3>,
    store_root: &Path,
) -> Result<GroupSummary, MetaError> {
    let empty_root = empty_v3_group_node();
    let root = nodes.get("").unwrap_or(&empty_root);
    build_v3_tree_group(nodes, store_root, "", String::new(), root)
}

#[cfg_attr(not(feature = "icechunk"), allow(dead_code))]
fn build_v3_tree_group(
    nodes: &BTreeMap<String, NodeMetadataV3>,
    store_root: &Path,
    node_path: &str,
    name: String,
    meta: &NodeMetadataV3,
) -> Result<GroupSummary, MetaError> {
    let prefix = if node_path.is_empty() {
        String::new()
    } else {
        format!("{node_path}/")
    };

    let mut vars = Vec::new();
    let mut children = Vec::new();
    for (child_path, child_meta) in keys_under(nodes, &prefix) {
        let rest = &child_path[prefix.len()..];
        if rest.contains('/') {
            // A grandchild or deeper: its own parent group collects it.
            continue;
        }
        match child_meta {
            NodeMetadataV3::Array(array) => vars.push(v3_var_summary(
                rest.to_owned(),
                array,
                &store_root.join(child_path),
            )?),
            NodeMetadataV3::Group(_) => children.push(build_v3_tree_group(
                nodes,
                store_root,
                child_path,
                rest.to_owned(),
                child_meta,
            )?),
        }
    }

    Ok(build_group_summary(
        name,
        v3_node_attrs(meta),
        vars,
        children,
    ))
}

/// Walks a v3 group's children, given `own` — the group's `zarr.json`
/// already parsed by the caller (either [`summarize_zarr`] for the root, or
/// this function itself for a recursive call), so each node's `zarr.json` is
/// read from disk exactly once rather than being parsed once for type
/// detection and re-parsed on recursion.
fn walk_v3_group(
    dir: &Path,
    name: String,
    own: GroupMetadataV3,
    guard: &mut WalkGuard,
) -> Result<GroupSummary, MetaError> {
    guard.enter(dir)?;
    let mut vars = Vec::new();
    let mut children = Vec::new();
    for entry in read_dir_sorted(dir)? {
        let child_path = entry.path();
        if !child_path.is_dir() {
            continue;
        }
        let node_json = child_path.join("zarr.json");
        if !node_json.is_file() {
            // Not a Zarr node (e.g. a chunk directory can't appear here,
            // since chunks live inside array directories, not beside them).
            continue;
        }
        let child_name = entry.file_name().to_string_lossy().into_owned();
        let node: NodeMetadataV3 = read_json_as(&node_json)?;
        match node {
            NodeMetadataV3::Array(array) => {
                vars.push(v3_var_summary(child_name, &array, &node_json)?);
            }
            NodeMetadataV3::Group(group) => {
                children.push(walk_v3_group(&child_path, child_name, group, guard)?);
            }
        }
    }
    guard.leave();

    Ok(build_group_summary(name, &own.attributes, vars, children))
}

pub(crate) fn v3_var_summary(
    name: String,
    array: &ArrayMetadataV3,
    node_path: &Path,
) -> Result<VarSummary, MetaError> {
    let shape = array.shape.clone();
    let dtype = v3_dtype_string(&array.data_type);
    let chunks = v3_chunk_shape(&array.chunk_grid, node_path)?;
    let dims = resolve_dim_names(&array.dimension_names, shape.len());

    Ok(VarSummary {
        name,
        dtype,
        dims,
        shape,
        chunks,
        attrs: json_object_to_attrs(&array.attributes),
        preview: None,
    })
}

/// Numpy-style dtype string for a Zarr v3 `data_type`. The plain scalar
/// data type names Zarr v3 defines (`float32`, `int64`, `bool`, ...) already
/// match numpy's dtype names verbatim; `"string"` (Zarr v3's variable-length
/// UTF-8 type) has no fixed-width numpy equivalent and is reported as
/// `object`, matching how xarray displays such arrays. Anything else
/// (structured/extension data types) is reported by its extension name only:
/// `MetadataV3` doesn't expose its raw JSON, only `name()`/`configuration()`.
fn v3_dtype_string(data_type: &MetadataV3) -> String {
    match data_type.name() {
        "string" => "object".to_owned(),
        other => other.to_owned(),
    }
}

/// Extracts the chunk shape from a v3 array's `chunk_grid` metadata. Only
/// the `"regular"` chunk grid (the only kind Zarr v3 currently defines) has
/// a `chunk_shape`; anything else yields `None` rather than an error, since
/// this reader never needs to interpret chunk layout beyond display.
fn v3_chunk_shape(
    chunk_grid: &MetadataV3,
    node_path: &Path,
) -> Result<Option<Vec<u64>>, MetaError> {
    if chunk_grid.name() != "regular" {
        return Ok(None);
    }
    let Some(config) = chunk_grid.configuration() else {
        return Ok(None);
    };
    let Some(value) = config.get("chunk_shape") else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|source| MetaError::Json {
            path: node_path.to_path_buf(),
            source,
        })
}

/// Resolves an array's per-axis dimension names against its actual rank
/// (`ndim`), shared by the Zarr v3 `dimension_names` field and the Zarr v2
/// `_ARRAY_DIMENSIONS` attribute convention.
///
/// `names` is optional to begin with (v3's `dimension_names` may be absent
/// entirely; v2's `_ARRAY_DIMENSIONS` may be missing or unparsable), and
/// even when present individual axes may carry no name (v3: `null`; v2: any
/// non-string entry, which the caller maps to `None`). Either case falls
/// back to numpy/xarray's synthetic `dim_{i}` naming for that axis. The
/// result is always exactly `ndim` long: a shorter list is padded with
/// synthetic names, a longer one (which shouldn't occur for a
/// spec-conformant store, but has been seen with malformed
/// `_ARRAY_DIMENSIONS`) is truncated, so it always zips safely against
/// `shape`.
fn resolve_dim_names(names: &Option<Vec<Option<String>>>, ndim: usize) -> Vec<String> {
    match names {
        Some(names) => (0..ndim)
            .map(|i| {
                names
                    .get(i)
                    .and_then(Option::clone)
                    .unwrap_or_else(|| format!("dim_{i}"))
            })
            .collect(),
        None => (0..ndim).map(|i| format!("dim_{i}")).collect(),
    }
}

// ---------------------------------------------------------------------
// Zarr v2 (walked directly, node by node)
// ---------------------------------------------------------------------

fn walk_v2_group(
    dir: &Path,
    name: String,
    guard: &mut WalkGuard,
) -> Result<GroupSummary, MetaError> {
    guard.enter(dir)?;
    let attrs = read_v2_attrs(dir)?;

    let mut vars = Vec::new();
    let mut children = Vec::new();
    for entry in read_dir_sorted(dir)? {
        let child_path = entry.path();
        if !child_path.is_dir() {
            continue;
        }
        let child_name = entry.file_name().to_string_lossy().into_owned();
        if child_path.join(".zarray").is_file() {
            vars.push(v2_var_summary(child_name, &child_path)?);
        } else if child_path.join(".zgroup").is_file() {
            children.push(walk_v2_group(&child_path, child_name, guard)?);
        }
        // Anything else under a group directory isn't a Zarr node (there
        // are none in a v2 store at this level) and is skipped.
    }
    guard.leave();

    Ok(build_group_summary(name, &attrs, vars, children))
}

fn v2_var_summary(name: String, dir: &Path) -> Result<VarSummary, MetaError> {
    let array: ArrayMetadataV2 = read_json_as(&dir.join(".zarray"))?;
    let attrs = read_v2_attrs(dir)?;
    Ok(v2_var_from_parts(name, &array, &attrs))
}

/// Pure assembly of an already-parsed v2 array's metadata and attributes;
/// nothing here can fail, all I/O and JSON parsing happens in the callers.
fn v2_var_from_parts(
    name: String,
    array: &ArrayMetadataV2,
    attrs: &serde_json::Map<String, Value>,
) -> VarSummary {
    let array_dimensions = attrs
        .get("_ARRAY_DIMENSIONS")
        .and_then(Value::as_array)
        .map(|dims| {
            dims.iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        });
    let dims = resolve_dim_names(&array_dimensions, array.shape.len());

    VarSummary {
        name,
        dtype: v2_dtype_from_metadata(&array.dtype),
        dims,
        shape: array.shape.clone(),
        chunks: Some(array.chunks.iter().map(|n| n.get()).collect()),
        attrs: json_object_to_attrs(attrs),
        preview: None,
    }
}

/// Numpy-style dtype string for a Zarr v2 `dtype`. A `Simple` dtype is a
/// typestring (`<f8`, `|S8`, ...) handled by [`v2_dtype_string`]; a
/// `Structured` dtype (a list of `[fieldname, datatype, shape?]` entries)
/// has no single numpy-style name, so — matching how other exotic/extension
/// data types are reported elsewhere in this reader — it's reported simply
/// as `compound` rather than dumping its raw per-field JSON.
fn v2_dtype_from_metadata(dtype: &DataTypeMetadataV2) -> String {
    match dtype {
        DataTypeMetadataV2::Simple(s) => v2_dtype_string(s),
        DataTypeMetadataV2::Structured(_) => "compound".to_owned(),
    }
}

/// Numpy-style dtype string from a Zarr v2 typestring (e.g. `<f4`, `|S8`,
/// `<i8`, `<M8[ns]`). The leading byte-order char (`<`/`>`/`=`/`|`) is
/// dropped; the kind char plus item size in bytes is expanded into numpy's
/// spelled-out name (`f4` → `float32`), matching netCDF's `dtype_string`
/// convention, and datetime/timedelta typestrings become
/// `datetime64[unit]`/`timedelta64[unit]` as xarray displays them. Anything
/// not matching this scheme (rare dtypes, malformed strings) is passed
/// through verbatim.
///
/// Works on `char`s rather than byte offsets: `.zarray` contents are
/// untrusted input, and slicing a typestring that happens to start with a
/// multibyte character at a byte offset would panic.
fn v2_dtype_string(dtype: &str) -> String {
    let mut chars = dtype.chars();
    let Some(first) = chars.next() else {
        return dtype.to_owned();
    };
    let kind = if matches!(first, '<' | '>' | '=' | '|') {
        match chars.next() {
            Some(kind) => kind,
            None => return dtype.to_owned(),
        }
    } else {
        first
    };
    let rest = chars.as_str();

    if matches!(kind, 'M' | 'm') {
        let base = if kind == 'M' {
            "datetime64"
        } else {
            "timedelta64"
        };
        return match rest {
            // Generic (unit-less) datetime64 / timedelta64.
            "8" => base.to_owned(),
            _ => match rest.strip_prefix("8[").and_then(|r| r.strip_suffix(']')) {
                Some(unit) if !unit.is_empty() => format!("{base}[{unit}]"),
                _ => dtype.to_owned(),
            },
        };
    }

    let Ok(count) = rest.parse::<usize>() else {
        return dtype.to_owned();
    };
    match kind {
        'f' => format!("float{}", count * 8),
        'i' => format!("int{}", count * 8),
        'u' => format!("uint{}", count * 8),
        'c' => format!("complex{}", count * 8),
        'b' => "bool".to_owned(),
        'S' => format!("|S{count}"),
        'U' => format!("<U{count}"),
        _ => dtype.to_owned(),
    }
}

// ---------------------------------------------------------------------
// Zarr v2, consolidated (`.zmetadata`) — read once, no further disk walk
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConsolidatedMetadata {
    metadata: BTreeMap<String, Value>,
}

/// Splits a `.zmetadata` key into the node's relative path and the per-node
/// file it names: `g/arr/.zarray` → `("g/arr", ".zarray")`, and a root-level
/// `.zgroup` → `("", ".zgroup")`.
fn split_consolidated_key(key: &str) -> (&str, &str) {
    match key.rsplit_once('/') {
        Some((node_path, file)) => (node_path, file),
        None => ("", key),
    }
}

fn summarize_v2_consolidated(store_dir: &Path) -> Result<DatasetSummary, MetaError> {
    let meta_path = store_dir.join(".zmetadata");
    let consolidated: ConsolidatedMetadata = read_json_as(&meta_path)?;

    // Sorted so that `build_v2_consolidated_group` can find a group's
    // children with a range scan instead of filtering every entry per group.
    let mut group_paths: BTreeSet<String> = BTreeSet::new();
    let mut arrays: BTreeMap<String, ArrayMetadataV2> = BTreeMap::new();
    let mut attrs: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();

    for (key, value) in consolidated.metadata {
        let (node_path, file) = split_consolidated_key(&key);
        match file {
            ".zgroup" => {
                group_paths.insert(node_path.to_owned());
            }
            ".zarray" => {
                let array: ArrayMetadataV2 =
                    serde_json::from_value(value).map_err(|source| MetaError::Json {
                        path: meta_path.clone(),
                        source,
                    })?;
                arrays.insert(node_path.to_owned(), array);
            }
            ".zattrs" => {
                if let Some(map) = value.as_object() {
                    attrs.insert(node_path.to_owned(), map.clone());
                }
            }
            // `.zmetadata` itself (nested, shouldn't occur) and any other
            // key is ignored: only the three per-node file kinds above are
            // meaningful.
            _ => {}
        }
    }

    // Ensure the root is always treated as a group even if `.zmetadata`
    // happened not to carry an explicit root `.zgroup` entry.
    group_paths.insert(String::new());

    let root =
        build_v2_consolidated_group(String::new(), String::new(), &arrays, &attrs, &group_paths);

    Ok(DatasetSummary {
        format: SourceFormat::ZarrV2,
        root,
        version_info: None,
    })
}

fn build_v2_consolidated_group(
    node_path: String,
    name: String,
    arrays: &BTreeMap<String, ArrayMetadataV2>,
    attrs: &BTreeMap<String, serde_json::Map<String, Value>>,
    group_paths: &BTreeSet<String>,
) -> GroupSummary {
    let empty = serde_json::Map::new();
    let own_attrs = attrs.get(&node_path).unwrap_or(&empty);

    let prefix = if node_path.is_empty() {
        String::new()
    } else {
        format!("{node_path}/")
    };

    // Direct-child array nodes: keys under this prefix with no further "/".
    let vars: Vec<VarSummary> = keys_under(arrays, &prefix)
        .filter_map(|(path, array)| {
            let rest = &path[prefix.len()..];
            if rest.contains('/') {
                return None;
            }
            let var_attrs = attrs.get(path).unwrap_or(&empty);
            Some(v2_var_from_parts(rest.to_owned(), array, var_attrs))
        })
        .collect();

    // Direct-child group nodes: any path under this prefix with no further
    // "/" that isn't itself an array — discovered from the union of
    // `.zgroup`, `.zattrs`, and `.zarray` paths whose parent is this node,
    // since a pure-group node may carry no attrs entry of its own (empty
    // attributes) or, in principle, no `.zgroup` entry either.
    //
    // A path is only excluded here when it's a *direct-child array* (no
    // further "/" and itself an array path, already collected into `vars`
    // above); a deeper array path (e.g. `g/arr/.zarray` when this node is
    // the root) still names `g` as an implicit child group even though `g`
    // itself carries no `.zgroup`/`.zattrs` entry of its own — dropping such
    // paths outright (as opposed to only direct-child arrays) would silently
    // lose that whole subtree.
    let group_paths_under = group_paths
        .range::<str, _>((Bound::Excluded(prefix.as_str()), Bound::Unbounded))
        .take_while(|path| path.starts_with(&prefix));
    let mut child_names: BTreeMap<String, String> = BTreeMap::new();
    for path in group_paths_under
        .chain(keys_under(attrs, &prefix).map(|(path, _)| path))
        .chain(keys_under(arrays, &prefix).map(|(path, _)| path))
    {
        let rest = &path[prefix.len()..];
        let is_direct_child_array = !rest.contains('/') && arrays.contains_key(path);
        if is_direct_child_array {
            continue;
        }
        let child = match rest.split_once('/') {
            Some((first, _)) => first,
            None => rest,
        };
        let child_path = format!("{prefix}{child}");
        child_names.insert(child_path, child.to_owned());
    }

    let children = child_names
        .into_iter()
        .map(|(child_path, child_name)| {
            build_v2_consolidated_group(child_path, child_name, arrays, attrs, group_paths)
        })
        .collect();

    build_group_summary(name, own_attrs, vars, children)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use zarrs_metadata::v3::FillValueMetadataV3;

    use super::*;

    /// Creates a fresh, empty temporary directory named `name` (e.g.
    /// `"root.zarr"`) for a test store to write into, nested under a
    /// process- and call-unique parent so parallel test runs never collide
    /// and so `name` alone (not some disambiguating suffix) is what
    /// `Path::file_stem` sees.
    fn temp_store_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the epoch")
            .as_nanos();
        let parent = std::env::temp_dir().join(format!(
            "gridlook-meta-zarr-test-{}-{nanos}-{n}",
            std::process::id()
        ));
        let dir = parent.join(name);
        fs::create_dir_all(&dir).expect("create temp store dir");
        dir
    }

    /// Minimal, valid Zarr v2 array metadata for a single 1-D array of
    /// length 2, used by tests that only care about the array's *presence*
    /// and name, not its dtype/shape/chunking details.
    fn tiny_v2_array() -> ArrayMetadataV2 {
        ArrayMetadataV2::new(
            vec![2],
            vec![NonZeroU64::new(2).expect("2 is nonzero")],
            DataTypeMetadataV2::Simple("<f4".to_owned()),
            FillValueMetadataV3::Null,
            None,
            None,
        )
    }

    #[test]
    fn resolve_dim_names_substitutes_missing_and_truncates_overlong() {
        let sparse = Some(vec![Some("time".to_owned()), None, Some("y".to_owned())]);
        assert_eq!(resolve_dim_names(&sparse, 3), vec!["time", "dim_1", "y"]);

        let overlong = Some(vec![
            Some("a".to_owned()),
            Some("b".to_owned()),
            Some("c".to_owned()),
        ]);
        assert_eq!(resolve_dim_names(&overlong, 2), vec!["a", "b"]);

        assert_eq!(
            resolve_dim_names(&None, 2),
            vec!["dim_0".to_owned(), "dim_1".to_owned()]
        );
    }

    fn v3_group_node() -> NodeMetadataV3 {
        NodeMetadataV3::Group(GroupMetadataV3::default())
    }

    fn v3_array_node() -> NodeMetadataV3 {
        let chunk_grid = MetadataV3::new_with_serializable_configuration(
            "regular".to_owned(),
            &serde_json::json!({ "chunk_shape": [2] }),
        )
        .expect("build regular chunk grid metadata");
        NodeMetadataV3::Array(ArrayMetadataV3::new(
            vec![2],
            chunk_grid,
            MetadataV3::new("float32"),
            FillValueMetadataV3::Null,
            Vec::new(),
        ))
    }

    fn names(vars: &[VarSummary]) -> Vec<&str> {
        vars.iter().map(|v| v.name.as_str()).collect()
    }

    #[test]
    fn keys_under_yields_only_the_prefix_block() {
        let map: BTreeMap<String, ()> = ["", "a", "a-", "a/b", "a/b/c", "a/x", "ab", "b"]
            .into_iter()
            .map(|k| (k.to_owned(), ()))
            .collect();
        let under_a: Vec<&str> = keys_under(&map, "a/").map(|(k, _)| k.as_str()).collect();
        assert_eq!(under_a, vec!["a/b", "a/b/c", "a/x"]);
        // The root prefix covers everything except the root itself.
        let under_root: Vec<&str> = keys_under(&map, "").map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            under_root,
            vec!["a", "a-", "a/b", "a/b/c", "a/x", "ab", "b"]
        );
        assert_eq!(keys_under(&map, "zzz/").count(), 0);
    }

    #[test]
    fn build_v3_tree_recovers_nesting_from_flat_paths() {
        let nodes: BTreeMap<String, NodeMetadataV3> = [
            ("", v3_group_node()),
            ("a", v3_group_node()),
            ("a/b", v3_group_node()),
            ("a/b/deep", v3_array_node()),
            ("a/x", v3_array_node()),
            // Shares the byte prefix "a" but is not under "a/".
            ("ab", v3_array_node()),
            ("c", v3_array_node()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();

        let root = build_v3_tree(&nodes, Path::new("store.zarr")).expect("build tree");
        assert_eq!(names(&root.data_vars), vec!["ab", "c"]);
        assert_eq!(root.children.len(), 1);
        let a = &root.children[0];
        assert_eq!(a.name, "a");
        assert_eq!(names(&a.data_vars), vec!["x"]);
        assert_eq!(a.children.len(), 1);
        let b = &a.children[0];
        assert_eq!(b.name, "b");
        assert_eq!(names(&b.data_vars), vec!["deep"]);
        assert!(b.children.is_empty());
    }

    #[test]
    fn build_v3_tree_without_a_root_entry_yields_an_empty_root_group() {
        let nodes: BTreeMap<String, NodeMetadataV3> =
            [("only".to_owned(), v3_array_node())].into_iter().collect();
        let root = build_v3_tree(&nodes, Path::new("store.zarr")).expect("build tree");
        assert!(root.attrs.is_empty());
        assert_eq!(names(&root.data_vars), vec!["only"]);
    }

    #[test]
    fn split_consolidated_key_separates_node_path_from_file() {
        assert_eq!(split_consolidated_key(".zgroup"), ("", ".zgroup"));
        assert_eq!(split_consolidated_key("g/.zattrs"), ("g", ".zattrs"));
        assert_eq!(
            split_consolidated_key("g/arr/.zarray"),
            ("g/arr", ".zarray")
        );
        // Not a per-node file name: falls through to the ignored branch.
        assert_eq!(split_consolidated_key("foo.zgroup"), ("", "foo.zgroup"));
    }

    #[test]
    fn v2_dtype_string_expands_numeric_and_string_typestrings() {
        assert_eq!(v2_dtype_string("<f4"), "float32");
        assert_eq!(v2_dtype_string(">f8"), "float64");
        assert_eq!(v2_dtype_string("<i8"), "int64");
        assert_eq!(v2_dtype_string("|u1"), "uint8");
        assert_eq!(v2_dtype_string("<c16"), "complex128");
        assert_eq!(v2_dtype_string("|b1"), "bool");
        assert_eq!(v2_dtype_string("|S8"), "|S8");
        assert_eq!(v2_dtype_string("<U3"), "<U3");
        // No byte-order prefix is fine too.
        assert_eq!(v2_dtype_string("f4"), "float32");
    }

    #[test]
    fn v2_dtype_string_maps_datetime_and_timedelta_typestrings() {
        assert_eq!(v2_dtype_string("<M8[ns]"), "datetime64[ns]");
        assert_eq!(v2_dtype_string("<M8[us]"), "datetime64[us]");
        assert_eq!(v2_dtype_string("<m8[s]"), "timedelta64[s]");
        assert_eq!(v2_dtype_string("<M8"), "datetime64");
        // Malformed unit brackets pass through untouched.
        assert_eq!(v2_dtype_string("<M8[ns"), "<M8[ns");
        assert_eq!(v2_dtype_string("<M8[]"), "<M8[]");
    }

    #[test]
    fn v2_dtype_string_passes_through_unrecognized_input_without_panicking() {
        assert_eq!(v2_dtype_string(""), "");
        assert_eq!(v2_dtype_string("<"), "<");
        assert_eq!(v2_dtype_string("<V16"), "<V16");
        assert_eq!(v2_dtype_string("|O"), "|O");
        // Multibyte first/second characters used to panic on a byte-offset
        // slice that fell inside the character.
        assert_eq!(v2_dtype_string("\u{e9}4"), "\u{e9}4");
        assert_eq!(v2_dtype_string("<\u{fc}8"), "<\u{fc}8");
        assert_eq!(v2_dtype_string("\u{1f600}"), "\u{1f600}");
    }

    #[test]
    fn v2_structured_dtype_displays_as_compound() {
        let dtype = DataTypeMetadataV2::Structured(Vec::new());
        assert_eq!(v2_dtype_from_metadata(&dtype), "compound");
    }

    #[test]
    fn root_array_name_falls_back_when_no_usable_stem() {
        assert_eq!(root_array_name(Path::new("root.zarr")), "root");
        assert_eq!(root_array_name(Path::new("/")), "array");
        assert_eq!(root_array_name(Path::new(".")), "array");
    }

    #[test]
    fn root_is_array_v2_uses_directory_stem_as_var_name() {
        let dir = temp_store_dir("root.zarr");
        let array = tiny_v2_array();
        fs::write(
            dir.join(".zarray"),
            serde_json::to_string(&array).expect("serialize array metadata"),
        )
        .expect("write .zarray");

        let summary = summarize_zarr(&dir).expect("summarize root-is-array v2 store");
        let names: Vec<&str> = summary
            .root
            .data_vars
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(names, vec!["root"]);

        let _ = fs::remove_dir_all(dir.parent().expect("has a parent"));
    }

    #[test]
    fn root_is_array_v3_uses_directory_stem_as_var_name() {
        let dir = temp_store_dir("mydata.zarr");
        let chunk_grid = MetadataV3::new_with_serializable_configuration(
            "regular".to_owned(),
            &serde_json::json!({ "chunk_shape": [2] }),
        )
        .expect("build regular chunk grid metadata");
        let mut array = ArrayMetadataV3::new(
            vec![2],
            chunk_grid,
            MetadataV3::new("float32"),
            FillValueMetadataV3::Null,
            Vec::new(),
        );
        array
            .attributes
            .insert("units".to_owned(), serde_json::json!("m"));
        fs::write(
            dir.join("zarr.json"),
            serde_json::to_string(&array).expect("serialize array metadata"),
        )
        .expect("write zarr.json");

        let summary = summarize_zarr(&dir).expect("summarize root-is-array v3 store");
        let names: Vec<&str> = summary
            .root
            .data_vars
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(names, vec!["mydata"]);

        // The array's attributes are the variable's, not the synthetic root
        // group's: they used to be attached to both and so rendered twice.
        assert_eq!(
            summary.root.data_vars[0].attrs,
            vec![("units".to_owned(), AttrValue::Text("m".to_owned()))]
        );
        assert!(
            summary.root.attrs.is_empty(),
            "root group must not duplicate the array's attrs, got {:?}",
            summary.root.attrs
        );

        let _ = fs::remove_dir_all(dir.parent().expect("has a parent"));
    }

    /// A symlink loop inside a group (`loop -> .`) makes `loop/zarr.json`
    /// resolve to the group's own metadata. Unguarded, the walk re-entered
    /// the group until the kernel's symlink-traversal limit hit (`ELOOP`),
    /// yielding ~40 nested phantom `loop` groups. It must instead stop with
    /// an error naming the loop.
    #[cfg(unix)]
    #[test]
    fn v3_symlink_loop_is_reported_instead_of_walked() {
        let dir = temp_store_dir("loop_v3.zarr");
        fs::write(
            dir.join("zarr.json"),
            r#"{"zarr_format":3,"node_type":"group","attributes":{}}"#,
        )
        .expect("write root zarr.json");
        std::os::unix::fs::symlink(".", dir.join("loop")).expect("create symlink loop");

        let err = summarize_zarr(&dir).expect_err("a symlink loop must be an error");
        assert!(
            matches!(err, MetaError::Invalid { .. }),
            "expected MetaError::Invalid, got {err:?}"
        );
        assert!(
            err.to_string().contains("symlink loop"),
            "error should hint at the cause, got: {err}"
        );

        let _ = fs::remove_dir_all(dir.parent().expect("has a parent"));
    }

    #[cfg(unix)]
    #[test]
    fn v2_symlink_loop_is_reported_instead_of_walked() {
        let dir = temp_store_dir("loop_v2.zarr");
        fs::write(dir.join(".zgroup"), r#"{"zarr_format":2}"#).expect("write root .zgroup");
        std::os::unix::fs::symlink(".", dir.join("loop")).expect("create symlink loop");

        let err = summarize_zarr(&dir).expect_err("a symlink loop must be an error");
        assert!(
            matches!(err, MetaError::Invalid { .. }),
            "expected MetaError::Invalid, got {err:?}"
        );

        let _ = fs::remove_dir_all(dir.parent().expect("has a parent"));
    }

    /// A symlink to a *sibling* group is not a loop: the target is walked
    /// once more under the link's name, which is odd but finite and not the
    /// guard's business.
    #[cfg(unix)]
    #[test]
    fn v3_symlink_to_sibling_group_is_not_a_loop() {
        let dir = temp_store_dir("sibling_v3.zarr");
        let group = r#"{"zarr_format":3,"node_type":"group","attributes":{}}"#;
        fs::write(dir.join("zarr.json"), group).expect("write root zarr.json");
        fs::create_dir(dir.join("a")).expect("create a");
        fs::write(dir.join("a/zarr.json"), group).expect("write a/zarr.json");
        std::os::unix::fs::symlink("a", dir.join("b")).expect("symlink b -> a");

        let summary = summarize_zarr(&dir).expect("sibling symlink must summarize");
        let names: Vec<&str> = summary
            .root
            .children
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b"]);

        let _ = fs::remove_dir_all(dir.parent().expect("has a parent"));
    }

    #[test]
    fn walk_guard_caps_nesting_depth() {
        let mut guard = WalkGuard::default();
        let dir = temp_store_dir("deep.zarr");
        // Re-entering the *same* directory is a loop, so give each level a
        // distinct (nonexistent, hence non-canonicalizable) path.
        for i in 0..MAX_GROUP_DEPTH {
            guard
                .enter(&dir.join(format!("level{i}")))
                .expect("within the cap");
        }
        let err = guard
            .enter(&dir.join("one-too-many"))
            .expect_err("past the cap");
        assert!(
            err.to_string().contains("deeper than"),
            "unexpected error: {err}"
        );

        let _ = fs::remove_dir_all(dir.parent().expect("has a parent"));
    }

    /// Regression test for a consolidated v2 store where an intermediate
    /// group (`g`) has no `.zgroup`/`.zattrs` entry of its own in
    /// `.zmetadata` — only its array `g/arr` does. `g` must still be
    /// discovered as an implicit child group of the root, with `arr` inside
    /// it, rather than being silently dropped.
    #[test]
    fn consolidated_v2_discovers_implicit_child_group() {
        let dir = temp_store_dir("implicit.zarr");
        let array = tiny_v2_array();

        let mut metadata = serde_json::Map::new();
        metadata.insert(
            ".zgroup".to_owned(),
            serde_json::json!({ "zarr_format": 2 }),
        );
        metadata.insert(
            "g/arr/.zarray".to_owned(),
            serde_json::to_value(&array).expect("serialize array metadata"),
        );
        let doc = serde_json::json!({ "metadata": metadata });
        fs::write(
            dir.join(".zmetadata"),
            serde_json::to_string(&doc).expect("serialize .zmetadata"),
        )
        .expect("write .zmetadata");

        let summary = summarize_zarr(&dir).expect("summarize consolidated v2 store");
        let g = summary
            .root
            .children
            .iter()
            .find(|c| c.name == "g")
            .expect("implicit group \"g\" is discovered");
        assert!(
            g.data_vars.iter().any(|v| v.name == "arr"),
            "implicit group \"g\" contains its array \"arr\", got {:?}",
            g.data_vars
        );

        let _ = fs::remove_dir_all(dir.parent().expect("has a parent"));
    }
}
