//! Direct `libnetcdf` inquiries the safe `netcdf` crate does not expose.
//!
//! The `netcdf` crate wraps everything the structural walk needs, but has no
//! getters for the storage details `ncdump -s` reports: the file format,
//! per-variable storage layout, deflate level, shuffle, fletcher32, other
//! HDF5 filters, fill mode, and the hidden netCDF-4 provenance attributes.
//! Its `ncid`/`varid` handles are private, so this module opens the file a
//! second time (read-only) through `netcdf-sys` and resolves groups and
//! variables by name.
//!
//! `libnetcdf` is not thread-safe. Every raw call here runs under
//! [`netcdf_sys::libnetcdf_lock`], the same reentrant lock the `netcdf`
//! crate takes, so the two handles never race each other.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uint};
use std::path::{Path, PathBuf};
use std::ptr;

use netcdf_sys::{
    NC_CHUNKED, NC_COMPACT, NC_CONTIGUOUS, NC_ENDIAN_BIG, NC_ENDIAN_LITTLE, NC_ENOFILTER,
    NC_ENOTATT, NC_FORMAT_64BIT_OFFSET, NC_FORMAT_CDF5, NC_FORMAT_CLASSIC, NC_FORMAT_NETCDF4,
    NC_FORMAT_NETCDF4_CLASSIC, NC_GLOBAL, NC_INT, NC_NOERR, NC_NOWRITE, libnetcdf_lock, nc_close,
    nc_get_att_int, nc_get_att_text, nc_inq_att, nc_inq_format, nc_inq_grp_full_ncid, nc_inq_type,
    nc_inq_var_chunking, nc_inq_var_deflate, nc_inq_var_endian, nc_inq_var_fill,
    nc_inq_var_filter_ids, nc_inq_var_filter_info, nc_inq_var_fletcher32, nc_inq_varid,
    nc_inq_varndims, nc_inq_vartype, nc_open, nc_type,
};

use crate::error::MetaError;
use crate::model::{Endianness, FileInfo, FilterInfo, StorageInfo, StorageLayout};

/// HDF5 filter ids that [`StorageInfo`] reports structurally rather than in
/// its `filters` list.
const H5Z_FILTER_DEFLATE: c_uint = 1;
const H5Z_FILTER_SHUFFLE: c_uint = 2;
const H5Z_FILTER_FLETCHER32: c_uint = 3;

/// A read-only `libnetcdf` handle on one file, closed on drop.
#[derive(Debug)]
pub(super) struct RawFile {
    ncid: c_int,
    format: c_int,
    path: PathBuf,
}

/// Runs `f` while holding the global libnetcdf lock.
fn with_lock<T>(f: impl FnOnce() -> T) -> T {
    let _guard = libnetcdf_lock.lock();
    f()
}

impl RawFile {
    pub(super) fn open(path: &Path) -> Result<Self, MetaError> {
        let c_path =
            CString::new(path.to_string_lossy().as_bytes()).map_err(|_| MetaError::Invalid {
                location: path.display().to_string(),
                message: "path contains a NUL byte".to_owned(),
            })?;
        let mut ncid: c_int = 0;
        // SAFETY: `c_path` is a valid NUL-terminated string that outlives
        // the call, and `ncid` is a valid out-pointer.
        let code = with_lock(|| unsafe { nc_open(c_path.as_ptr(), NC_NOWRITE, &mut ncid) });
        check(code, path)?;

        let mut format: c_int = 0;
        // SAFETY: `ncid` was just returned by a successful `nc_open`;
        // `format` is a valid out-pointer.
        let code = with_lock(|| unsafe { nc_inq_format(ncid, &mut format) });
        if code != NC_NOERR {
            // SAFETY: `ncid` is open and not used again after this.
            with_lock(|| unsafe { nc_close(ncid) });
            return Err(nc_error(code, path));
        }

        Ok(RawFile {
            ncid,
            format,
            path: path.to_path_buf(),
        })
    }

    /// `true` for the two HDF5-backed formats, the only ones with storage
    /// layouts, filters, endianness, and hidden provenance attributes.
    fn is_netcdf4(&self) -> bool {
        matches!(self.format, NC_FORMAT_NETCDF4 | NC_FORMAT_NETCDF4_CLASSIC)
    }

