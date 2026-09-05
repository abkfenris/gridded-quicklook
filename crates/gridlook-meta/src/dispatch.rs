//! Format detection and dispatch for local paths.
//!
//! Regular files are routed by what they contain first (the classic-netCDF
//! or HDF5 signature), then by extension; directories by their *contents*
//! (a Zarr root marker, or Icechunk's repository layout), never by name, so
//! a store called anything at all is recognized. Both the Quick Look FFI
//! layer and the CLI route through here so they can never disagree.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::MetaError;
use crate::icechunk::is_icechunk_repo;
use crate::model::{DatasetSummary, SummarizeOptions};

/// File extensions (lowercased, without the leading dot) routed to the
/// NetCDF/HDF5 reader even when the file's signature isn't recognized (see
/// [`has_netcdf_signature`]), so that a truncated or otherwise odd `.nc`
/// still reaches libnetcdf and gets its diagnostic rather than a generic
/// "unsupported file type" error.
pub const NETCDF_LIKE_EXTENSIONS: &[&str] = &["nc", "nc4", "cdf", "h5", "hdf5", "he5"];

/// HDF5 file signature (the start of the superblock).
const HDF5_MAGIC: &[u8; 8] = b"\x89HDF\r\n\x1a\n";

/// Root-level entries that mark a directory as a Zarr store: a v3 node
/// document, a v2 group/array marker, or v2 consolidated metadata. Checked
/// with a handful of `stat` calls -- never by walking the store.
pub const ZARR_ROOT_MARKERS: &[&str] = &["zarr.json", ".zgroup", ".zarray", ".zmetadata"];

/// Which reader family a source should go to. Detected from the source, or
/// supplied by a caller to override detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatHint {
    /// NetCDF-3/4 or HDF5 file.
    NetCdf,
    /// Zarr v2 or v3 store (the reader distinguishes the two).
    Zarr,
    /// Icechunk repository.
    Icechunk,
}

/// Does `name` (a file name or the last segment of a URL) carry one of the
/// [`NETCDF_LIKE_EXTENSIONS`]?
pub fn has_netcdf_like_extension(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| NETCDF_LIKE_EXTENSIONS.contains(&ext.as_str()))
}

/// Does the file start like something libnetcdf can open?
///
/// - Classic netCDF: `CDF` followed by a version byte (1 = classic, 2 =
///   64-bit offset, 5 = CDF5) at offset 0.
/// - netCDF-4 / HDF5: the HDF5 superblock signature at offset 0, or, for a
///   file with a user block, at 512 · 2ⁿ. libhdf5 searches those offsets
///   until it runs off the end of the file; a handful of doublings covers
///   every user block size seen in practice, and each probe is one small
///   `read`.
///
/// Any I/O trouble simply reads as "no signature": the extension fallback
/// and, ultimately, libnetcdf's own error handling take it from there.
pub fn has_netcdf_signature(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 8];

    if read_at(&mut file, 0, &mut magic)
        && (matches!(&magic[..4], b"CDF\x01" | b"CDF\x02" | b"CDF\x05") || &magic == HDF5_MAGIC)
    {
        return true;
    }

    let mut offset = 512;
    for _ in 0..7 {
        if read_at(&mut file, offset, &mut magic) && &magic == HDF5_MAGIC {
            return true;
        }
        offset *= 2;
    }
    false
}

/// Fills `buf` from `offset`; `false` if the file is too short or unreadable.
fn read_at(file: &mut fs::File, offset: u64, buf: &mut [u8]) -> bool {
    file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(buf).is_ok()
}

