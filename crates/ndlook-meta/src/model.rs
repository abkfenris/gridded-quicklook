//! Format-agnostic summary of a gridded dataset's structure.
//!
//! Every format reader (NetCDF/HDF5, Zarr, Icechunk) produces a
//! [`DatasetSummary`]; the HTML renderer consumes this model and never
//! sees format-specific types.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::de::{Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Which reader produced the summary. Rendered as a format badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    NetCdf,
    Hdf5,
    ZarrV2,
    ZarrV3,
    Icechunk,
    Grib,
}

/// Top-level summary of one dataset / store / repo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetSummary {
    pub format: SourceFormat,
    pub root: GroupSummary,
    /// Present only for version-controlled stores (Icechunk).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_info: Option<VersionInfo>,
}

/// One group (netCDF-4/HDF5 group, Zarr group, DataTree node).
///
/// `children` makes the model a datatree: renderers must recurse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupSummary {
    /// Group name; empty string for the root group.
    pub name: String,
    pub dims: Vec<DimInfo>,
    /// Variables classified as coordinates (name ∈ dims, or listed in a
    /// `coordinates` attribute — xarray's heuristic).
    pub coords: Vec<VarSummary>,
    pub data_vars: Vec<VarSummary>,
    pub attrs: Vec<(String, AttrValue)>,
    pub children: Vec<GroupSummary>,
}

impl GroupSummary {
    /// Builds a [`GroupSummary`] from a flat list of variables and children,
    /// applying xarray's coordinate-classification heuristic and the
    /// deterministic-ordering conventions shared by every format reader.
    ///
    /// A variable is classified as a coordinate if its name matches one of
    /// its own dimensions ("dimension coordinate"), or if it is named in
    /// some sibling variable's `coordinates` attribute within `vars`.
    /// `coords`, `data_vars`, and `children` are all sorted by name.
    ///
    /// `dims` is `Some` for formats with a real group-level dimension
    /// registry (netCDF), which is used as given (including `is_unlimited`
    /// flags); it is `None` for formats with no such registry (Zarr,
    /// Icechunk), in which case the group's dims are derived from the union
    /// of its variables' own `(name, size)` pairs, with `is_unlimited`
    /// always `false` — Zarr arrays have no notion of an
    /// unlimited/appendable dimension distinct from `shape`.
    pub fn from_parts(
        name: String,
        dims: Option<Vec<DimInfo>>,
        attrs: Vec<(String, AttrValue)>,
        vars: Vec<VarSummary>,
        mut children: Vec<GroupSummary>,
    ) -> Self {
        let mut coord_names: HashSet<String> = HashSet::new();
        for var in &vars {
            if let Some((_, AttrValue::Text(names))) =
                var.attrs.iter().find(|(k, _)| k == "coordinates")
            {
                coord_names.extend(names.split_whitespace().map(str::to_owned));
            }
        }

        let mut coords = Vec::new();
        let mut data_vars = Vec::new();
        for var in vars {
            let is_dim_coord = var.dims.contains(&var.name);
            if is_dim_coord || coord_names.contains(&var.name) {
                coords.push(var);
            } else {
                data_vars.push(var);
            }
        }
        coords.sort_by(|a, b| a.name.cmp(&b.name));
        data_vars.sort_by(|a, b| a.name.cmp(&b.name));

        let dims = dims.unwrap_or_else(|| {
            let mut dims_map: BTreeMap<String, u64> = BTreeMap::new();
            for var in coords.iter().chain(data_vars.iter()) {
                for (dim_name, size) in var.dims.iter().zip(var.shape.iter()) {
                    dims_map.entry(dim_name.clone()).or_insert(*size);
                }
            }
            dims_map
                .into_iter()
                .map(|(name, size)| DimInfo {
                    name,
                    size,
                    is_unlimited: false,
                })
                .collect()
        });

        children.sort_by(|a, b| a.name.cmp(&b.name));

        GroupSummary {
            name,
            dims,
            coords,
            data_vars,
            attrs,
            children,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DimInfo {
    pub name: String,
    pub size: u64,
    pub is_unlimited: bool,
}

/// One variable/array's structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VarSummary {
    pub name: String,
    /// Human-readable dtype, e.g. `float32`, `int64`, `|S8`.
    pub dtype: String,
    pub dims: Vec<String>,
    pub shape: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<u64>>,
    pub attrs: Vec<(String, AttrValue)>,
    /// Short inline value peek for small variables (e.g. first few values of
    /// a 1-D coordinate), already formatted for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Attribute values, preserving enough type fidelity for faithful display.
///
/// # JSON wire format
///
/// Externally tagged: every value is a single-key object naming its
/// variant — `{"Text":"degC"}`, `{"Int":3}`, `{"Float":1.0}`,
/// `{"IntList":[1,2]}`, `{"FloatList":[1.0,2.0]}`,
/// `{"TextList":["a","b"]}`. The tag is what lets a decoder — notably the
/// Swift mirror in `apple/App/DatasetModel.swift` — reconstruct the exact
/// variant. Under the untagged form this type used to carry, a decoder had
/// to guess from the JSON shape in declaration order, so `1.0` came back as
/// `Int(1)` (JSON has one number type, and the integer arm was tried first)
/// and a one-element list was indistinguishable from a scalar to any
/// decoder that ordered its arms differently.
///
/// Non-finite floats are written as strings — `{"Float":"NaN"}`,
/// `{"Float":"inf"}`, `{"Float":"-inf"}`, and the same spellings for
/// entries inside a `FloatList` — because JSON has no literal for them and
/// `serde_json` otherwise flattens all three to `null`, which loses the
/// value entirely and, for `-inf`, even its sign. This is the common case
/// rather than an exotic one: `_FillValue` is NaN on nearly every CF
/// dataset. Finite floats stay JSON numbers, and deserialization accepts
/// either spelling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AttrValue {
    Text(String),
    Int(i64),
    Float(#[serde(with = "wire_float")] f64),
    IntList(Vec<i64>),
    FloatList(#[serde(with = "wire_float_list")] Vec<f64>),
    TextList(Vec<String>),
}

/// The string spellings [`AttrValue`] uses for the three non-finite floats.
/// Both Rust's `f64::from_str` and Swift's `Double(_:)` initializer parse
/// all three back, so neither end needs a bespoke table to decode them.
const NAN_TEXT: &str = "NaN";
const INF_TEXT: &str = "inf";
const NEG_INF_TEXT: &str = "-inf";

/// One `f64` in [`AttrValue`]'s JSON form: a plain JSON number when finite,
/// one of [`NAN_TEXT`] / [`INF_TEXT`] / [`NEG_INF_TEXT`] when not.
///
/// A wrapper type rather than a pair of free functions so the exact same
/// rule can be applied element-wise inside `FloatList` — a `Vec<f64>` of
/// `WireFloat`s serializes as a sequence of the scalar form.
struct WireFloat(f64);

impl Serialize for WireFloat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0.is_finite() {
            serializer.serialize_f64(self.0)
        } else if self.0.is_nan() {
            // NaN's sign bit is not meaningful, so all NaNs share a spelling.
            serializer.serialize_str(NAN_TEXT)
        } else if self.0.is_sign_positive() {
            serializer.serialize_str(INF_TEXT)
        } else {
            serializer.serialize_str(NEG_INF_TEXT)
        }
    }
}

impl<'de> Deserialize<'de> for WireFloat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(WireFloatVisitor)
            .map(WireFloat)
    }
}

