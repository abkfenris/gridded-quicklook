//! `gridlook-cdl` renders a [`DatasetSummary`] as CDL text in the style of
//! `ncdump -h` (and `ncdump -hs`), for every format `gridlook-meta` reads:
//! NetCDF, HDF5, Zarr v2/v3 and Icechunk.
//!
//! Only the header is produced (dimensions, variables, attributes, groups);
//! there is no `data:` section.
//!
//! Known, deliberate deviations from ncdump: groups print in name order
//! rather than creation order; Zarr attributes print in name order (Zarr has
//! no attribute order); numbers from JSON sources (Zarr, Icechunk) print
//! without a type suffix because the store never recorded one.

mod literal;
mod render;
mod specials;
mod types;

use gridlook_meta::{DatasetSummary, SourceFormat};

pub use literal::{NumberPolicy, attr_literal, cdl_name, cdl_string, float_literal};
pub use specials::{global_specials, var_specials};
pub use types::cdl_type_name;

/// How to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdlOptions {
    /// The name after `netcdf` on the first line (ncdump uses the file name
    /// without its extension; `-n` overrides it).
    pub name: String,
    /// Append the special virtual attributes `ncdump -s` shows (`_Storage`,
    /// `_ChunkSizes`, `_Format`, ...). Needs a summary read with
    /// `SummarizeOptions::storage_details`; without one only what the plain
    /// summary carries (`_ChunkSizes`) appears.
    pub specials: bool,
    /// `ncdump -g`: print only these groups (leaf name or full path such as
    /// `/group_a/nested`; `/` selects the root) and their descendants.
    /// Enclosing groups still print as empty wrappers.
    pub groups: Option<Vec<String>>,
}

impl CdlOptions {
    pub fn new(name: impl Into<String>) -> Self {
        CdlOptions {
            name: name.into(),
            specials: false,
            groups: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CdlError {
    /// A `-g` group name matched nothing in the dataset.
    #[error("group \"{0}\" not found")]
    UnknownGroup(String),
}

/// Renders the CDL header for `summary`.
pub fn render_cdl(summary: &DatasetSummary, opts: &CdlOptions) -> Result<String, CdlError> {
    render::render(summary, opts)
}

/// The `ncdump -k` kind string: the reader's precise file kind when it
/// recorded one (`netCDF-4`, `classic`, `Zarr v3`, ...), else the coarse
/// format family.
pub fn kind_string(summary: &DatasetSummary) -> String {
    if let Some(info) = &summary.file_info {
        return info.kind.clone();
    }
    match summary.format {
        SourceFormat::NetCdf => "netCDF",
        SourceFormat::Hdf5 => "HDF5",
        SourceFormat::ZarrV2 => "Zarr v2",
        SourceFormat::ZarrV3 => "Zarr v3",
        SourceFormat::Icechunk => "Icechunk",
    }
    .to_owned()
}