/// Sniffs what kind of source a local `path` is. `None` means no reader
/// claims it (a file with neither a netCDF/HDF5 signature nor a known
/// extension, or a directory that is neither a Zarr store nor an Icechunk
/// repository).
///
/// Sniffing the signature means a NetCDF file called anything at all is
/// recognized (Quick Look hands over whatever its UTI matched; the CLI takes
/// any path), and the extension list stops having to be exhaustive. A path
/// that does not exist is still classified by extension, so that opening it
/// produces the reader's own "failed to open" error rather than a vaguer
/// "unsupported" one.
pub fn detect_local_kind(path: &Path) -> Option<FormatHint> {
    if path.is_dir() {
        // Zarr's root markers are checked first: they are definitive files
        // at the root, while `is_icechunk_repo` is a directory-layout sniff
        // that a Zarr store could satisfy by coincidence (child groups named
        // `snapshots`/`transactions`/`refs`). An Icechunk repo root never
        // contains a Zarr root marker, so this ordering misroutes neither.
        if ZARR_ROOT_MARKERS
            .iter()
            .any(|marker| path.join(marker).is_file())
        {
            return Some(FormatHint::Zarr);
        }
        if is_icechunk_repo(path) {
            return Some(FormatHint::Icechunk);
        }
        return None;
    }

    let known_extension = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(has_netcdf_like_extension);
    (has_netcdf_signature(path) || known_extension).then_some(FormatHint::NetCdf)
}

/// Why [`detect_local_kind`] returned `None` for `path`, phrased for an end
/// user.
pub fn unsupported_reason(path: &Path) -> String {
    if path.is_dir() {
        return "not a Zarr store or an Icechunk repository".to_owned();
    }
    if !path.exists() {
        return "no such file or directory".to_owned();
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("unsupported file type \".{ext}\""),
        None => "not a netCDF or HDF5 file, and no recognizable file extension".to_owned(),
    }
}

