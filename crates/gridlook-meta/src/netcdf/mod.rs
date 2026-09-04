//! NetCDF/HDF5 metadata reader.
//!
//! Opens a file through `libnetcdf` (which transparently handles both
//! classic netCDF and netCDF-4/HDF5 files) and walks its group tree into a
//! format-agnostic [`DatasetSummary`]. Only metadata and small coordinate
//! previews are read; bulk variable data is never touched.

mod raw;

use std::collections::HashSet;
use std::ffi::{CString, c_int};
use std::path::Path;

// `::netcdf` (leading `::`) disambiguates the external `netcdf` crate from
// this module, which is itself named `netcdf` (`crate::netcdf`).
use ::netcdf::types::{FloatType, IntType, NcVariableType};
use ::netcdf::{Attribute, AttributeValue, Dimension, Group, Variable};

use crate::error::MetaError;
use crate::model::{
    AttrValue, DatasetSummary, DimInfo, GroupSummary, SourceFormat, SummarizeOptions, VarSummary,
};

use self::raw::RawFile;

/// Summarize the structure of a NetCDF or HDF5 file at `path`.
///
/// Plain HDF5 files (h5py output, say) open through `libnetcdf` just like
/// netCDF-4 ones; they are told apart by [`detect_format`] and reported as
/// [`SourceFormat::Hdf5`] so the format badge is honest about what wrote
/// the file.
pub fn summarize_netcdf(path: &Path) -> Result<DatasetSummary, MetaError> {
    summarize_netcdf_with(path, &SummarizeOptions::default())
}

