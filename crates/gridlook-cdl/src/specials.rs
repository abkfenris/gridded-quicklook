//! The "special" virtual attributes `ncdump -s` appends: per-variable
//! storage details and file-level format details, presented as attributes
//! so the output stays CDL.
//!
//! NetCDF names follow ncdump exactly (`_Storage`, `_ChunkSizes`,
//! `_DeflateLevel`, `_Shuffle`, `_Fletcher32`, `_Endianness`, `_Filter`,
//! `_NoFill`, `_Format`, `_NCProperties`, `_SuperblockVersion`,
//! `_IsNetcdf4`). Zarr and Icechunk have no ncdump precedent, so their
//! extras use the same leading-underscore style: `_Codecs`, `_Order`,
//! `_ChunkKeyEncoding`, `_StringLength`, and `_Icechunk*` for the version
//! history.

use gridlook_meta::{
    AttrValue, DatasetSummary, Endianness, SourceFormat, StorageLayout, VarSummary,
};

use crate::types::fixed_string_length;

fn text(s: &str) -> AttrValue {
    AttrValue::Text(s.to_owned())
}

/// ncdump prints these as `int`; fall back to `int64` only when a value
/// does not fit.
fn int_list(values: &[u64]) -> AttrValue {
    match values
        .iter()
        .map(|&v| i32::try_from(v))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(ints) => AttrValue::Int32List(ints),
        Err(_) => AttrValue::IntList(values.iter().map(|&v| v as i64).collect()),
    }
}

