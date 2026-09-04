//! Zarr store metadata reader.
//!
//! Reads a Zarr v2 or v3 store's node metadata (`zarr.json`, or
//! `.zgroup`/`.zarray`/`.zattrs`, or consolidated metadata) into a
//! format-agnostic [`DatasetSummary`]. Only the small per-node JSON
//! documents are read; chunk data is never opened, so previews are always
//! omitted (see [`summarize_zarr`] for details).
//!
//! All access goes through the [`ZarrStore`] trait, so the same reader
//! serves local directories ([`FsStore`]) and, with the `remote` feature,
//! object stores.

mod consolidated;
pub(crate) mod store;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use zarrs_metadata::v2::{ArrayMetadataV2, ArrayMetadataV2Order, DataTypeMetadataV2, MetadataV2};
use zarrs_metadata::v3::{
    ArrayMetadataV3, FillValueMetadataV3, GroupMetadataV3, MetadataV3, NodeMetadataV3,
};

use crate::error::MetaError;
use crate::model::{
    AttrValue, CodecInfo, DatasetSummary, Endianness, FileInfo, GroupSummary, SourceFormat,
    StorageInfo, StorageLayout, SummarizeOptions, VarSummary,
};

pub(crate) use consolidated::{FlatNode, build_tree_from_flat};
pub(crate) use store::{FsStore, ZarrStore, join_key};

/// Deepest group nesting the directory walkers follow. No real hierarchy
/// is anywhere near this deep; the cap keeps a pathological store (a
/// symlink loop the store could not detect, say) from exhausting the small
/// stack of whatever thread Quick Look renders on.
const MAX_GROUP_DEPTH: usize = 64;

/// Errors when a walker is about to descend past [`MAX_GROUP_DEPTH`].
fn check_depth(depth: usize, store: &dyn ZarrStore, node_path: &str) -> Result<(), MetaError> {
    if depth > MAX_GROUP_DEPTH {
        return Err(MetaError::Invalid {
            location: store.describe(node_path),
            message: format!("group nesting deeper than {MAX_GROUP_DEPTH} levels"),
        });
    }
    Ok(())
}

/// Summarize the structure of a Zarr directory store at `path`.
///
/// Detects the store's flavor from what's present at `path`:
/// - `zarr.json` at the root → Zarr v3. If the root group carries inline
///   `consolidated_metadata` (zarr-python 3's convention), every node comes
///   from that; otherwise the directory tree is walked and every
///   subdirectory with its own `zarr.json` is a group or array node.
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
    summarize_zarr_with(path, &SummarizeOptions::default())
}

/// [`summarize_zarr`] with control over how much detail is gathered. With
/// [`SummarizeOptions::storage_details`] set, each variable's codec chain,
/// fill value, byte order and chunk-key encoding are recorded in
/// [`VarSummary::storage`] and the summary carries a [`FileInfo`].
pub fn summarize_zarr_with(
    path: &Path,
    opts: &SummarizeOptions,
) -> Result<DatasetSummary, MetaError> {
    summarize_zarr_store(&FsStore::new(path), opts)
}

