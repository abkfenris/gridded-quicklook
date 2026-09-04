//! NetCDF/HDF5 metadata reader.
//!
//! Opens a file through `libnetcdf` (which transparently handles both
//! classic netCDF and netCDF-4/HDF5 files) and walks its group tree into a
//! format-agnostic [`DatasetSummary`]. Only metadata and small coordinate
//! previews are read; bulk variable data is never touched.

use std::collections::HashSet;
use std::ffi::{c_int, CString};
use std::path::Path;

// `::netcdf` (leading `::`) disambiguates the external `netcdf` crate from
// this module, which is itself named `netcdf` (`crate::netcdf`).
use ::netcdf::types::{FloatType, IntType, NcVariableType};
use ::netcdf::{Attribute, AttributeValue, Dimension, Group, Variable};

use crate::error::MetaError;
use crate::model::{AttrValue, DatasetSummary, DimInfo, GroupSummary, SourceFormat, VarSummary};

/// Summarize the structure of a NetCDF or HDF5 file at `path`.
///
/// Plain HDF5 files (h5py output, say) open through `libnetcdf` just like
/// netCDF-4 ones; they are told apart by [`detect_format`] and reported as
/// [`SourceFormat::Hdf5`] so the format badge is honest about what wrote
/// the file.
pub fn summarize_netcdf(path: &Path) -> Result<DatasetSummary, MetaError> {
    let file = ::netcdf::open(path).map_err(|source| MetaError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let format = detect_format(path);

    let root = match file.root() {
        // netCDF-4 (and netCDF-4 classic) files expose a proper group tree.
        Some(group) => summarize_group(&group, true),
        // Classic/64-bit-offset/CDF5 files have no group API; fall back to
        // the flat file-level variable/attribute accessors for a single,
        // childless root group.
        None => {
            let dims: Vec<DimInfo> = file.dimensions().map(|d| dim_info(&d)).collect();
            let vars: Vec<Variable> = file.variables().collect();
            let attrs = file.attributes().map(|a| attr_entry(&a)).collect();
            GroupSummary::from_parts(
                String::new(),
                Some(dims),
                attrs,
                collect_var_summaries(&vars),
                Vec::new(),
            )
        }
    };

    Ok(DatasetSummary {
        format,
        root,
        version_info: None,
    })
}

/// Was the file at `path` written by the netCDF library, or is it HDF5 that
/// libnetcdf merely knows how to open?
///
/// libnetcdf answers this itself through the virtual global attribute
/// `_IsNetcdf4` (see `nc_provenance.h`): it is constructed on the fly for
/// HDF5-backed files, `1` when the file carries netCDF-4's provenance
/// markers (`_NCProperties`, dimension scales) and `0` otherwise, and does
/// not exist at all for classic-format files. It is what `ncdump -s` prints.
///
/// The probe goes through `netcdf-sys` on a short-lived handle of its own
/// rather than the already-open [`::netcdf::File`]: the safe crate keeps
/// its `ncid` private, and its by-name attribute lookup goes via
/// `nc_inq_attid`, which libnetcdf refuses for virtual attributes
/// (`NC_EATTMETA`) and which the crate then `unwrap`s into a panic. Any
/// failure along the way reads as "netCDF", the status quo before this
/// probe existed.
fn detect_format(path: &Path) -> SourceFormat {
    let Some(c_path) = path.to_str().and_then(|s| CString::new(s).ok()) else {
        return SourceFormat::NetCdf;
    };

    // libnetcdf is not thread-safe; the `netcdf` crate serializes every
    // call through this same lock, so taking it here keeps the raw calls
    // below from interleaving with the crate's.
    let _guard = netcdf_sys::libnetcdf_lock.lock();
    let mut ncid: c_int = 0;
    // SAFETY: `c_path` is a valid NUL-terminated string and `ncid` a valid
    // out-pointer; the handle is closed below on every path after a
    // successful open.
    if unsafe { netcdf_sys::nc_open(c_path.as_ptr(), netcdf_sys::NC_NOWRITE, &mut ncid) }
        != netcdf_sys::NC_NOERR
    {
        return SourceFormat::NetCdf;
    }
    let mut is_netcdf4: c_int = 1;
    // SAFETY: `ncid` was just returned by a successful `nc_open`, the name
    // is a NUL-terminated literal and `is_netcdf4` a valid out-pointer.
    let status = unsafe {
        netcdf_sys::nc_get_att_int(
            ncid,
            netcdf_sys::NC_GLOBAL,
            c"_IsNetcdf4".as_ptr(),
            &mut is_netcdf4,
        )
    };
    // SAFETY: closing the handle opened above, exactly once.
    unsafe { netcdf_sys::nc_close(ncid) };

    if status == netcdf_sys::NC_NOERR && is_netcdf4 == 0 {
        SourceFormat::Hdf5
    } else {
        SourceFormat::NetCdf
    }
}

fn summarize_group(group: &Group, is_root: bool) -> GroupSummary {
    let dims: Vec<DimInfo> = group.dimensions().map(|d| dim_info(&d)).collect();
    let vars: Vec<Variable> = group.variables().collect();
    let attrs = group.attributes().map(|a| attr_entry(&a)).collect();
    let children = group.groups().map(|g| summarize_group(&g, false)).collect();

    let name = if is_root { String::new() } else { group.name() };
    GroupSummary::from_parts(
        name,
        Some(dims),
        attrs,
        collect_var_summaries(&vars),
        children,
    )
}

/// Builds each variable's [`VarSummary`] in a scope (group or, for
/// classic-format files with no group API, the whole file), deciding
/// up front — via xarray's coordinate heuristic — whether each variable is a
/// coordinate, so that only coordinates pay for [`preview_values`]'s data
/// read. The actual coords/data_vars split surfaced in the final
/// [`GroupSummary`] is (re)computed identically by
/// [`GroupSummary::from_parts`]; this earlier pass exists solely to gate
/// that expensive read, not to classify for display purposes.
fn collect_var_summaries(variables: &[Variable]) -> Vec<VarSummary> {
    let mut coord_names: HashSet<String> = HashSet::new();
    for var in variables {
        if let Some(Ok(AttributeValue::Str(names))) = var.attribute_value("coordinates") {
            coord_names.extend(names.split_whitespace().map(str::to_owned));
        }
    }

    variables
        .iter()
        .map(|var| {
            let name = var.name();
            let is_dim_coord = var.dimensions().iter().any(|d| d.name() == name);
            var_summary(var, is_dim_coord || coord_names.contains(&name))
        })
        .collect()
}

fn dim_info(dim: &Dimension) -> DimInfo {
    DimInfo {
        name: dim.name(),
        size: dim.len() as u64,
        is_unlimited: dim.is_unlimited(),
    }
}

/// Reads one attribute's name and value.
///
/// `attr.value()` can fail even for a well-formed file: netCDF reports
/// `Error::TypeUnknown` for attribute types this crate/libnetcdf can't
/// decode (enum, compound, vlen, opaque — e.g. an HDF5 `bool` attribute
/// written by h5py). Such an error shouldn't take down the whole summary,
/// so it's mapped to a placeholder text value rather than propagated.
fn attr_entry(attr: &Attribute) -> (String, AttrValue) {
    let name = attr.name().to_owned();
    let value = match attr.value() {
        Ok(value) => attr_value_from(value),
        Err(_) => AttrValue::Text("<unsupported attribute type>".to_owned()),
    };
    (name, value)
}

/// Converts a raw netCDF attribute value into the format-agnostic
/// [`AttrValue`]. Exotic/rare numeric types (bytes, shorts, unsigned
/// variants, ...) are widened into `Int`/`Float`/`IntList`/`FloatList`;
/// this loses no information for the value ranges those types can hold,
/// except `u64`/`u64[]` (`NC_UINT64`), which can exceed `i64::MAX` (e.g.
/// `NC_FILL_UINT64`); those fall back to exact decimal text via
/// [`ulonglong_to_attr_value`]/[`ulonglongs_to_attr_value`].
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
        AttributeValue::Ulonglong(v) => ulonglong_to_attr_value(v),
        AttributeValue::Ulonglongs(v) => ulonglongs_to_attr_value(v),
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

/// Converts a single `NC_UINT64` attribute value, preserving values above
/// `i64::MAX` (e.g. `NC_FILL_UINT64 = 18446744073709551614`) as exact
/// decimal text instead of silently wrapping into a negative `i64`.
fn ulonglong_to_attr_value(v: u64) -> AttrValue {
    match i64::try_from(v) {
        Ok(v) => AttrValue::Int(v),
        Err(_) => AttrValue::Text(v.to_string()),
    }
}

/// Converts an `NC_UINT64` array attribute value. If every element fits in
/// `i64`, the whole list is kept numeric (`IntList`); otherwise the whole
/// list is rendered as exact decimal text (`TextList`) so no single element
/// silently wraps.
fn ulonglongs_to_attr_value(v: Vec<u64>) -> AttrValue {
    match v
        .iter()
        .map(|&x| i64::try_from(x))
        .collect::<Result<_, _>>()
    {
        Ok(ints) => AttrValue::IntList(ints),
        Err(_) => AttrValue::TextList(v.into_iter().map(|x| x.to_string()).collect()),
    }
}

fn var_summary(var: &Variable, is_coord: bool) -> VarSummary {
    let name = var.name();
    let dims: Vec<Dimension> = var.dimensions().to_vec();
    let dim_names = dims.iter().map(Dimension::name).collect();
    let shape: Vec<u64> = dims.iter().map(|d| d.len() as u64).collect();
    let dtype = dtype_string(&var.vartype(), &dims);
    // Defensive: some backends/variable storage layouts (e.g. contiguous
    // classic-format variables) can error here rather than simply reporting
    // "not chunked"; either way there's no chunking info to show.
    let chunks = var.chunking().ok().flatten();
    let chunks = chunks.map(|c| c.into_iter().map(|x| x as u64).collect());
    let attrs = var.attributes().map(|a| attr_entry(&a)).collect();
    let preview = if is_coord { preview_values(var) } else { None };

    VarSummary {
        name,
        dtype,
        dims: dim_names,
        shape,
        chunks,
        attrs,
        preview,
    }
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
///
/// This is the only place the reader touches variable *data*, and it is
/// purely cosmetic, so a failed read must not take down the whole summary:
/// the values are simply not previewed. The realistic failure is an HDF5
/// filter plugin the statically linked libnetcdf doesn't ship (zstd, blosc,
/// ...) on a compressed coordinate, which would otherwise turn a perfectly
/// readable file's preview into an error card.
fn preview_values(var: &Variable) -> Option<String> {
    let dims = var.dimensions();
    if dims.len() != 1 {
        return None;
    }
    let len = dims[0].len();
    if len == 0 || len > 64 {
        return None;
    }

    let is_float = match var.vartype() {
        NcVariableType::Float(_) => true,
        NcVariableType::Int(_) => false,
        // Text and user-defined types aren't previewed here.
        _ => return None,
    };

    let values: Vec<f64> = var.get_values(..).ok()?;

    Some(format_preview(&values, is_float))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulonglong_below_i64_max_stays_int() {
        assert_eq!(ulonglong_to_attr_value(42), AttrValue::Int(42));
        assert_eq!(
            ulonglong_to_attr_value(i64::MAX as u64),
            AttrValue::Int(i64::MAX)
        );
    }

    #[test]
    fn ulonglong_above_i64_max_becomes_exact_text() {
        // NC_FILL_UINT64, the classic netCDF fill value for NC_UINT64: as
        // i64 this wraps to -2, which is what the bug used to render.
        let fill = 18_446_744_073_709_551_614_u64;
        assert_eq!(
            ulonglong_to_attr_value(fill),
            AttrValue::Text("18446744073709551614".to_owned())
        );
        assert_eq!(
            ulonglong_to_attr_value(i64::MAX as u64 + 1),
            AttrValue::Text((i64::MAX as u64 + 1).to_string())
        );
    }

    #[test]
    fn ulonglongs_all_fit_stays_int_list() {
        assert_eq!(
            ulonglongs_to_attr_value(vec![0, 1, i64::MAX as u64]),
            AttrValue::IntList(vec![0, 1, i64::MAX])
        );
    }

    #[test]
    fn ulonglongs_any_overflow_becomes_text_list() {
        let fill = 18_446_744_073_709_551_614_u64;
        assert_eq!(
            ulonglongs_to_attr_value(vec![0, fill, 2]),
            AttrValue::TextList(vec![
                "0".to_owned(),
                "18446744073709551614".to_owned(),
                "2".to_owned(),
            ])
        );
    }
}