    /// File-level details: the `ncdump -k` kind string plus, for netCDF-4
    /// files, the hidden `_NCProperties` / `_SuperblockVersion` /
    /// `_IsNetcdf4` attributes libnetcdf serves on explicit lookup.
    pub(super) fn file_info(&self) -> Result<FileInfo, MetaError> {
        let kind = match self.format {
            NC_FORMAT_CLASSIC => "classic",
            NC_FORMAT_64BIT_OFFSET => "64-bit offset",
            NC_FORMAT_CDF5 => "cdf5",
            NC_FORMAT_NETCDF4 => "netCDF-4",
            NC_FORMAT_NETCDF4_CLASSIC => "netCDF-4 classic model",
            _ => "unknown",
        }
        .to_owned();

        let mut info = FileInfo {
            kind,
            ..FileInfo::default()
        };
        if self.is_netcdf4() {
            info.nc_properties = self.global_text_attr("_NCProperties")?;
            info.superblock_version = self.global_int_attr("_SuperblockVersion")?;
            info.is_netcdf4 = self.global_int_attr("_IsNetcdf4")?.map(|v| v != 0);
        }
        Ok(info)
    }

    /// Resolves a group by its full path (`""` or `"/"` for the root,
    /// `"/group_a/nested"` otherwise).
    pub(super) fn group_ncid(&self, full_path: &str) -> Result<c_int, MetaError> {
        if full_path.is_empty() || full_path == "/" {
            return Ok(self.ncid);
        }
        let c_name = self.c_name(full_path)?;
        let mut grp: c_int = 0;
        // SAFETY: `self.ncid` is an open handle, `c_name` a valid C string
        // that outlives the call, `grp` a valid out-pointer.
        let code =
            with_lock(|| unsafe { nc_inq_grp_full_ncid(self.ncid, c_name.as_ptr(), &mut grp) });
        check(code, &self.path)?;
        Ok(grp)
    }

    /// Storage details for the variable `name` in group `grp`.
    ///
    /// Layout, compression, filters and endianness exist only in netCDF-4
    /// (HDF5) files; for classic formats only the fill mode is queried,
    /// matching what `ncdump -s` shows for them.
    pub(super) fn var_storage(&self, grp: c_int, name: &str) -> Result<StorageInfo, MetaError> {
        let c_name = self.c_name(name)?;
        let mut varid: c_int = 0;
        // SAFETY: `grp` is a group handle in this open file, `c_name` a valid
        // C string, `varid` a valid out-pointer.
        let code = with_lock(|| unsafe { nc_inq_varid(grp, c_name.as_ptr(), &mut varid) });
        check(code, &self.path)?;

        let mut info = StorageInfo::default();

        let mut no_fill: c_int = 0;
        // SAFETY: `grp`/`varid` identify an existing variable; a NULL
        // fill-value pointer is documented as "don't return the value".
        let code =
            with_lock(|| unsafe { nc_inq_var_fill(grp, varid, &mut no_fill, ptr::null_mut()) });
        check(code, &self.path)?;
        info.no_fill = no_fill != 0;

        if !self.is_netcdf4() {
            return Ok(info);
        }

        let mut ndims: c_int = 0;
        // SAFETY: valid handles and out-pointer.
        let code = with_lock(|| unsafe { nc_inq_varndims(grp, varid, &mut ndims) });
        check(code, &self.path)?;
        let mut storage: c_int = 0;
        let mut chunks = vec![0usize; usize::try_from(ndims).unwrap_or(0)];
        // SAFETY: `chunks` has exactly `ndims` slots, which is how many
        // libnetcdf writes (none for a scalar, where the pointer is unused).
        let code = with_lock(|| unsafe {
            nc_inq_var_chunking(grp, varid, &mut storage, chunks.as_mut_ptr())
        });
        check(code, &self.path)?;
        info.layout = match storage {
            NC_CHUNKED => Some(StorageLayout::Chunked),
            NC_CONTIGUOUS => Some(StorageLayout::Contiguous),
            NC_COMPACT => Some(StorageLayout::Compact),
            _ => None,
        };

        let (mut shuffle, mut deflate, mut level): (c_int, c_int, c_int) = (0, 0, 0);
        // SAFETY: valid handles and out-pointers.
        let code = with_lock(|| unsafe {
            nc_inq_var_deflate(grp, varid, &mut shuffle, &mut deflate, &mut level)
        });
        check(code, &self.path)?;
        info.shuffle = shuffle != 0;
        if deflate != 0 {
            info.deflate_level = u8::try_from(level).ok();
        }

        let mut fletcher32: c_int = 0;
        // SAFETY: valid handles and out-pointer.
        let code = with_lock(|| unsafe { nc_inq_var_fletcher32(grp, varid, &mut fletcher32) });
        check(code, &self.path)?;
        info.fletcher32 = fletcher32 != 0;

        info.endianness = self.var_endianness(grp, varid)?;
        info.filters = self.var_filters(grp, varid)?;

        Ok(info)
    }