/// Summarizes the Zarr hierarchy in any [`ZarrStore`].
pub(crate) fn summarize_zarr_store(
    store: &dyn ZarrStore,
    opts: &SummarizeOptions,
) -> Result<DatasetSummary, MetaError> {
    if let Some(bytes) = store.get("zarr.json")? {
        let node: NodeMetadataV3 = parse_json(&bytes, store, "zarr.json")?;
        let root = match node {
            NodeMetadataV3::Group(group) => match v3_consolidated_nodes(&group, store, opts)? {
                Some(nodes) => build_tree_from_flat(&nodes),
                None => walk_v3_group(store, "", String::new(), group, opts, 0)?,
            },
            NodeMetadataV3::Array(array) => {
                let var = v3_var_summary(
                    store.root_name(),
                    &array,
                    &store.describe("zarr.json"),
                    opts,
                )?;
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
        return Ok(finish(SourceFormat::ZarrV3, root, opts));
    }

    if let Some(bytes) = store.get(".zmetadata")? {
        let root = summarize_v2_consolidated(&bytes, store, opts)?;
        return Ok(finish(SourceFormat::ZarrV2, root, opts));
    }

    if store.get(".zgroup")?.is_some() {
        let root = walk_v2_group(store, "", String::new(), opts, 0)?;
        return Ok(finish(SourceFormat::ZarrV2, root, opts));
    }

    if let Some(bytes) = store.get(".zarray")? {
        let array: ArrayMetadataV2 = parse_json(&bytes, store, ".zarray")?;
        let attrs = read_v2_attrs(store, "")?;
        let var = v2_var_from_parts(store.root_name(), &array, &attrs, opts)?;
        let root = build_group_summary(
            String::new(),
            &serde_json::Map::new(),
            vec![var],
            Vec::new(),
        );
        return Ok(finish(SourceFormat::ZarrV2, root, opts));
    }

    Err(MetaError::Invalid {
        location: store.location().to_owned(),
        message: "not a recognizable Zarr store (missing zarr.json/.zgroup/.zarray/.zmetadata)"
            .to_owned(),
    })
}

/// Assembles the final summary, attaching a [`FileInfo`] when details were
/// requested.
fn finish(format: SourceFormat, root: GroupSummary, opts: &SummarizeOptions) -> DatasetSummary {
    let file_info = opts.storage_details.then(|| FileInfo {
        kind: zarr_kind_string(format).to_owned(),
        ..FileInfo::default()
    });
    DatasetSummary {
        format,
        root,
        version_info: None,
        file_info,
    }
}

/// The `ncdump -k`-style kind string for a Zarr-family format.
pub(crate) fn zarr_kind_string(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::ZarrV2 => "Zarr v2",
        SourceFormat::ZarrV3 => "Zarr v3",
        SourceFormat::Icechunk => "Icechunk",
        SourceFormat::NetCdf => "netCDF",
        SourceFormat::Hdf5 => "HDF5",
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
/// this attribute is known to carry the encoding. A 4-byte payload is a
/// `float32` and is kept as one.
fn decode_base64_float_attr(key: &str, value: &Value) -> Option<AttrValue> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    if key != "_FillValue" {
        return None;
    }
    let bytes = STANDARD.decode(value.as_str()?).ok()?;
    match bytes.len() {
        4 => Some(AttrValue::Float32(f32::from_le_bytes(
            bytes.try_into().ok()?,
        ))),
        8 => Some(AttrValue::Float(f64::from_le_bytes(bytes.try_into().ok()?))),
        _ => None,
    }
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
// JSON document helpers
// ---------------------------------------------------------------------

fn parse_json<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    store: &dyn ZarrStore,
    key: &str,
) -> Result<T, MetaError> {
    serde_json::from_slice(bytes).map_err(|source| MetaError::Json {
        location: store.describe(key),
        source,
    })
}

/// Reads and parses the document at `key`, `Ok(None)` if absent.
fn get_json<T: serde::de::DeserializeOwned>(
    store: &dyn ZarrStore,
    key: &str,
) -> Result<Option<T>, MetaError> {
    match store.get(key)? {
        Some(bytes) => parse_json(&bytes, store, key).map(Some),
        None => Ok(None),
    }
}

/// Reads a v2 node's optional `.zattrs`, defaulting to an empty map.
fn read_v2_attrs(
    store: &dyn ZarrStore,
    node_path: &str,
) -> Result<serde_json::Map<String, Value>, MetaError> {
    Ok(get_json::<Value>(store, &join_key(node_path, ".zattrs"))?
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default())
}

// ---------------------------------------------------------------------
// Zarr v3
// ---------------------------------------------------------------------

/// Inline consolidated metadata on a v3 root group, if present: zarr-python
/// 3 writes `"consolidated_metadata": {"kind": "inline", "metadata": {path:
/// node, ...}}` into the root `zarr.json`. `zarrs_metadata` has no typed
/// field for it, so it is dug out of the group's additional fields. Returns
/// the flat node list ready for [`build_tree_from_flat`], with the root
/// group itself at `""`.
fn v3_consolidated_nodes(
    group: &GroupMetadataV3,
    store: &dyn ZarrStore,
    opts: &SummarizeOptions,
) -> Result<Option<BTreeMap<String, FlatNode>>, MetaError> {
    #[derive(Deserialize)]
    struct Consolidated {
        #[serde(default)]
        kind: Option<String>,
        metadata: BTreeMap<String, Value>,
    }

    let Some(field) = group.additional_fields.get("consolidated_metadata") else {
        return Ok(None);
    };
    let consolidated: Consolidated =
        serde_json::from_value(field.as_value().clone()).map_err(|source| MetaError::Json {
            location: store.describe("zarr.json#consolidated_metadata"),
            source,
        })?;
    if consolidated.kind.as_deref().is_some_and(|k| k != "inline") {
        return Ok(None);
    }

    let mut nodes: BTreeMap<String, FlatNode> = BTreeMap::new();
    nodes.insert(
        String::new(),
        FlatNode::Group {
            attrs: group.attributes.clone(),
        },
    );
    for (path, value) in consolidated.metadata {
        let path = path.trim_matches('/').to_owned();
        let location = store.describe(&format!("zarr.json#consolidated_metadata/{path}"));
        let node: NodeMetadataV3 =
            serde_json::from_value(value).map_err(|source| MetaError::Json {
                location: location.clone(),
                source,
            })?;
        let leaf = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let flat = match node {
            NodeMetadataV3::Array(array) => {
                FlatNode::Array(Box::new(v3_var_summary(leaf, &array, &location, opts)?))
            }
            NodeMetadataV3::Group(g) => FlatNode::Group {
                attrs: g.attributes.clone(),
            },
        };
        nodes.insert(path, flat);
    }
    Ok(Some(nodes))
}

/// Walks a v3 group's children by listing `node_path`, given `own` — the
/// group's `zarr.json` already parsed by the caller — so each node's
/// document is read exactly once.
fn walk_v3_group(
    store: &dyn ZarrStore,
    node_path: &str,
    name: String,
    own: GroupMetadataV3,
    opts: &SummarizeOptions,
    depth: usize,
) -> Result<GroupSummary, MetaError> {
    check_depth(depth, store, node_path)?;
    let mut vars = Vec::new();
    let mut children = Vec::new();
    for child_name in store.list_dir(node_path)?.dirs {
        let child_path = join_key(node_path, &child_name);
        let node_key = join_key(&child_path, "zarr.json");
        // A subdirectory without a `zarr.json` is not a Zarr node (chunks
        // live inside array directories, not beside them) and is skipped.
        let Some(node) = get_json::<NodeMetadataV3>(store, &node_key)? else {
            continue;
        };
        match node {
            NodeMetadataV3::Array(array) => {
                vars.push(v3_var_summary(
                    child_name,
                    &array,
                    &store.describe(&node_key),
                    opts,
                )?);
            }
            NodeMetadataV3::Group(group) => {
                children.push(walk_v3_group(
                    store,
                    &child_path,
                    child_name,
                    group,
                    opts,
                    depth + 1,
                )?);
            }
        }
    }

    Ok(build_group_summary(name, &own.attributes, vars, children))
}

/// Summarizes one v3 array node. `location` names the node's document for
/// error messages.
pub(crate) fn v3_var_summary(
    name: String,
    array: &ArrayMetadataV3,
    location: &str,
    opts: &SummarizeOptions,
) -> Result<VarSummary, MetaError> {
    let shape = array.shape.clone();
    let dtype = v3_dtype_string(&array.data_type);
    let chunks = v3_chunk_shape(&array.chunk_grid, location)?;
    let dims = resolve_dim_names(&array.dimension_names, shape.len());
    let storage = opts.storage_details.then(|| v3_storage_info(array, &dtype));

    Ok(VarSummary {
        name,
        dtype,
        dims,
        shape,
        chunks,
        attrs: json_object_to_attrs(&array.attributes),
        preview: None,
        storage,
    })
}

/// Storage details of a v3 array: its codec chain, byte order (from the
/// `bytes` codec), metadata fill value and chunk-key encoding.
fn v3_storage_info(array: &ArrayMetadataV3, dtype: &str) -> StorageInfo {
    let codecs: Vec<CodecInfo> = array
        .codecs
        .iter()
        .map(|codec| CodecInfo {
            name: codec.name().to_owned(),
            configuration: configuration_value(codec.configuration().map(|c| &**c)),
        })
        .collect();
    let endianness = array
        .codecs
        .iter()
        .find(|codec| codec.name() == "bytes")
        .and_then(|codec| codec.configuration())
        .and_then(|config| config.get("endian"))
        .and_then(Value::as_str)
        .and_then(endianness_from_str);
    let separator = array
        .chunk_key_encoding
        .configuration()
        .and_then(|config| config.get("separator"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            // Spec defaults: "default" encoding uses "/", "v2" uses ".".
            if array.chunk_key_encoding.name() == "v2" {
                ".".to_owned()
            } else {
                "/".to_owned()
            }
        });

    StorageInfo {
        layout: Some(StorageLayout::Chunked),
        endianness,
        codecs,
        fill_value: fill_value_attr(&array.fill_value, dtype),
        chunk_key_encoding: Some(format!("{}{separator}", array.chunk_key_encoding.name())),
        ..StorageInfo::default()
    }
}

/// A codec/compressor configuration as a JSON object, or `None` when empty.
fn configuration_value(config: Option<&serde_json::Map<String, Value>>) -> Option<Value> {
    config
        .filter(|map| !map.is_empty())
        .map(|map| Value::Object(map.clone()))
}

fn endianness_from_str(s: &str) -> Option<Endianness> {
    match s {
        "little" => Some(Endianness::Little),
        "big" => Some(Endianness::Big),
        _ => None,
    }
}

/// Converts a Zarr metadata `fill_value` into an [`AttrValue`], typed to
/// match the array's `dtype` where that is unambiguous (`float32` fills
/// become `Float32` so renderers can print them as such). `null` (v2's "no
/// fill value") is `None`. Non-finite floats arrive as the strings `"NaN"`,
/// `"Infinity"`, `"-Infinity"`; hex byte strings and structured fills are
/// kept as text.
fn fill_value_attr(fill: &FillValueMetadataV3, dtype: &str) -> Option<AttrValue> {
    let is_f32 = dtype == "float32";
    let float = |f: f64| {
        if is_f32 {
            AttrValue::Float32(f as f32)
        } else {
            AttrValue::Float(f)
        }
    };
    let is_float_dtype = dtype.starts_with("float");
    Some(match fill {
        FillValueMetadataV3::Null => return None,
        FillValueMetadataV3::Bool(b) => AttrValue::Text(b.to_string()),
        FillValueMetadataV3::Number(n) => match n.as_i64() {
            Some(i) if !is_float_dtype => AttrValue::Int(i),
            _ => float(n.as_f64().unwrap_or_default()),
        },
        FillValueMetadataV3::String(s) => match s.as_str() {
            "NaN" => float(f64::NAN),
            "Infinity" => float(f64::INFINITY),
            "-Infinity" => float(f64::NEG_INFINITY),
            other => AttrValue::Text(other.to_owned()),
        },
        FillValueMetadataV3::Array(_) | FillValueMetadataV3::Object(_) => {
            AttrValue::Text(fill.to_string())
        }
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
fn v3_chunk_shape(chunk_grid: &MetadataV3, location: &str) -> Result<Option<Vec<u64>>, MetaError> {
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
            location: location.to_owned(),
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
    store: &dyn ZarrStore,
    node_path: &str,
    name: String,
    opts: &SummarizeOptions,
    depth: usize,
) -> Result<GroupSummary, MetaError> {
    check_depth(depth, store, node_path)?;
    let attrs = read_v2_attrs(store, node_path)?;

    let mut vars = Vec::new();
    let mut children = Vec::new();
    for child_name in store.list_dir(node_path)?.dirs {
        let child_path = join_key(node_path, &child_name);
        if let Some(array) = get_json::<ArrayMetadataV2>(store, &join_key(&child_path, ".zarray"))?
        {
            let attrs = read_v2_attrs(store, &child_path)?;
            vars.push(v2_var_from_parts(child_name, &array, &attrs, opts)?);
        } else if store.get(&join_key(&child_path, ".zgroup"))?.is_some() {
            children.push(walk_v2_group(
                store,
                &child_path,
                child_name,
                opts,
                depth + 1,
            )?);
        }
        // Anything else under a group directory isn't a Zarr node (there
        // are none in a v2 store at this level) and is skipped.
    }

    Ok(build_group_summary(name, &attrs, vars, children))
}

fn v2_var_from_parts(
    name: String,
    array: &ArrayMetadataV2,
    attrs: &serde_json::Map<String, Value>,
    opts: &SummarizeOptions,
) -> Result<VarSummary, MetaError> {
    let array_dimensions = attrs
        .get("_ARRAY_DIMENSIONS")
        .and_then(Value::as_array)
        .map(|dims| {
            dims.iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        });
    let dims = resolve_dim_names(&array_dimensions, array.shape.len());
    let dtype = v2_dtype_from_metadata(&array.dtype);
    let storage = opts.storage_details.then(|| v2_storage_info(array, &dtype));

    Ok(VarSummary {
        name,
        dtype,
        dims,
        shape: array.shape.clone(),
        chunks: Some(array.chunks.iter().map(|n| n.get()).collect()),
        attrs: json_object_to_attrs(attrs),
        preview: None,
        storage,
    })
}

/// Storage details of a v2 array: filters then compressor as the codec
/// chain, byte order from the typestring, memory order, metadata fill
/// value, and the dimension separator (expressed as a v3-style
/// `v2<separator>` chunk-key encoding for uniformity).
fn v2_storage_info(array: &ArrayMetadataV2, dtype: &str) -> StorageInfo {
    let codec_info = |codec: &MetadataV2| CodecInfo {
        name: codec.id().to_owned(),
        configuration: configuration_value(Some(&**codec.configuration())),
    };
    let codecs: Vec<CodecInfo> = array
        .filters
        .iter()
        .flatten()
        .chain(array.compressor.iter())
        .map(codec_info)
        .collect();
    let endianness = match &array.dtype {
        DataTypeMetadataV2::Simple(s) => match s.chars().next() {
            Some('<') => Some(Endianness::Little),
            Some('>') => Some(Endianness::Big),
            _ => None,
        },
        DataTypeMetadataV2::Structured(_) => None,
    };
    let separator: char = array.dimension_separator.into();

    StorageInfo {
        layout: Some(StorageLayout::Chunked),
        endianness,
        codecs,
        fill_value: fill_value_attr(&array.fill_value, dtype),
        order: Some(match array.order {
            ArrayMetadataV2Order::C => 'C',
            ArrayMetadataV2Order::F => 'F',
        }),
        chunk_key_encoding: Some(format!("v2{separator}")),
        ..StorageInfo::default()
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
// Zarr v2, consolidated (`.zmetadata`) — read once, no further listing
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConsolidatedMetadata {
    metadata: BTreeMap<String, Value>,
}

fn summarize_v2_consolidated(
    bytes: &[u8],
    store: &dyn ZarrStore,
    opts: &SummarizeOptions,
) -> Result<GroupSummary, MetaError> {
    let consolidated: ConsolidatedMetadata = parse_json(bytes, store, ".zmetadata")?;
    let location = store.describe(".zmetadata");

    let mut group_paths: HashSet<String> = HashSet::new();
    let mut arrays: HashMap<String, ArrayMetadataV2> = HashMap::new();
    let mut attrs: HashMap<String, serde_json::Map<String, Value>> = HashMap::new();

    for (key, value) in consolidated.metadata {
        if let Some(node_path) = node_path_for(&key, ".zgroup") {
            group_paths.insert(node_path.to_owned());
        } else if let Some(node_path) = node_path_for(&key, ".zarray") {
            let array: ArrayMetadataV2 =
                serde_json::from_value(value).map_err(|source| MetaError::Json {
                    location: location.clone(),
                    source,
                })?;
            arrays.insert(node_path.to_owned(), array);
        } else if let Some(node_path) = node_path_for(&key, ".zattrs")
            && let Some(map) = value.as_object()
        {
            attrs.insert(node_path.to_owned(), map.clone());
        }
        // `.zmetadata` itself (nested, shouldn't occur) and any other key is
        // ignored: only the three per-node file kinds above are meaningful.
    }

    let empty = serde_json::Map::new();

    // A store whose root is a single array, consolidated: one variable.
    if let Some(array) = arrays.get("") {
        let var_attrs = attrs.get("").unwrap_or(&empty);
        let var = v2_var_from_parts(store.root_name(), array, var_attrs, opts)?;
        return Ok(build_group_summary(
            String::new(),
            &empty,
            vec![var],
            Vec::new(),
        ));
    }

    let mut nodes: BTreeMap<String, FlatNode> = BTreeMap::new();
    for (path, array) in &arrays {
        let leaf = path.rsplit('/').next().unwrap_or(path).to_owned();
        let var_attrs = attrs.get(path).unwrap_or(&empty);
        nodes.insert(
            path.clone(),
            FlatNode::Array(Box::new(v2_var_from_parts(leaf, array, var_attrs, opts)?)),
        );
    }
    // Every explicit group, plus any path that only has attributes, is a
    // group node; the root is always one.
    for path in group_paths
        .iter()
        .chain(attrs.keys())
        .chain(std::iter::once(&String::new()))
    {
        if arrays.contains_key(path) {
            continue;
        }
        nodes
            .entry(path.clone())
            .or_insert_with(|| FlatNode::Group {
                attrs: attrs.get(path).cloned().unwrap_or_default(),
            });
    }

    Ok(build_tree_from_flat(&nodes))
}

/// If `key` is `"{node_path}/{doc}"` or just `"{doc}"` (the root), returns
/// `node_path` (`""` for the root).
fn node_path_for<'a>(key: &'a str, doc: &str) -> Option<&'a str> {
    if key == doc {
        Some("")
    } else {
        key.strip_suffix(doc)?.strip_suffix('/')
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::num::NonZeroU64;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::store::MemoryStore;
    use super::*;

    const DETAILS: SummarizeOptions = SummarizeOptions {
        storage_details: true,
    };

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

    fn tiny_v3_array_json(dims: &[&str], extra_codecs: &str) -> String {
        let dim_names: Vec<String> = dims.iter().map(|d| format!("\"{d}\"")).collect();
        format!(
            r#"{{"zarr_format":3,"node_type":"array","shape":[2],"data_type":"float32",
              "chunk_grid":{{"name":"regular","configuration":{{"chunk_shape":[2]}}}},
              "chunk_key_encoding":{{"name":"default","configuration":{{"separator":"/"}}}},
              "fill_value":"NaN",
              "codecs":[{{"name":"bytes","configuration":{{"endian":"big"}}}}{extra_codecs}],
              "attributes":{{"units":"m"}},"dimension_names":[{}]}}"#,
            dim_names.join(",")
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
    fn node_path_for_handles_root_and_nested_keys() {
        assert_eq!(node_path_for(".zgroup", ".zgroup"), Some(""));
        assert_eq!(node_path_for("g/arr/.zarray", ".zarray"), Some("g/arr"));
        assert_eq!(node_path_for("g/.zattrs", ".zarray"), None);
        assert_eq!(node_path_for("x.zarray", ".zarray"), None);
    }

    #[test]
    fn fill_values_follow_the_dtype() {
        match fill_value_attr(&FillValueMetadataV3::String("NaN".into()), "float32") {
            Some(AttrValue::Float32(f)) => assert!(f.is_nan()),
            other => panic!("expected Float32(NaN), got {other:?}"),
        }
        match fill_value_attr(&FillValueMetadataV3::String("-Infinity".into()), "float64") {
            Some(AttrValue::Float(f)) => assert_eq!(f, f64::NEG_INFINITY),
            other => panic!("expected Float(-inf), got {other:?}"),
        }
        assert_eq!(
            fill_value_attr(&FillValueMetadataV3::Number(0.into()), "int16"),
            Some(AttrValue::Int(0))
        );
        assert_eq!(
            fill_value_attr(&FillValueMetadataV3::Number(0.into()), "float64"),
            Some(AttrValue::Float(0.0))
        );
        assert_eq!(fill_value_attr(&FillValueMetadataV3::Null, "int8"), None);
        assert_eq!(
            fill_value_attr(&FillValueMetadataV3::String("0x00".into()), "int8"),
            Some(AttrValue::Text("0x00".into()))
        );
    }

    #[test]
    fn base64_fill_value_keeps_float32_width() {
        // f32 NaN little-endian bytes.
        assert_eq!(
            decode_base64_float_attr("_FillValue", &Value::String("AADAfw==".into()))
                .map(|v| matches!(v, AttrValue::Float32(f) if f.is_nan())),
            Some(true)
        );
        assert_eq!(
            decode_base64_float_attr("_FillValue", &Value::String("AAAAAAAA+H8=".into()))
                .map(|v| matches!(v, AttrValue::Float(f) if f.is_nan())),
            Some(true)
        );
        assert_eq!(
            decode_base64_float_attr("other", &Value::String("AADAfw==".into())),
            None
        );
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
        fs::write(dir.join("zarr.json"), tiny_v3_array_json(&["x"], "")).expect("write zarr.json");

        let summary = summarize_zarr(&dir).expect("summarize root-is-array v3 store");
        let names: Vec<&str> = summary
            .root
            .coords
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        // `mydata` is not one of its own dims, so it lands in data_vars.
        assert!(names.is_empty());
        assert_eq!(summary.root.data_vars[0].name, "mydata");
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

    /// A store nested deeper than the cap (here: a chain of groups in an
    /// in-memory store, which has no symlinks to detect) stops with an
    /// error instead of recursing without bound.
    #[test]
    fn walkers_cap_nesting_depth() {
        let group = r#"{"zarr_format":3,"node_type":"group","attributes":{}}"#;
        let mut store = MemoryStore::new("mem://deep.zarr");
        let mut path = String::new();
        store.insert("zarr.json", group);
        for i in 0..=MAX_GROUP_DEPTH {
            path = join_key(&path, &format!("g{i}"));
            store.insert(&join_key(&path, "zarr.json"), group);
        }
        let err =
            summarize_zarr_store(&store, &SummarizeOptions::default()).expect_err("past the cap");
        assert!(
            err.to_string().contains("deeper than"),
            "unexpected error: {err}"
        );
    }

    /// Regression test for a consolidated v2 store where an intermediate
    /// group (`g`) has no `.zgroup`/`.zattrs` entry of its own in
    /// `.zmetadata` — only its array `g/arr` does. `g` must still be
    /// discovered as an implicit child group of the root, with `arr` inside
    /// it, rather than being silently dropped.
    #[test]
    fn consolidated_v2_discovers_implicit_child_group() {
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

        // No listing at all: consolidated metadata must be enough.
        let mut store = MemoryStore::new("mem://implicit.zarr");
        store.can_list = false;
        store.insert(".zmetadata", serde_json::to_string(&doc).unwrap());

        let summary = summarize_zarr_store(&store, &SummarizeOptions::default())
            .expect("summarize consolidated v2 store");
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
    }

    /// zarr-python 3's inline consolidated metadata makes a v3 store readable
    /// without any listing — the case that matters over plain HTTP.
    #[test]
    fn consolidated_v3_needs_no_listing() {
        let arr = tiny_v3_array_json(&["x"], r#",{"name":"zstd","configuration":{"level":3}}"#);
        let arr_value: Value = serde_json::from_str(&arr).unwrap();
        let root = serde_json::json!({
            "zarr_format": 3, "node_type": "group",
            "attributes": {"title": "consolidated"},
            "consolidated_metadata": {
                "kind": "inline", "must_understand": false,
                "metadata": {
                    "x": arr_value,
                    "g": {"zarr_format": 3, "node_type": "group", "attributes": {"note": "sub"}, "consolidated_metadata": null},
                    "g/y": arr_value,
                }
            }
        });
        let mut store = MemoryStore::new("https://example.org/data/store.zarr");
        store.can_list = false;
        store.insert("zarr.json", serde_json::to_string(&root).unwrap());

        let summary = summarize_zarr_store(&store, &DETAILS).expect("summarize consolidated v3");
        assert_eq!(summary.format, SourceFormat::ZarrV3);
        assert_eq!(
            summary.file_info.as_ref().map(|f| f.kind.as_str()),
            Some("Zarr v3")
        );
        assert_eq!(summary.root.attrs.len(), 1);
        assert_eq!(summary.root.coords[0].name, "x");
        let g = &summary.root.children[0];
        assert_eq!(g.name, "g");
        assert_eq!(g.attrs.len(), 1);
        assert_eq!(g.data_vars[0].name, "y");

        let storage = summary.root.coords[0]
            .storage
            .as_ref()
            .expect("storage details");
        assert_eq!(storage.endianness, Some(Endianness::Big));
        assert_eq!(storage.chunk_key_encoding.as_deref(), Some("default/"));
        let codec_names: Vec<&str> = storage.codecs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(codec_names, vec!["bytes", "zstd"]);
        assert!(matches!(storage.fill_value, Some(AttrValue::Float32(f)) if f.is_nan()));
    }

    #[test]
    fn unconsolidated_store_without_listing_fails_clearly() {
        let mut store = MemoryStore::new("https://example.org/plain.zarr");
        store.can_list = false;
        store.insert(
            "zarr.json",
            r#"{"zarr_format":3,"node_type":"group","attributes":{}}"#,
        );
        let err = summarize_zarr_store(&store, &SummarizeOptions::default())
            .expect_err("walking needs listing");
        assert!(matches!(err, MetaError::ListingUnsupported { .. }), "{err}");
    }

    #[test]
    fn v2_storage_details_record_codecs_and_order() {
        let mut store = MemoryStore::new("mem://v2.zarr");
        store.insert(".zgroup", r#"{"zarr_format":2}"#);
        store.insert(
            "a/.zarray",
            r#"{"zarr_format":2,"shape":[4],"chunks":[2],"dtype":">i2","fill_value":-1,
                "order":"F","dimension_separator":".",
                "filters":[{"id":"delta","dtype":"<i2"}],
                "compressor":{"id":"blosc","cname":"lz4","clevel":5,"shuffle":1,"blocksize":0}}"#,
        );
        store.insert("a/.zattrs", r#"{"_ARRAY_DIMENSIONS":["a"]}"#);

        let summary = summarize_zarr_store(&store, &DETAILS).expect("summarize v2");
        let a = summary.root.variable("a").expect("a exists");
        assert_eq!(a.dtype, "int16");
        let storage = a.storage.as_ref().expect("storage");
        assert_eq!(storage.endianness, Some(Endianness::Big));
        assert_eq!(storage.order, Some('F'));
        assert_eq!(storage.chunk_key_encoding.as_deref(), Some("v2."));
        assert_eq!(storage.fill_value, Some(AttrValue::Int(-1)));
        let names: Vec<&str> = storage.codecs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["delta", "blosc"]);
        assert_eq!(
            storage.codecs[1]
                .configuration
                .as_ref()
                .and_then(|c| c.get("cname")),
            Some(&Value::String("lz4".into()))
        );
    }
}
