//! `gridded-meta` extracts structural metadata (dimensions, variables,
//! attributes, groups) from gridded scientific data formats such as
//! NetCDF, Zarr, and Icechunk.

pub mod model;

pub use model::{
    AttrValue, DatasetSummary, DimInfo, GroupSummary, SourceFormat, VarSummary, VersionInfo,
};
