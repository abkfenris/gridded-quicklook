//! Zarr directory-store metadata reader.
//!
//! Walks a Zarr v2 or v3 directory store's node metadata (`zarr.json`, or
//! `.zgroup`/`.zarray`/`.zattrs`, or a consolidated `.zmetadata`) into a
//! format-agnostic [`DatasetSummary`]. Only the small per-node JSON metadata
//! files are read; chunk data is never opened, so previews are always
//! omitted (see [`summarize_zarr`] for details).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use zarrs_metadata::v2::{ArrayMetadataV2, DataTypeMetadataV2};
use zarrs_metadata::v3::{ArrayMetadataV3, GroupMetadataV3, MetadataV3, NodeMetadataV3};

use crate::model::{AttrValue, DatasetSummary, DimInfo, GroupSummary, SourceFormat, VarSummary};
use crate::netcdf::MetaError;

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
            NodeMetadataV3::Group(_) => walk_v3_group(path, String::new())?,
            NodeMetadataV3::Array(array) => {
                let var = v3_var_summary(String::new(), &array, &v3_root)?;
                build_group_summary(String::new(), &array.attributes, vec![var], Vec::new())
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
        let root = walk_v2_group(path, String::new())?;
        return Ok(DatasetSummary {
            format: SourceFormat::ZarrV2,
            root,
            version_info: None,
        });
    }

    if path.join(".zarray").is_file() {
        let var = v2_var_summary(String::new(), path)?;
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

// ---------------------------------------------------------------------
// Shared group-building logic (used by both v2 and v3 readers)
// ---------------------------------------------------------------------

/// Classifies variables into coords/data_vars (xarray's heuristic: a
/// variable is a coordinate if its name matches one of its own dimensions,
/// or if it is named in some sibling variable's `coordinates` attribute),
/// derives the group's dims from the union of its direct variables' own
/// dims, and sorts everything deterministically by name.
pub(crate) fn build_group_summary(
    name: String,
    attrs: &serde_json::Map<String, Value>,
    vars: Vec<VarSummary>,
    mut children: Vec<GroupSummary>,
) -> GroupSummary {
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

    // Zarr has no group-level dimension registry like netCDF; a group's
    // dims are derived from the union of its direct variables' own
    // (name, size) pairs. `is_unlimited` is always false: Zarr arrays have
    // no notion of an unlimited/appendable dimension distinct from `shape`.
    let mut dims_map: BTreeMap<String, u64> = BTreeMap::new();
    for var in coords.iter().chain(data_vars.iter()) {
        for (dim_name, size) in var.dims.iter().zip(var.shape.iter()) {
            dims_map.entry(dim_name.clone()).or_insert(*size);
        }
    }
    let dims = dims_map
        .into_iter()
        .map(|(name, size)| DimInfo {
            name,
            size,
            is_unlimited: false,
        })
        .collect();

    children.sort_by(|a, b| a.name.cmp(&b.name));

    GroupSummary {
        name,
        dims,
        coords,
        data_vars,
        attrs: json_object_to_attrs(attrs),
        children,
    }
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

// ---------------------------------------------------------------------
// JSON file I/O helpers
// ---------------------------------------------------------------------

fn read_json_value(path: &Path) -> Result<Value, MetaError> {
    let text = fs::read_to_string(path).map_err(|source| MetaError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| MetaError::Json {
        path: path.to_path_buf(),
        source,
    })
}

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

/// Like [`read_json_value`], but returns `Ok(None)` if the file is simply
/// absent (e.g. an optional `.zattrs`) rather than treating that as an I/O
/// error.
fn read_json_value_opt(path: &Path) -> Result<Option<Value>, MetaError> {
    if !path.is_file() {
        return Ok(None);
    }
    read_json_value(path).map(Some)
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

fn walk_v3_group(dir: &Path, name: String) -> Result<GroupSummary, MetaError> {
    let own: NodeMetadataV3 = read_json_as(&dir.join("zarr.json"))?;

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
            NodeMetadataV3::Group(_) => children.push(walk_v3_group(&child_path, child_name)?),
        }
    }

    Ok(build_group_summary(
        name,
        v3_node_attrs(&own),
        vars,
        children,
    ))
}

pub(crate) fn v3_var_summary(
    name: String,
    array: &ArrayMetadataV3,
    node_path: &Path,
) -> Result<VarSummary, MetaError> {
    let shape = array.shape.clone();
    let dtype = v3_dtype_string(&array.data_type);
    let chunks = v3_chunk_shape(&array.chunk_grid, node_path)?;
    let dims = resolve_v3_dim_names(&array.dimension_names, shape.len());

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

/// Resolves a v3 array's per-axis dimension names. `dimension_names` is
/// optional in the spec, and even when present individual axes may be
/// `null`; either case falls back to numpy/xarray's synthetic `dim_{i}`
/// naming for that axis.
fn resolve_v3_dim_names(names: &Option<Vec<Option<String>>>, ndim: usize) -> Vec<String> {
    match names {
        Some(names) => names
            .iter()
            .enumerate()
            .map(|(i, n)| n.clone().unwrap_or_else(|| format!("dim_{i}")))
            .collect(),
        None => (0..ndim).map(|i| format!("dim_{i}")).collect(),
    }
}

// ---------------------------------------------------------------------
// Zarr v2 (walked directly, node by node)
// ---------------------------------------------------------------------

fn walk_v2_group(dir: &Path, name: String) -> Result<GroupSummary, MetaError> {
    let attrs = read_json_value_opt(&dir.join(".zattrs"))?
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

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
            children.push(walk_v2_group(&child_path, child_name)?);
        }
        // Anything else under a group directory isn't a Zarr node (there
        // are none in a v2 store at this level) and is skipped.
    }

    Ok(build_group_summary(name, &attrs, vars, children))
}