    /// Byte order of a multi-byte variable, or `None` when the type is
    /// single-byte (where it is meaningless, and ncdump omits it) or
    /// libnetcdf reports native order.
    fn var_endianness(&self, grp: c_int, varid: c_int) -> Result<Option<Endianness>, MetaError> {
        let mut xtype: nc_type = 0;
        // SAFETY: valid handles and out-pointer.
        let code = with_lock(|| unsafe { nc_inq_vartype(grp, varid, &mut xtype) });
        check(code, &self.path)?;
        let mut size: usize = 0;
        // SAFETY: a NULL name pointer is documented as "don't return the
        // name"; `size` is a valid out-pointer.
        let code = with_lock(|| unsafe { nc_inq_type(grp, xtype, ptr::null_mut(), &mut size) });
        check(code, &self.path)?;
        if size <= 1 {
            return Ok(None);
        }
        let mut endian: c_int = 0;
        // SAFETY: valid handles and out-pointer.
        let code = with_lock(|| unsafe { nc_inq_var_endian(grp, varid, &mut endian) });
        check(code, &self.path)?;
        Ok(match endian {
            NC_ENDIAN_LITTLE => Some(Endianness::Little),
            NC_ENDIAN_BIG => Some(Endianness::Big),
            _ => None,
        })
    }

    /// HDF5 filters in the variable's pipeline other than deflate, shuffle
    /// and fletcher32, which [`Self::var_storage`] reports directly.
    fn var_filters(&self, grp: c_int, varid: c_int) -> Result<Vec<FilterInfo>, MetaError> {
        let mut count: usize = 0;
        // SAFETY: a NULL ids pointer is documented as "only return the
        // count"; `count` is a valid out-pointer.
        let code =
            with_lock(|| unsafe { nc_inq_var_filter_ids(grp, varid, &mut count, ptr::null_mut()) });
        if code == NC_ENOFILTER {
            return Ok(Vec::new());
        }
        check(code, &self.path)?;
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut ids = vec![0 as c_uint; count];
        // SAFETY: `ids` has `count` slots, the number libnetcdf just
        // reported for this variable.
        let code = with_lock(|| unsafe {
            nc_inq_var_filter_ids(grp, varid, &mut count, ids.as_mut_ptr())
        });
        check(code, &self.path)?;
        ids.truncate(count);

        let mut filters = Vec::new();
        for id in ids {
            if matches!(
                id,
                H5Z_FILTER_DEFLATE | H5Z_FILTER_SHUFFLE | H5Z_FILTER_FLETCHER32
            ) {
                continue;
            }
            let mut nparams: usize = 0;
            // SAFETY: NULL params pointer = count only; valid out-pointer.
            let code = with_lock(|| unsafe {
                nc_inq_var_filter_info(grp, varid, id, &mut nparams, ptr::null_mut())
            });
            check(code, &self.path)?;
            let mut params = vec![0 as c_uint; nparams];
            if nparams > 0 {
                // SAFETY: `params` has `nparams` slots as just reported.
                let code = with_lock(|| unsafe {
                    nc_inq_var_filter_info(grp, varid, id, &mut nparams, params.as_mut_ptr())
                });
                check(code, &self.path)?;
                params.truncate(nparams);
            }
            filters.push(FilterInfo { id, params });
        }
        Ok(filters)
    }