/// Summarizes whatever lives at `path`, detecting its format unless `hint`
/// says otherwise.
pub fn summarize_path(
    path: &Path,
    hint: Option<FormatHint>,
    opts: &SummarizeOptions,
) -> Result<DatasetSummary, MetaError> {
    let kind = match hint.or_else(|| detect_local_kind(path)) {
        Some(kind) => kind,
        None => {
            return Err(MetaError::Unsupported {
                location: path.display().to_string(),
                message: unsupported_reason(path),
            });
        }
    };

    match kind {
        FormatHint::NetCdf => crate::netcdf::summarize_netcdf_with(path, opts),
        FormatHint::Zarr => crate::zarr::summarize_zarr_with(path, opts),
        #[cfg(feature = "icechunk")]
        FormatHint::Icechunk => crate::icechunk::summarize_icechunk_with(path, opts),
        #[cfg(not(feature = "icechunk"))]
        FormatHint::Icechunk => Err(MetaError::Unsupported {
            location: path.display().to_string(),
            message:
                "Icechunk support was not compiled in (enable gridlook-meta's `icechunk` feature)"
                    .to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/data")
            .join(name)
    }

    /// A fresh directory named `name` inside a unique temp dir; keep the
    /// guard alive for the test.
    fn temp_dir(name: &str) -> (tempfile::TempDir, PathBuf) {
        let parent = tempfile::tempdir().expect("create temp dir");
        let dir = parent.path().join(name);
        fs::create_dir_all(&dir).expect("create temp dir");
        (parent, dir)
    }

    /// Copies `fixture` into a temp dir under `new_name`, so the signature
    /// sniff can be exercised with names the extension list never heard of.
    fn renamed_fixture(fixture: &str, new_name: &str) -> (tempfile::TempDir, PathBuf) {
        let (tmp, dir) = temp_dir("renamed");
        let copy = dir.join(new_name);
        fs::copy(self::fixture(fixture), &copy).expect("copy fixture");
        (tmp, copy)
    }

    #[test]
    fn extensions_are_case_insensitive() {
        assert!(has_netcdf_like_extension("a.NC"));
        assert!(has_netcdf_like_extension("b.hdf5"));
        assert!(!has_netcdf_like_extension("c.zarr"));
        assert!(!has_netcdf_like_extension("noext"));
    }

    #[test]
    fn netcdf_files_are_recognized_by_signature_whatever_their_name() {
        let (_tmp, hdf5) = renamed_fixture("simple.nc", "renamed.dat");
        assert!(has_netcdf_signature(&hdf5));
        assert_eq!(detect_local_kind(&hdf5), Some(FormatHint::NetCdf));

        let (_tmp, classic) = renamed_fixture("simple_classic.nc", "no_extension");
        assert!(has_netcdf_signature(&classic));
        assert_eq!(detect_local_kind(&classic), Some(FormatHint::NetCdf));

        let summary = summarize_path(&hdf5, None, &SummarizeOptions::default())
            .expect("a renamed netCDF-4 file summarizes");
        assert_eq!(
            summary
                .root
                .variable("temperature")
                .map(|v| v.dtype.as_str()),
            Some("float32")
        );
    }

    /// A known extension still reaches libnetcdf even without a signature,
    /// so the user gets libnetcdf's diagnostic rather than "unsupported".
    #[test]
    fn known_extension_without_a_signature_still_reaches_the_reader() {
        let (_tmp, fake) = renamed_fixture("../generate.py", "not_really.nc");
        assert!(!has_netcdf_signature(&fake));
        assert_eq!(detect_local_kind(&fake), Some(FormatHint::NetCdf));
        let err = summarize_path(&fake, None, &SummarizeOptions::default())
            .expect_err("a Python script is not a netCDF file");
        assert!(matches!(err, MetaError::Open { .. }), "{err:?}");

        // Neither signature nor extension: unsupported, and the reason says so.
        let (_tmp, plain) = renamed_fixture("../generate.py", "notes");
        assert_eq!(detect_local_kind(&plain), None);
        assert!(unsupported_reason(&plain).contains("not a netCDF or HDF5 file"));
    }

    #[test]
    fn detects_fixture_kinds() {
        assert_eq!(
            detect_local_kind(&fixture("simple.nc")),
            Some(FormatHint::NetCdf)
        );
        assert_eq!(
            detect_local_kind(&fixture("simple_v2.zarr")),
            Some(FormatHint::Zarr)
        );
        assert_eq!(
            detect_local_kind(&fixture("tree.zarr")),
            Some(FormatHint::Zarr)
        );
        assert_eq!(
            detect_local_kind(&fixture("icechunk_repo.icechunk")),
            Some(FormatHint::Icechunk)
        );
        assert_eq!(detect_local_kind(&fixture("..")), None);
        assert_eq!(detect_local_kind(&fixture("../generate.py")), None);
        // Missing files are still classified by extension.
        assert_eq!(
            detect_local_kind(&fixture("does-not-exist.nc")),
            Some(FormatHint::NetCdf)
        );
    }

    /// A Zarr store whose child nodes happen to be named like Icechunk
    /// internals must still be routed to the Zarr reader: the definitive
    /// root markers win over the icechunk directory-layout sniff.
    #[test]
    fn zarr_store_with_icechunk_like_children_routes_to_zarr() {
        let (_tmp, dir) = temp_dir("dispatch.zarr");
        for child in ["snapshots", "transactions"] {
            fs::create_dir_all(dir.join(child)).expect("create child dirs");
        }
        fs::write(
            dir.join("zarr.json"),
            r#"{"zarr_format":3,"node_type":"group","attributes":{}}"#,
        )
        .expect("write root zarr.json");

        assert_eq!(detect_local_kind(&dir), Some(FormatHint::Zarr));
        let summary =
            summarize_path(&dir, None, &SummarizeOptions::default()).expect("summarize zarr store");
        assert_eq!(summary.format, crate::model::SourceFormat::ZarrV3);
    }

    #[test]
    fn unsupported_paths_report_why() {
        let err = summarize_path(&fixture(".."), None, &SummarizeOptions::default())
            .expect_err("a plain directory is unsupported");
        assert!(matches!(err, MetaError::Unsupported { .. }));
        assert!(
            err.to_string()
                .contains("not a Zarr store or an Icechunk repository")
        );

        let err = summarize_path(
            &fixture("../generate.py"),
            None,
            &SummarizeOptions::default(),
        )
        .expect_err("a .py file is unsupported");
        assert!(err.to_string().contains("unsupported file type \".py\""));
    }

    #[test]
    fn hint_overrides_detection() {
        // Forcing a NetCDF file through the Zarr reader fails as a Zarr
        // error, proving the hint won over the extension.
        let err = summarize_path(
            &fixture("simple.nc"),
            Some(FormatHint::Zarr),
            &SummarizeOptions::default(),
        )
        .expect_err("a .nc file is not a Zarr store");
        assert!(matches!(
            err,
            MetaError::Invalid { .. } | MetaError::Io { .. }
        ));
    }
}