/// Special attributes for one variable, in ncdump's order, given the
/// dataset's format.
pub fn var_specials(var: &VarSummary, format: SourceFormat) -> Vec<(String, AttrValue)> {
    let mut out: Vec<(String, AttrValue)> = Vec::new();
    let Some(storage) = &var.storage else {
        return out;
    };
    let mut push = |name: &str, value: AttrValue| out.push((name.to_owned(), value));

    if let Some(layout) = storage.layout {
        let name = match layout {
            StorageLayout::Chunked => "chunked",
            StorageLayout::Contiguous => "contiguous",
            StorageLayout::Compact => "compact",
        };
        push("_Storage", text(name));
        if layout == StorageLayout::Chunked
            && let Some(chunks) = &var.chunks
        {
            push("_ChunkSizes", int_list(chunks));
        }
    }
    if let Some(level) = storage.deflate_level {
        push("_DeflateLevel", AttrValue::Int32(level.into()));
    }
    if storage.shuffle {
        push("_Shuffle", text("true"));
    }
    if storage.fletcher32 {
        push("_Fletcher32", text("true"));
    }
    if let Some(endianness) = storage.endianness {
        push(
            "_Endianness",
            text(match endianness {
                Endianness::Little => "little",
                Endianness::Big => "big",
            }),
        );
    }
    if !storage.filters.is_empty() {
        // ncdump: `"id,p1,p2|id2,..."`.
        let spec = storage
            .filters
            .iter()
            .map(|f| {
                std::iter::once(f.id.to_string())
                    .chain(f.params.iter().map(u32::to_string))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("|");
        push("_Filter", text(&spec));
    }
    if storage.no_fill {
        push("_NoFill", text("true"));
    }

    // Zarr / Icechunk extras.
    if !storage.codecs.is_empty() {
        let codecs = storage
            .codecs
            .iter()
            .map(|c| match &c.configuration {
                Some(config) => format!("{}({config})", c.name),
                None => c.name.clone(),
            })
            .collect();
        push("_Codecs", AttrValue::TextList(codecs));
    }
    if let Some(fill) = &storage.fill_value {
        let has_user_fill = var.attrs.iter().any(|(k, _)| k == "_FillValue");
        if !has_user_fill {
            push("_FillValue", fill.clone());
        }
    }
    if let Some(order) = storage.order {
        push("_Order", text(&order.to_string()));
    }
    if let Some(encoding) = &storage.chunk_key_encoding {
        push("_ChunkKeyEncoding", text(encoding));
    }
    if !matches!(format, SourceFormat::NetCdf | SourceFormat::Hdf5) {
        // A netCDF `char` variable carries its width as a trailing dim; a
        // Zarr `|S{n}` array has no such dim, so say how wide it is.
        if let Some(width) = fixed_string_length(&var.dtype) {
            push("_StringLength", int_list(&[width]));
        }
    }
    out
}

/// Special global attributes: the format kind, netCDF-4 provenance, and
/// Icechunk version information.
pub fn global_specials(summary: &DatasetSummary) -> Vec<(String, AttrValue)> {
    let mut out: Vec<(String, AttrValue)> = Vec::new();
    let mut push = |name: &str, value: AttrValue| out.push((name.to_owned(), value));

    push("_Format", text(&crate::kind_string(summary)));
    if let Some(info) = &summary.file_info {
        if let Some(props) = &info.nc_properties {
            push("_NCProperties", text(props));
        }
        if let Some(version) = info.superblock_version {
            push("_SuperblockVersion", AttrValue::Int32(version));
        }
        if let Some(is_nc4) = info.is_netcdf4 {
            push("_IsNetcdf4", AttrValue::Int32(is_nc4.into()));
        }
    }
    if let Some(version) = &summary.version_info {
        push("_IcechunkBranch", text(&version.branch));
        if let Some(tip) = version.ancestry.first() {
            push("_IcechunkSnapshot", text(&tip.id));
            if let Some(message) = &tip.message {
                push("_IcechunkMessage", text(message));
            }
            if let Some(wrote_at) = &tip.wrote_at {
                push("_IcechunkWroteAt", text(wrote_at));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use gridlook_meta::{CodecInfo, FilterInfo, StorageInfo};

    use super::*;

    fn var(storage: StorageInfo) -> VarSummary {
        VarSummary {
            name: "v".into(),
            dtype: "float32".into(),
            dims: vec!["x".into()],
            shape: vec![4],
            chunks: Some(vec![2]),
            attrs: Vec::new(),
            preview: None,
            storage: Some(storage),
        }
    }

    fn names(specials: &[(String, AttrValue)]) -> Vec<&str> {
        specials.iter().map(|(k, _)| k.as_str()).collect()
    }

    #[test]
    fn netcdf_specials_follow_ncdump_order() {
        let v = var(StorageInfo {
            layout: Some(StorageLayout::Chunked),
            deflate_level: Some(4),
            shuffle: true,
            fletcher32: true,
            endianness: Some(Endianness::Big),
            no_fill: true,
            filters: vec![FilterInfo {
                id: 32015,
                params: vec![3],
            }],
            ..StorageInfo::default()
        });
        let specials = var_specials(&v, SourceFormat::NetCdf);
        assert_eq!(
            names(&specials),
            vec![
                "_Storage",
                "_ChunkSizes",
                "_DeflateLevel",
                "_Shuffle",
                "_Fletcher32",
                "_Endianness",
                "_Filter",
                "_NoFill"
            ]
        );
        assert_eq!(specials[1].1, AttrValue::Int32List(vec![2]));
        assert_eq!(specials[6].1, AttrValue::Text("32015,3".into()));
    }

    #[test]
    fn contiguous_variables_have_no_chunk_sizes() {
        let v = var(StorageInfo {
            layout: Some(StorageLayout::Contiguous),
            ..StorageInfo::default()
        });
        assert_eq!(
            names(&var_specials(&v, SourceFormat::NetCdf)),
            vec!["_Storage"]
        );
    }

    #[test]
    fn zarr_specials_add_codecs_and_metadata_fill() {
        let mut v = var(StorageInfo {
            layout: Some(StorageLayout::Chunked),
            codecs: vec![CodecInfo {
                name: "bytes".into(),
                configuration: Some(serde_json::json!({"endian": "little"})),
            }],
            fill_value: Some(AttrValue::Float32(f32::NAN)),
            order: Some('C'),
            chunk_key_encoding: Some("default/".into()),
            ..StorageInfo::default()
        });
        v.dtype = "|S6".into();
        let specials = var_specials(&v, SourceFormat::ZarrV3);
        assert_eq!(
            names(&specials),
            vec![
                "_Storage",
                "_ChunkSizes",
                "_Codecs",
                "_FillValue",
                "_Order",
                "_ChunkKeyEncoding",
                "_StringLength"
            ]
        );
        assert_eq!(
            specials[2].1,
            AttrValue::TextList(vec!["bytes({\"endian\":\"little\"})".into()])
        );

        // A user `_FillValue` attribute wins over the metadata one.
        v.attrs.push(("_FillValue".into(), AttrValue::Float(1.0)));
        assert!(!names(&var_specials(&v, SourceFormat::ZarrV3)).contains(&"_FillValue"));
    }

    #[test]
    fn variables_without_storage_have_no_specials() {
        let mut v = var(StorageInfo::default());
        v.storage = None;
        assert!(var_specials(&v, SourceFormat::NetCdf).is_empty());
    }
}