    /// Reads a hidden text attribute on the root group, `None` if absent.
    fn global_text_attr(&self, name: &str) -> Result<Option<String>, MetaError> {
        let Some((_, len)) = self.global_attr_shape(name)? else {
            return Ok(None);
        };
        let c_name = self.c_name(name)?;
        // libnetcdf writes exactly `len` chars with no terminator.
        let mut buf = vec![0 as c_char; len + 1];
        // SAFETY: `buf` has `len + 1` slots, at least the `len` libnetcdf
        // writes; the extra slot keeps a NUL terminator for `CStr`.
        let code = with_lock(|| unsafe {
            nc_get_att_text(self.ncid, NC_GLOBAL, c_name.as_ptr(), buf.as_mut_ptr())
        });
        check(code, &self.path)?;
        let bytes: Vec<u8> = buf[..len].iter().map(|&c| c as u8).collect();
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Reads a hidden single-int attribute on the root group, `None` if
    /// absent or not an int.
    fn global_int_attr(&self, name: &str) -> Result<Option<i32>, MetaError> {
        let Some((xtype, len)) = self.global_attr_shape(name)? else {
            return Ok(None);
        };
        if xtype != NC_INT || len == 0 {
            return Ok(None);
        }
        let c_name = self.c_name(name)?;
        let mut buf = vec![0 as c_int; len];
        // SAFETY: `buf` has `len` slots, the attribute's reported length.
        let code = with_lock(|| unsafe {
            nc_get_att_int(self.ncid, NC_GLOBAL, c_name.as_ptr(), buf.as_mut_ptr())
        });
        check(code, &self.path)?;
        Ok(Some(buf[0]))
    }

    /// Type and length of a root-group attribute, `None` when it does not
    /// exist.
    fn global_attr_shape(&self, name: &str) -> Result<Option<(nc_type, usize)>, MetaError> {
        let c_name = self.c_name(name)?;
        let mut xtype: nc_type = 0;
        let mut len: usize = 0;
        // SAFETY: valid handle, valid C string, valid out-pointers.
        let code = with_lock(|| unsafe {
            nc_inq_att(self.ncid, NC_GLOBAL, c_name.as_ptr(), &mut xtype, &mut len)
        });
        if code == NC_ENOTATT {
            return Ok(None);
        }
        check(code, &self.path)?;
        Ok(Some((xtype, len)))
    }

    fn c_name(&self, name: &str) -> Result<CString, MetaError> {
        CString::new(name).map_err(|_| MetaError::Invalid {
            location: self.path.display().to_string(),
            message: format!("name {name:?} contains a NUL byte"),
        })
    }
}

impl Drop for RawFile {
    fn drop(&mut self) {
        // SAFETY: `ncid` was opened by `RawFile::open` and is closed exactly
        // once, here. Close errors have nowhere to go on a read-only handle.
        with_lock(|| unsafe { nc_close(self.ncid) });
    }
}

fn check(code: c_int, path: &Path) -> Result<(), MetaError> {
    if code == NC_NOERR {
        Ok(())
    } else {
        Err(nc_error(code, path))
    }
}

/// Wraps a libnetcdf status code as a [`MetaError::Read`] whose message is
/// libnetcdf's own `nc_strerror` text (via the `netcdf` crate's error type).
fn nc_error(code: c_int, path: &Path) -> MetaError {
    MetaError::Read {
        path: path.to_path_buf(),
        source: ::netcdf::Error::from(code),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/data")
            .join(name)
    }

    #[test]
    fn reports_netcdf4_file_info() {
        let raw = RawFile::open(&fixture("simple.nc")).expect("open simple.nc");
        let info = raw.file_info().expect("file info");
        assert_eq!(info.kind, "netCDF-4");
        assert_eq!(info.is_netcdf4, Some(true));
        assert!(
            info.nc_properties
                .as_deref()
                .is_some_and(|p| p.starts_with("version="))
        );
        assert!(info.superblock_version.is_some());
    }

    #[test]
    fn reports_classic_file_info_without_hdf5_details() {
        let raw = RawFile::open(&fixture("simple_classic.nc")).expect("open classic");
        let info = raw.file_info().expect("file info");
        assert_eq!(info.kind, "classic");
        assert_eq!(info.nc_properties, None);
        assert_eq!(info.is_netcdf4, None);

        let root = raw.group_ncid("").expect("root group");
        let storage = raw.var_storage(root, "temperature").expect("var storage");
        assert_eq!(storage.layout, None);
        assert_eq!(storage.deflate_level, None);
        assert_eq!(storage.endianness, None);
    }

    #[test]
    fn reports_chunked_and_contiguous_layouts() {
        let raw = RawFile::open(&fixture("simple.nc")).expect("open simple.nc");
        let root = raw.group_ncid("/").expect("root group");
        let chunked = raw.var_storage(root, "temperature").expect("temperature");
        assert_eq!(chunked.layout, Some(StorageLayout::Chunked));
        assert_eq!(chunked.endianness, Some(Endianness::Little));
        assert!(chunked.filters.is_empty());
        let contiguous = raw.var_storage(root, "salinity").expect("salinity");
        assert_eq!(contiguous.layout, Some(StorageLayout::Contiguous));
    }

    #[test]
    fn resolves_nested_groups_by_full_path() {
        let raw = RawFile::open(&fixture("groups.nc")).expect("open groups.nc");
        let nested = raw.group_ncid("/group_a/nested").expect("nested group");
        let storage = raw.var_storage(nested, "temperature").expect("nested var");
        assert!(storage.layout.is_some());
        let err = raw.group_ncid("/nope").expect_err("missing group");
        assert!(matches!(err, MetaError::Read { .. }), "{err}");
    }

    #[test]
    fn missing_file_is_an_error() {
        let err = RawFile::open(&fixture("does-not-exist.nc")).expect_err("open fails");
        assert!(matches!(err, MetaError::Read { .. }), "{err}");
    }
}
