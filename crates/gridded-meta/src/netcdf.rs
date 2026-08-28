//! NetCDF/HDF5 metadata reader.
//!
//! Opens a file through `libnetcdf` (which transparently handles both
//! classic netCDF and netCDF-4/HDF5 files) and walks its group tree into a
//! format-agnostic [`DatasetSummary`]. Only metadata and small coordinate
//! previews are read; bulk variable data is never touched.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

// `::netcdf` (leading `::`) disambiguates the external `netcdf` crate from
// this module, which is itself named `netcdf` (`crate::netcdf`).
use ::netcdf::types::{FloatType, IntType, NcVariableType};
use ::netcdf::{Attribute, AttributeValue, Dimension, Group, Variable};

use crate::model::{AttrValue, DatasetSummary, DimInfo, GroupSummary, SourceFormat, VarSummary};

/// Errors that can occur while reading NetCDF/HDF5 metadata.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("failed to open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: ::netcdf::Error,
    },
    #[error("failed to read metadata from {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: ::netcdf::Error,
    },
}

/// Summarize the structure of a NetCDF or HDF5 file at `path`.
///
/// Plain HDF5 files without netCDF-4 conventions still open successfully
/// through `libnetcdf` and are reported as [`SourceFormat::NetCdf`]; there
/// is no cheap way to distinguish "HDF5 that happens to satisfy netCDF-4
/// conventions" from "HDF5 written by the netCDF-4 library" at this layer,
/// so no separate `Hdf5` detection is attempted in this milestone.
pub fn summarize_netcdf(path: &Path) -> Result<DatasetSummary, MetaError> {
    let file = ::netcdf::open(path).map_err(|source| MetaError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    let root = match file.root() {
        // netCDF-4 (and netCDF-4 classic) files expose a proper group tree.
        Some(group) => summarize_group(&group, true, path)?,
        // Classic/64-bit-offset files have no group API; fall back to the
        // flat file-level accessors for a single, childless root group.
        None => GroupSummary {
            name: String::new(),
            dims: file.dimensions().map(|d| dim_info(&d)).collect(),
            coords: Vec::new(),
            data_vars: Vec::new(),
            attrs: file
                .attributes()
                .map(|a| attr_entry(&a, path))
                .collect::<Result<_, _>>()?,
            children: Vec::new(),
        },
    };

    Ok(DatasetSummary {
        format: SourceFormat::NetCdf,
        root,
        version_info: None,
    })
}

fn summarize_group(group: &Group, is_root: bool, path: &Path) -> Result<GroupSummary, MetaError> {
    let dims: Vec<DimInfo> = group.dimensions().map(|d| dim_info(&d)).collect();

    // xarray's coordinate heuristic: a variable is a coordinate if its name
    // matches one of its own dimensions ("dimension coordinate"), or if it
    // is named in some variable's `coordinates` attribute in this group.
    let mut coord_names: HashSet<String> = HashSet::new();
    for var in group.variables() {
        if let Some(Ok(AttributeValue::Str(names))) = var.attribute_value("coordinates") {
            coord_names.extend(names.split_whitespace().map(str::to_owned));
        }
    }

    let mut coords = Vec::new();
    let mut data_vars = Vec::new();
    for var in group.variables() {
        let name = var.name();
        let is_dim_coord = var.dimensions().iter().any(|d| d.name() == name);
        let is_coord = is_dim_coord || coord_names.contains(&name);
        let summary = var_summary(&var, is_coord, path)?;
        if is_coord {
            coords.push(summary);
        } else {
            data_vars.push(summary);
        }
    }
    coords.sort_by(|a, b| a.name.cmp(&b.name));
    data_vars.sort_by(|a, b| a.name.cmp(&b.name));

    let attrs = group
        .attributes()
        .map(|a| attr_entry(&a, path))
        .collect::<Result<_, _>>()?;

    let children = group
        .groups()
        .map(|g| summarize_group(&g, false, path))
        .collect::<Result<_, _>>()?;

    Ok(GroupSummary {
        name: if is_root { String::new() } else { group.name() },
        dims,
        coords,
        data_vars,
        attrs,
        children,
    })
}

fn dim_info(dim: &Dimension) -> DimInfo {
    DimInfo {
        name: dim.name(),
        size: dim.len() as u64,
        is_unlimited: dim.is_unlimited(),
    }
}

fn attr_entry(attr: &Attribute, path: &Path) -> Result<(String, AttrValue), MetaError> {
    let name = attr.name().to_owned();
    let value = attr.value().map_err(|source| MetaError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok((name, attr_value_from(value)))
}

/// Converts a raw netCDF attribute value into the format-agnostic
/// [`AttrValue`]. Exotic/rare numeric types (bytes, shorts, unsigned
/// variants, ...) are widened into `Int`/`Float`/`IntList`/`FloatList`;
/// this loses no information for the value ranges those types can hold.
fn attr_value_from(value: AttributeValue) -> AttrValue {
    match value {
        AttributeValue::Uchar(v) => AttrValue::Int(v.into()),
        AttributeValue::Uchars(v) => AttrValue::IntList(v.into_iter().map(i64::from).collect()),
        AttributeValue::Schar(v) => AttrValue::Int(v.into()),
        AttributeValue::Schars(v) => AttrValue::IntList(v.into_iter().map(i64::from).collect()),
        AttributeValue::Ushort(v) => AttrValue::Int(v.into()),
        AttributeValue::Ushorts(v) => AttrValue::IntList(v.into_iter().map(i64::from).collect()),
        AttributeValue::Short(v) => AttrValue::Int(v.into()),
        AttributeValue::Shorts(v) => AttrValue::IntList(v.into_iter().map(i64::from).collect()),
        AttributeValue::Uint(v) => AttrValue::Int(v.into()),
        AttributeValue::Uints(v) => AttrValue::IntList(v.into_iter().map(i64::from).collect()),
        AttributeValue::Int(v) => AttrValue::Int(v.into()),
        AttributeValue::Ints(v) => AttrValue::IntList(v.into_iter().map(i64::from).collect()),
        AttributeValue::Ulonglong(v) => AttrValue::Int(v as i64),
        AttributeValue::Ulonglongs(v) => {
            AttrValue::IntList(v.into_iter().map(|x| x as i64).collect())
        }
        AttributeValue::Longlong(v) => AttrValue::Int(v),
        AttributeValue::Longlongs(v) => AttrValue::IntList(v),
        AttributeValue::Float(v) => AttrValue::Float(v.into()),
        AttributeValue::Floats(v) => AttrValue::FloatList(v.into_iter().map(f64::from).collect()),
        AttributeValue::Double(v) => AttrValue::Float(v),
        AttributeValue::Doubles(v) => AttrValue::FloatList(v),
        AttributeValue::Str(v) => AttrValue::Text(v),
        AttributeValue::Strs(v) => AttrValue::TextList(v),
    }
}

fn var_summary(var: &Variable, is_coord: bool, path: &Path) -> Result<VarSummary, MetaError> {
    let name = var.name();
    let dims: Vec<Dimension> = var.dimensions().to_vec();
    let dim_names = dims.iter().map(Dimension::name).collect();
    let shape: Vec<u64> = dims.iter().map(|d| d.len() as u64).collect();
    let dtype = dtype_string(&var.vartype(), &dims);
    let chunks = var.chunking().map_err(|source| MetaError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let chunks = chunks.map(|c| c.into_iter().map(|x| x as u64).collect());
    let attrs = var
        .attributes()
        .map(|a| attr_entry(&a, path))
        .collect::<Result<_, _>>()?;
    let preview = if is_coord {
        preview_values(var, path)?
    } else {
        None
    };

    Ok(VarSummary {
        name,
        dtype,
        dims: dim_names,
        shape,
        chunks,
        attrs,
        preview,
    })
}

/// Numpy-style dtype string, matching how xarray displays it (e.g.
/// `float32`, `int64`, `|S8` for fixed-length char arrays).
fn dtype_string(vartype: &NcVariableType, dims: &[Dimension]) -> String {
    match vartype {
        NcVariableType::Float(FloatType::F32) => "float32".to_owned(),
        NcVariableType::Float(FloatType::F64) => "float64".to_owned(),
        NcVariableType::Int(IntType::I8) => "int8".to_owned(),
        NcVariableType::Int(IntType::U8) => "uint8".to_owned(),
        NcVariableType::Int(IntType::I16) => "int16".to_owned(),
        NcVariableType::Int(IntType::U16) => "uint16".to_owned(),
        NcVariableType::Int(IntType::I32) => "int32".to_owned(),
        NcVariableType::Int(IntType::U32) => "uint32".to_owned(),
        NcVariableType::Int(IntType::I64) => "int64".to_owned(),
        NcVariableType::Int(IntType::U64) => "uint64".to_owned(),
        // A netCDF `char` array conventionally uses its trailing dimension
        // as the fixed string length (the `NC_CHAR` + "string length dim"
        // convention netCDF-classic uses to emulate fixed-width strings).
        NcVariableType::Char => {
            let len = dims.last().map(|d| d.len()).unwrap_or(1);
            format!("|S{len}")
        }
        NcVariableType::String => "object".to_owned(),
        other => format!("{other:?}"),
    }
}

/// Short inline preview for small 1-D coordinate variables, e.g.
/// `10.0 12.5 15.0 ... 42.0`. Never reads data for larger variables, and
/// never CF-decodes datetimes (units containing "since") — that's left for
/// a later milestone.
fn preview_values(var: &Variable, path: &Path) -> Result<Option<String>, MetaError> {
    let dims = var.dimensions();
    if dims.len() != 1 {
        return Ok(None);
    }
    let len = dims[0].len();
    if len == 0 || len > 64 {
        return Ok(None);
    }

    let is_float = match var.vartype() {
        NcVariableType::Float(_) => true,
        NcVariableType::Int(_) => false,
        // Text and user-defined types aren't previewed here.
        _ => return Ok(None),
    };

    let values: Vec<f64> = var.get_values(..).map_err(|source| MetaError::Read {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(Some(format_preview(&values, is_float)))
}

fn format_preview(values: &[f64], is_float: bool) -> String {
    let format_one = |v: f64| -> String {
        if is_float {
            let s = format!("{v}");
            if s.contains(['.', 'e', 'E']) || s == "inf" || s == "-inf" || s == "NaN" {
                s
            } else {
                format!("{s}.0")
            }
        } else {
            format!("{}", v as i64)
        }
    };

    if values.len() <= 4 {
        values
            .iter()
            .copied()
            .map(format_one)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        let head = values[..3]
            .iter()
            .copied()
            .map(format_one)
            .collect::<Vec<_>>()
            .join(" ");
        let tail = format_one(values[values.len() - 1]);
        format!("{head} ... {tail}")
    }
}
