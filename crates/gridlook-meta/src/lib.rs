//! `gridlook-meta` extracts structural metadata (dimensions, variables,
//! attributes, groups) from gridded scientific data formats such as
//! NetCDF, Zarr, and Icechunk.

pub mod dispatch;
mod error;
pub mod icechunk;
pub mod model;
pub mod netcdf;
#[cfg(feature = "remote")]
pub mod remote;
pub mod zarr;

pub use dispatch::{
    FormatHint, NETCDF_LIKE_EXTENSIONS, ZARR_ROOT_MARKERS, detect_local_kind,
    has_netcdf_like_extension, summarize_path,
};
pub use error::MetaError;
pub use icechunk::is_icechunk_repo;
#[cfg(feature = "icechunk")]
pub use icechunk::{
    summarize_icechunk, summarize_icechunk_storage, summarize_icechunk_storage_async,
    summarize_icechunk_with,
};
pub use model::{
    AttrScalar, AttrValue, CodecInfo, DatasetSummary, DimInfo, Endianness, FileInfo, FilterInfo,
    GroupSummary, NumKind, SnapshotInfo, SourceFormat, StorageInfo, StorageLayout,
    SummarizeOptions, VarSummary, VersionInfo,
};
pub use netcdf::{summarize_netcdf, summarize_netcdf_with};
#[cfg(feature = "remote")]
pub use remote::{RemoteOptions, Source, summarize_source};
pub use zarr::{summarize_zarr, summarize_zarr_with};