fn v2_var_summary(name: String, dir: &Path) -> Result<VarSummary, MetaError> {
    let array: ArrayMetadataV2 = read_json_as(&dir.join(".zarray"))?;
    let attrs = read_json_value_opt(&dir.join(".zattrs"))?
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    v2_var_from_parts(name, &array, &attrs)
}

fn v2_var_from_parts(
    name: String,
    array: &ArrayMetadataV2,
    attrs: &serde_json::Map<String, Value>,
) -> Result<VarSummary, MetaError> {
    let dims = attrs
        .get("_ARRAY_DIMENSIONS")
        .and_then(Value::as_array)
        .map(|dims| {
            dims.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| (0..array.shape.len()).map(|i| format!("dim_{i}")).collect());

    Ok(VarSummary {
        name,
        dtype: v2_dtype_from_metadata(&array.dtype),
        dims,
        shape: array.shape.clone(),
        chunks: Some(array.chunks.iter().map(|n| n.get()).collect()),
        attrs: json_object_to_attrs(attrs),
        preview: None,
    })
}

/// Numpy-style dtype string for a Zarr v2 `dtype`. A `Simple` dtype is a
/// typestring (`<f8`, `|S8`, ...) handled by [`v2_dtype_string`]; a
/// `Structured` dtype (a list of `[fieldname, datatype, shape?]` entries)
/// has no single numpy-style name, so it's passed through as its raw JSON.
fn v2_dtype_from_metadata(dtype: &DataTypeMetadataV2) -> String {
    match dtype {
        DataTypeMetadataV2::Simple(s) => v2_dtype_string(s),
        DataTypeMetadataV2::Structured(_) => dtype.to_string(),
    }
}

/// Numpy-style dtype string from a Zarr v2 typestring (e.g. `<f4`, `|S8`,
/// `<i8`). The leading byte-order char (`<`/`>`/`=`/`|`) is dropped; the
/// kind char plus item size in bytes is expanded into numpy's spelled-out
/// name (`f4` → `float32`), matching netCDF's `dtype_string` convention.
/// Anything not matching this scheme (rare/structured dtypes) is passed
/// through verbatim.
fn v2_dtype_string(dtype: &str) -> String {
    let bytes = dtype.as_bytes();
    if bytes.is_empty() {
        return dtype.to_owned();
    }
    let (kind, rest) = if matches!(bytes[0], b'<' | b'>' | b'=' | b'|') && bytes.len() > 1 {
        (bytes[1] as char, &dtype[2..])
    } else {
        (bytes[0] as char, &dtype[1..])
    };
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

fn summarize_v2_consolidated(store_dir: &Path) -> Result<DatasetSummary, MetaError> {
    let meta_path = store_dir.join(".zmetadata");
    let consolidated: ConsolidatedMetadata = read_json_as(&meta_path)?;

    let mut group_paths: HashSet<String> = HashSet::new();
    let mut arrays: HashMap<String, ArrayMetadataV2> = HashMap::new();
    let mut attrs: HashMap<String, serde_json::Map<String, Value>> = HashMap::new();

    for (key, value) in consolidated.metadata {
        if let Some(node_path) = key.strip_suffix("/.zgroup").or(match key.as_str() {
            ".zgroup" => Some(""),
            _ => None,
        }) {
            group_paths.insert(node_path.to_owned());
        } else if let Some(node_path) = key.strip_suffix("/.zarray").or(match key.as_str() {
            ".zarray" => Some(""),
            _ => None,
        }) {
            let array: ArrayMetadataV2 =
                serde_json::from_value(value).map_err(|source| MetaError::Json {
                    path: meta_path.clone(),
                    source,
                })?;
            arrays.insert(node_path.to_owned(), array);
        } else if let Some(node_path) = key.strip_suffix("/.zattrs").or(match key.as_str() {
            ".zattrs" => Some(""),
            _ => None,
        }) {
            if let Some(map) = value.as_object() {
                attrs.insert(node_path.to_owned(), map.clone());
            }
        }
        // `.zmetadata` itself (nested, shouldn't occur) and any other key is
        // ignored: only the three per-node file kinds above are meaningful.
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
    arrays: &HashMap<String, ArrayMetadataV2>,
    attrs: &HashMap<String, serde_json::Map<String, Value>>,
    group_paths: &HashSet<String>,
) -> GroupSummary {
    let empty = serde_json::Map::new();
    let own_attrs = attrs.get(&node_path).unwrap_or(&empty);

    let prefix = if node_path.is_empty() {
        String::new()
    } else {
        format!("{node_path}/")
    };

    // Direct-child array nodes: keys under this prefix with no further "/".
    let mut vars: Vec<VarSummary> = arrays
        .iter()
        .filter_map(|(path, array)| {
            let rest = path.strip_prefix(&prefix)?;
            if rest.is_empty() || rest.contains('/') {
                return None;
            }
            let var_attrs = attrs.get(path).unwrap_or(&empty);
            v2_var_from_parts(rest.to_owned(), array, var_attrs).ok()
        })
        .collect();
    vars.sort_by(|a, b| a.name.cmp(&b.name));

    // Direct-child group nodes: any path under this prefix with no further
    // "/" that isn't itself an array — discovered from the union of
    // `.zgroup`, `.zattrs`, and `.zarray` paths whose parent is this node,
    // since a pure-group node may carry no attrs entry of its own (empty
    // attributes) or, in principle, no `.zgroup` entry either.
    let mut child_names: BTreeMap<String, String> = BTreeMap::new();
    for path in group_paths.iter().chain(attrs.keys()).chain(arrays.keys()) {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() || arrays.contains_key(path) {
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
