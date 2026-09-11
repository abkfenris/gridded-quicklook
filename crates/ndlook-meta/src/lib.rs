//! `ndlook-meta` extracts structural metadata (dimensions, variables,
//! attributes, groups) from gridded scientific data formats such as
//! NetCDF, Zarr, and Icechunk.

mod error;
pub mod grib;
pub mod icechunk;
pub mod model;
pub mod netcdf;
pub mod zarr;

pub use error::MetaError;
pub use grib::summarize_grib;
pub use icechunk::is_icechunk_repo;
#[cfg(feature = "icechunk")]
pub use icechunk::{IcechunkRef, summarize_icechunk, summarize_icechunk_at};
pub use model::{
    AttrValue, DatasetSummary, DimInfo, GroupSummary, SnapshotInfo, SourceFormat, VarSummary,
    VersionInfo,
};
pub use netcdf::summarize_netcdf;
pub use zarr::summarize_zarr;
