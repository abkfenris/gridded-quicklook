//! The error type shared by every metadata reader in this crate (NetCDF,
//! Zarr, and Icechunk).

use std::path::PathBuf;

/// Errors that can occur while reading gridded dataset metadata, across all
/// supported formats.
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
    /// I/O failure while reading a Zarr store's metadata files directly off
    /// disk (e.g. `.zgroup`, `zarr.json`).
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A Zarr metadata file's contents were not valid JSON.
    #[error("failed to parse JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// A Zarr metadata file was valid JSON but did not have the shape this
    /// reader expects (missing/malformed fields, unrecognized store layout).
    #[error("invalid Zarr metadata in {path}: {message}")]
    Invalid { path: PathBuf, message: String },
    /// Failure while reading an Icechunk repository: opening its storage or
    /// repository handle, resolving a branch or snapshot, or listing nodes.
    #[error("cannot read Icechunk repository {path}: {message}")]
    Icechunk { path: PathBuf, message: String },
}