struct WireFloatVisitor;

impl Visitor<'_> for WireFloatVisitor {
    type Value = f64;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "a number or a non-finite float spelled as a string")
    }

    fn visit_f64<E>(self, v: f64) -> Result<f64, E> {
        Ok(v)
    }

    /// A JSON number without a fractional part arrives as an integer, so a
    /// `Float` written as `1` (or by some other producer that trims `.0`)
    /// still decodes.
    fn visit_i64<E>(self, v: i64) -> Result<f64, E> {
        Ok(v as f64)
    }

    fn visit_u64<E>(self, v: u64) -> Result<f64, E> {
        Ok(v as f64)
    }

    /// Delegates to Rust's own float parser rather than matching the three
    /// constants above, so the alternative spellings other encoders emit
    /// (`Infinity`, `-Infinity`, `nan`, any letter case) decode too.
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<f64, E> {
        v.parse::<f64>()
            .map_err(|_| E::invalid_value(Unexpected::Str(v), &self))
    }
}

/// `#[serde(with = ...)]` glue applying [`WireFloat`]'s rule to a scalar
/// `AttrValue::Float`.
mod wire_float {
    use super::WireFloat;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        WireFloat(*value).serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        WireFloat::deserialize(deserializer).map(|wire| wire.0)
    }
}

/// The same glue for `AttrValue::FloatList`, applied to each entry: a list
/// of fill values or valid ranges is exactly where `NaN`/`inf` show up.
mod wire_float_list {
    use super::WireFloat;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        values: &[f64],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(values.iter().copied().map(WireFloat))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<f64>, D::Error> {
        let wires = Vec::<WireFloat>::deserialize(deserializer)?;
        Ok(wires.into_iter().map(|wire| wire.0).collect())
    }
}

/// One snapshot in a version-controlled store's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// RFC 3339 timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrote_at: Option<String>,
}

