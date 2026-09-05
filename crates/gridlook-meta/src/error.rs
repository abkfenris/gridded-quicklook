//! The error type shared by every metadata reader in this crate (NetCDF,
//! Zarr, and Icechunk).

use std::path::PathBuf;

/// Errors that can occur while reading gridded dataset metadata, across all
/// supported formats.
///
/// NetCDF readers always work on a local file, so their variants carry a
/// `PathBuf`; everything else carries a `location` string that is a path
/// for local stores and a URL for remote ones.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("failed to open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: ::netcdf::Error,
    },
    /// libnetcdf reported an error while inquiring about a file's storage
    /// details (see the raw layer in the NetCDF reader).
    #[error("failed to read metadata from {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: ::netcdf::Error,
    },
    /// I/O failure while reading a Zarr store's metadata documents (e.g.
    /// `.zgroup`, `zarr.json`) from local disk.
    #[error("failed to read {location}: {source}")]
    Io {
        location: String,
        #[source]
        source: std::io::Error,
    },
    /// A Zarr metadata document's contents were not valid JSON.
    #[error("failed to parse JSON in {location}: {source}")]
    Json {
        location: String,
        #[source]
        source: serde_json::Error,
    },
    /// A Zarr metadata document was valid JSON but did not have the shape
    /// this reader expects (missing/malformed fields, unrecognized store
    /// layout).
    #[error("invalid Zarr metadata in {location}: {message}")]
    Invalid { location: String, message: String },
    /// Failure while reading an Icechunk repository: opening its storage or
    /// repository handle, resolving a branch or snapshot, or listing nodes.
    #[error("cannot read Icechunk repository {location}: {message}")]
    Icechunk { location: String, message: String },
    /// No reader claims the source (unknown file extension, a directory that
    /// is neither a Zarr store nor an Icechunk repository, or a format whose
    /// reader was not compiled in).
    #[error("unsupported source {location}: {message}")]
    Unsupported { location: String, message: String },
    /// The store cannot enumerate children (typically a plain HTTP server),
    /// and the Zarr hierarchy has no consolidated metadata to fall back on.
    #[error("cannot list {location}: {message}")]
    ListingUnsupported { location: String, message: String },
    /// Failure talking to a remote object store.
    #[cfg(feature = "remote")]
    #[error("failed to access {location}: {source}")]
    Remote {
        location: String,
        #[source]
        source: object_store::Error,
    },
}