/// [`summarize_netcdf`] with control over how much detail is gathered.
///
/// With [`SummarizeOptions::storage_details`] set, the file is re-opened
/// through the raw `libnetcdf` API after the structural walk (see
/// [`raw`]) to fill in each variable's [`VarSummary::storage`] and the
/// summary's [`DatasetSummary::file_info`].
pub fn summarize_netcdf_with(
    path: &Path,
    opts: &SummarizeOptions,
) -> Result<DatasetSummary, MetaError> {
    // Hold libnetcdf's (reentrant) lock for the whole summary, not just per
    // call. HDF5 keeps a process-global cache of datatype conversion paths;
    // a path built while converting a variable-length (`string`) fill value
    // remembers the file handle it was built against, and reusing it after
    // that handle was closed by *another thread's* summary of the same file
    // dereferences the freed handle (a segfault deep in H5T__conv_vlen).
    // Serializing whole summaries means no other thread opens or closes a
    // handle while this one is in flight.
    let _serialized = netcdf_sys::libnetcdf_lock.lock();

    let file = ::netcdf::open(path).map_err(|source| MetaError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let format = detect_format(path);

    let mut root = match file.root() {
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

    // The structural walk is done with `file`; close it before the raw
    // handle opens the same file so at most one handle is live at a time.
    drop(file);

    let file_info = if opts.storage_details {
        let raw = RawFile::open(path)?;
        attach_storage(&mut root, &raw, "")?;
        Some(raw.file_info()?)
    } else {
        None
    };

    Ok(DatasetSummary {
        format,
        root,
        version_info: None,
        file_info,
    })
}

/// Fills in [`VarSummary::storage`] for every variable in `group` and its
/// descendants. `group_path` is the group's full netCDF path (`""` for the
/// root, `"/group_a/nested"` below it).
fn attach_storage(
    group: &mut GroupSummary,
    raw: &RawFile,
    group_path: &str,
) -> Result<(), MetaError> {
    let grp = raw.group_ncid(group_path)?;
    for var in group.coords.iter_mut().chain(group.data_vars.iter_mut()) {
        var.storage = Some(raw.var_storage(grp, &var.name)?);
    }
    for child in &mut group.children {
        let child_path = format!("{group_path}/{}", child.name);
        attach_storage(child, raw, &child_path)?;
    }
    Ok(())
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
/// [`AttrValue`], one variant per netCDF atomic type so the exact type
/// (width, signedness, float precision) survives for renderers that print
/// typed literals (CDL's `1.5f`, `2s`, `0b`, `18446744073709551614ULL`).
fn attr_value_from(value: AttributeValue) -> AttrValue {
    match value {
        AttributeValue::Uchar(v) => AttrValue::UInt8(v),
        AttributeValue::Uchars(v) => AttrValue::UInt8List(v),
        AttributeValue::Schar(v) => AttrValue::Int8(v),
        AttributeValue::Schars(v) => AttrValue::Int8List(v),
        AttributeValue::Ushort(v) => AttrValue::UInt16(v),
        AttributeValue::Ushorts(v) => AttrValue::UInt16List(v),
        AttributeValue::Short(v) => AttrValue::Int16(v),
        AttributeValue::Shorts(v) => AttrValue::Int16List(v),
        AttributeValue::Uint(v) => AttrValue::UInt32(v),
        AttributeValue::Uints(v) => AttrValue::UInt32List(v),
        AttributeValue::Int(v) => AttrValue::Int32(v),
        AttributeValue::Ints(v) => AttrValue::Int32List(v),
        AttributeValue::Ulonglong(v) => AttrValue::UInt64(v),
        AttributeValue::Ulonglongs(v) => AttrValue::UInt64List(v),
        AttributeValue::Longlong(v) => AttrValue::Int(v),
        AttributeValue::Longlongs(v) => AttrValue::IntList(v),
        AttributeValue::Float(v) => AttrValue::Float32(v),
        AttributeValue::Floats(v) => AttrValue::Float32List(v),
        AttributeValue::Double(v) => AttrValue::Float(v),
        AttributeValue::Doubles(v) => AttrValue::FloatList(v),
        AttributeValue::Str(v) => AttrValue::Text(v),
        AttributeValue::Strs(v) => AttrValue::TextList(v),
    }
}

fn var_summary(var: &Variable, is_coord: bool) -> VarSummary {
    let name = var.name();
    let dims: Vec<Dimension> = var.dimensions().to_vec();
    let dim_names = dims.iter().map(Dimension::name).collect();
    let shape: Vec<u64> = dims.iter().map(|d| d.len() as u64).collect();
    let dtype = dtype_string(&var.vartype());
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
        storage: None,
    }
}

/// Numpy-style dtype string for a variable as stored (e.g. `float32`,
/// `int64`, `|S1`), matching `ncdump`/`h5py` and xarray with
/// `decode_cf=False`.
fn dtype_string(vartype: &NcVariableType) -> String {
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
        // `NC_CHAR` is a one-byte character type; a netCDF `char` array
        // conventionally uses its trailing dimension as a fixed string
        // length, which xarray's CF decoding collapses into `|S{len}` while
        // *dropping that dimension*. This reader reports dims/shape exactly
        // as stored, so the dtype has to match that view: `|S1` over every
        // dimension including the string-length one. Reporting `|S{len}`
        // alongside the still-present trailing dim described neither the
        // raw variable nor xarray's decoded one.
        NcVariableType::Char => "|S1".to_owned(),
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

    /// Writes a tiny netCDF-4 file with a classic fixed-width string variable
    /// (`NC_CHAR` over `(station, string8)`) and checks that the summary
    /// describes it as stored: `|S1` over both dimensions, rather than the
    /// old `|S8` that claimed CF-decoded width while still listing the
    /// string-length dimension.
    #[test]
    fn char_variable_is_reported_as_stored() {
        let dir = std::env::temp_dir().join(format!(
            "gridlook-meta-netcdf-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("chars.nc");
        {
            let mut file = ::netcdf::create(&path).expect("create netCDF file");
            file.add_dimension("station", 2).expect("add station dim");
            file.add_dimension("string8", 8).expect("add string8 dim");
            file.add_variable_with_type(
                "station_name",
                &["station", "string8"],
                &NcVariableType::Char,
            )
            .expect("add char variable");
        }

        let summary = summarize_netcdf(&path).expect("summarize");
        let var = summary
            .root
            .data_vars
            .iter()
            .find(|v| v.name == "station_name")
            .expect("station_name is a data variable");
        assert_eq!(var.dtype, "|S1");
        assert_eq!(var.dims, vec!["station", "string8"]);
        assert_eq!(var.shape, vec![2, 8]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `NC_FILL_UINT64` exceeds `i64::MAX`; it used to be squeezed through
    /// `i64` (wrapping to -2). The typed `UInt64` variant keeps it exact.
    #[test]
    fn uint64_attributes_keep_their_full_range() {
        let fill = 18_446_744_073_709_551_614_u64;
        assert_eq!(
            attr_value_from(AttributeValue::Ulonglong(fill)),
            AttrValue::UInt64(fill)
        );
        assert_eq!(
            attr_value_from(AttributeValue::Ulonglongs(vec![0, fill])),
            AttrValue::UInt64List(vec![0, fill])
        );
    }

    #[test]
    fn narrow_numeric_types_are_preserved() {
        assert_eq!(
            attr_value_from(AttributeValue::Float(1.5)),
            AttrValue::Float32(1.5)
        );
        assert_eq!(
            attr_value_from(AttributeValue::Schars(vec![1, 2])),
            AttrValue::Int8List(vec![1, 2])
        );
        assert_eq!(
            attr_value_from(AttributeValue::Short(-3)),
            AttrValue::Int16(-3)
        );
        assert_eq!(attr_value_from(AttributeValue::Int(7)), AttrValue::Int32(7));
        assert_eq!(
            attr_value_from(AttributeValue::Longlong(9)),
            AttrValue::Int(9)
        );
        assert_eq!(
            attr_value_from(AttributeValue::Double(0.1)),
            AttrValue::Float(0.1)
        );
    }
}