/// Version metadata for an Icechunk repo, scoped to whichever ref (branch,
/// tag, or bare snapshot) was previewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Display name of the previewed ref: a branch name, a tag name, or a
    /// snapshot id, depending on `ref_kind`. Named `branch` rather than
    /// something ref-neutral to avoid disturbing existing callers/tests
    /// that only ever previewed `main`; `ref_kind` disambiguates for
    /// renderers that need to label it correctly.
    pub branch: String,
    /// "branch", "tag", or "snapshot", labeling what kind of ref `branch`
    /// names. `#[serde(default)]` so JSON produced before this field
    /// existed still deserializes (as `None`, i.e. "assume branch").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_kind: Option<String>,
    /// Every branch in the repo, sorted by name — but only when the caller
    /// asked for the listing (`ListRefs::Yes` in the Icechunk reader);
    /// empty otherwise, since listing costs an extra store round-trip that
    /// a preview which never shows the list shouldn't pay. `#[serde(default)]`
    /// keeps pre-existing JSON fixtures/snapshots deserializable.
    #[serde(default)]
    pub branches: Vec<String>,
    /// Every tag in the repo, sorted by name, under the same
    /// listed-on-request rule as `branches`. `#[serde(default)]` keeps
    /// pre-existing JSON fixtures/snapshots deserializable.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Newest first; the tip snapshot is `ancestry[0]`.
    pub ancestry: Vec<SnapshotInfo>,
    /// `true` if the ancestry walk was capped before reaching the repo's
    /// initial snapshot (see `ANCESTRY_LIMIT` in the Icechunk reader).
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_json(value: &AttrValue) -> String {
        serde_json::to_string(value).expect("AttrValue always serializes")
    }

    fn from_json(json: &str) -> AttrValue {
        serde_json::from_str(json).unwrap_or_else(|err| panic!("decode {json}: {err}"))
    }

    /// Every variant carries its name on the wire, so a decoder never has
    /// to guess from the JSON shape.
    #[test]
    fn attr_values_are_externally_tagged() {
        assert_eq!(
            to_json(&AttrValue::Text("degC".to_owned())),
            r#"{"Text":"degC"}"#
        );
        assert_eq!(to_json(&AttrValue::Int(3)), r#"{"Int":3}"#);
        assert_eq!(
            to_json(&AttrValue::IntList(vec![1, 2])),
            r#"{"IntList":[1,2]}"#
        );
        assert_eq!(
            to_json(&AttrValue::TextList(vec!["a".to_owned()])),
            r#"{"TextList":["a"]}"#
        );
    }

    /// The bug the tagging fixes: an untagged `1.0` decoded as `Int(1)`,
    /// because JSON has a single number type and the integer arm was tried
    /// first. A whole-numbered float must stay a float in both directions.
    #[test]
    fn a_whole_numbered_float_stays_a_float() {
        assert_eq!(to_json(&AttrValue::Float(1.0)), r#"{"Float":1.0}"#);
        assert_eq!(from_json(r#"{"Float":1.0}"#), AttrValue::Float(1.0));
        // ... and a producer that trimmed the `.0` is still understood.
        assert_eq!(from_json(r#"{"Float":1}"#), AttrValue::Float(1.0));
    }

    /// JSON has no non-finite literals and `serde_json` writes all three as
    /// `null`, so they travel as strings instead — including `-inf`, whose
    /// sign the `null` form destroyed.
    #[test]
    fn non_finite_floats_round_trip_as_strings() {
        assert_eq!(to_json(&AttrValue::Float(f64::NAN)), r#"{"Float":"NaN"}"#);
        assert_eq!(
            to_json(&AttrValue::Float(f64::INFINITY)),
            r#"{"Float":"inf"}"#
        );
        assert_eq!(
            to_json(&AttrValue::Float(f64::NEG_INFINITY)),
            r#"{"Float":"-inf"}"#
        );

        match from_json(r#"{"Float":"NaN"}"#) {
            AttrValue::Float(f) => assert!(f.is_nan(), "expected NaN, got {f}"),
            other => panic!("expected Float, got {other:?}"),
        }
        assert_eq!(
            from_json(r#"{"Float":"inf"}"#),
            AttrValue::Float(f64::INFINITY)
        );
        assert_eq!(
            from_json(r#"{"Float":"-inf"}"#),
            AttrValue::Float(f64::NEG_INFINITY)
        );
        // Spellings other encoders use decode too.
        assert_eq!(
            from_json(r#"{"Float":"-Infinity"}"#),
            AttrValue::Float(f64::NEG_INFINITY)
        );
    }

    /// The same rule applies entry-by-entry inside a list, which is where
    /// fill values and valid ranges actually live.
    #[test]
    fn float_lists_mix_finite_and_non_finite_entries() {
        let list = AttrValue::FloatList(vec![0.0, f64::NEG_INFINITY, f64::INFINITY]);
        assert_eq!(to_json(&list), r#"{"FloatList":[0.0,"-inf","inf"]}"#);
        assert_eq!(
            from_json(r#"{"FloatList":[0.0,"-inf","inf"]}"#),
            AttrValue::FloatList(vec![0.0, f64::NEG_INFINITY, f64::INFINITY])
        );
    }

    #[test]
    fn a_float_that_is_neither_a_number_nor_a_float_spelling_is_rejected() {
        let err = serde_json::from_str::<AttrValue>(r#"{"Float":"twelve"}"#)
            .expect_err("\"twelve\" is not a float");
        assert!(
            err.to_string().contains("twelve"),
            "the message should name the offending value, got: {err}"
        );
    }
}
