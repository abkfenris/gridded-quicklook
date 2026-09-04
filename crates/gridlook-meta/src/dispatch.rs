//! Format detection and dispatch for local paths.
//!
//! Regular files are routed by extension; directories by their *contents*
//! (a Zarr root marker, or Icechunk's repository layout), never by name, so
//! a store called anything at all is recognized. Both the Quick Look FFI
//! layer and the CLI route through here so they can never disagree.

use std::path::Path;

use crate::error::MetaError;
use crate::icechunk::is_icechunk_repo;
use crate::model::{DatasetSummary, SummarizeOptions};

/// File extensions (lowercased, without the leading dot) routed to the
/// NetCDF/HDF5 reader.
pub const NETCDF_LIKE_EXTENSIONS: &[&str] = &["nc", "nc4", "cdf", "h5", "hdf5", "he5"];

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

/// Sniffs what kind of source a local `path` is. `None` means no reader
/// claims it (unknown extension, or a directory that is neither a Zarr store
/// nor an Icechunk repository).
///
/// A path that does not exist is still classified by extension, so that
/// opening it produces the reader's own "failed to open" error rather than a
/// vaguer "unsupported" one.
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

    let name = path.file_name()?.to_str()?;
    has_netcdf_like_extension(name).then_some(FormatHint::NetCdf)
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
        None => "no recognizable file extension".to_owned(),
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

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gridlook-meta-dispatch-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn extensions_are_case_insensitive() {
        assert!(has_netcdf_like_extension("a.NC"));
        assert!(has_netcdf_like_extension("b.hdf5"));
        assert!(!has_netcdf_like_extension("c.zarr"));
        assert!(!has_netcdf_like_extension("noext"));
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
        let dir = temp_dir("dispatch.zarr");
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
        let _ = fs::remove_dir_all(&dir);
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
