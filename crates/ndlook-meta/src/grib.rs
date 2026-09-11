//! GRIB1/GRIB2 metadata reader.
//!
//! A GRIB file is a flat concatenation of self-delimiting *messages*, each
//! one a single 2-D field for a single variable, level, reference time and
//! forecast step. There is no container-level table of contents, so the
//! only way to describe a file is to walk it message by message.
//!
//! This reader reads each message's header sections (via `gribberish`) and
//! never unpacks the packed data section, then re-assembles the messages
//! into the hypercubes a user thinks in: one variable per (parameter, level
//! type, statistical process), with `time` / `step` / level / horizontal
//! dimensions recovered from the distinct values seen across that group's
//! messages. This is the same reshaping cfgrib does for xarray, and the
//! attributes are named to match (`GRIB_shortName`, `GRIB_typeOfLevel`, …).
//!
//! Real-world GRIB files are frequently *not* a clean hypercube: mixed
//! grids, ragged level sets, and truncated downloads are all normal. A
//! preview must never fail on one, so any group that doesn't reshape
//! cleanly is demoted, message by message, into an `unaligned` child group
//! rather than turned into an error.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::Path;

use gribberish::message::MessageIterator;
use gribberish::message_metadata::MessageMetadata;
use gribberish::templates::product::tables::FixedSurfaceType;
use memmap2::Mmap;

use crate::error::MetaError;
use crate::model::{AttrValue, DatasetSummary, GroupSummary, SourceFormat, VarSummary};

/// Upper bound on messages read from one file. A global model run can hold
/// hundreds of thousands; a preview only ever shows a summary, so stopping
/// early bounds the work and the file says so in a root attribute.
const MESSAGE_LIMIT: usize = 10_000;

/// Summarize the structure of the GRIB file at `path`.
///
/// Both GRIB editions are handled (`gribberish` reads GRIB1 and GRIB2);
/// the edition is reported as a root attribute rather than as a separate
/// [`SourceFormat`], since the two are the same format to a user.
pub fn summarize_grib(path: &Path) -> Result<DatasetSummary, MetaError> {
    let file = File::open(path).map_err(|source| MetaError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    // SAFETY: mapping a file is only unsound if another process truncates
    // or writes it while the map is live, which would fault on access. The
    // map exists solely for the duration of this call, over a file the user
    // just asked to preview; the same caveat applies to every mmap-based
    // reader. Mapping (rather than reading) matters here because GRIB files
    // routinely run to hundreds of megabytes while the header sections this
    // reader touches are a tiny, scattered fraction of that.
    let mmap = unsafe { Mmap::map(&file) }.map_err(|source| MetaError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    summarize_grib_bytes(&mmap, path)
}

fn summarize_grib_bytes(data: &[u8], path: &Path) -> Result<DatasetSummary, MetaError> {
    let mut metas: Vec<MessageMetadata> = Vec::new();
    let mut seen = 0usize;
    let mut unparsed = 0usize;
    let mut truncated = false;

    for message in MessageIterator::from_data(data, 0) {
        if seen >= MESSAGE_LIMIT {
            truncated = true;
            break;
        }
        seen += 1;
        match MessageMetadata::try_from(&message) {
            Ok(metadata) => metas.push(metadata),
            // A message whose product/grid template this build doesn't
            // understand is counted, not fatal: the rest of the file still
            // previews.
            Err(_) => unparsed += 1,
        }
    }

    if seen == 0 {
        return Err(MetaError::Grib {
            path: path.to_path_buf(),
            message: "no GRIB messages found".to_string(),
        });
    }

    let mut root_attrs = vec![("GRIB_edition".to_string(), AttrValue::Int(edition(data)))];
    root_attrs.push((
        "ndlook:message_count".to_string(),
        AttrValue::Int(seen as i64),
    ));
    if truncated {
        root_attrs.push((
            "ndlook:truncated".to_string(),
            AttrValue::Text(format!(
                "summarized the first {MESSAGE_LIMIT} messages; the file holds more"
            )),
        ));
    }
    if unparsed > 0 {
        root_attrs.push((
            "ndlook:unreadable_messages".to_string(),
            AttrValue::Int(unparsed as i64),
        ));
    }

    let (vars, unaligned) = build_variables(&metas);
    let children = if unaligned.is_empty() {
        Vec::new()
    } else {
        vec![GroupSummary::from_parts(
            "unaligned".to_string(),
            None,
            vec![(
                "ndlook:note".to_string(),
                AttrValue::Text(
                    "messages that do not reshape into a regular hypercube, listed one per message"
                        .to_string(),
                ),
            )],
            unaligned,
            Vec::new(),
        )]
    };

    Ok(DatasetSummary {
        format: SourceFormat::Grib,
        root: GroupSummary::from_parts(String::new(), None, root_attrs, vars, children),
        version_info: None,
    })
}

/// GRIB edition number, byte 7 of the indicator section. Defaults to 2 for
/// a file too short or too odd to say.
fn edition(data: &[u8]) -> i64 {
    if data.len() > 7 && &data[..4] == b"GRIB" {
        i64::from(data[7])
    } else {
        2
    }
}

/// One recovered axis, before it is interned under a final name.
struct Axis {
    base: &'static str,
    dtype: &'static str,
    values: Vec<String>,
    /// `false` when `values` are placeholders that carry only the axis
    /// length, because materializing the real coordinates would cost more
    /// than a preview is worth (see [`placeholder`]).
    materialized: bool,
    units: Option<String>,
}

/// One recovered coordinate: either a real dimension (more than one
/// distinct value across the group) or a scalar coordinate (exactly one),
/// which is how cfgrib reports a variable's single time/step/level.
struct Coord {
    name: String,
    axis: Axis,
}

impl Coord {
    fn is_scalar(&self) -> bool {
        self.axis.values.len() == 1
    }

    fn to_var(&self) -> VarSummary {
        let (dims, shape) = if self.is_scalar() {
            (Vec::new(), Vec::new())
        } else {
            (vec![self.name.clone()], vec![self.axis.values.len() as u64])
        };
        VarSummary {
            name: self.name.clone(),
            dtype: self.axis.dtype.to_string(),
            dims,
            shape,
            chunks: None,
            attrs: self
                .axis
                .units
                .iter()
                .map(|units| ("units".to_string(), AttrValue::Text(units.clone())))
                .collect(),
            preview: if self.axis.materialized {
                preview_of(&self.axis.values)
            } else {
                None
            },
        }
    }
}

/// Interns coordinates by name, keeping distinct value sets apart.
///
/// Two variable groups can legitimately want the same coordinate name for
/// different values — `heightAboveGround` is 2 m for a temperature field
/// and 10 m for winds — and silently sharing one would misreport both. A
/// name is reused only when the values match exactly; otherwise the second
/// claimant gets `name_2`, `name_3`, … so nothing is lost and nothing lies.
#[derive(Default)]
struct Coords {
    by_name: BTreeMap<String, Coord>,
}

impl Coords {
    fn intern(&mut self, axis: Axis) -> String {
        let wanted = signature(&axis.values);
        for suffix in 1.. {
            let name = if suffix == 1 {
                axis.base.to_string()
            } else {
                format!("{}_{suffix}", axis.base)
            };
            match self.by_name.get(&name) {
                Some(existing) if wanted == signature(&existing.axis.values) => return name,
                Some(_) => continue,
                None => {
                    self.by_name.insert(
                        name.clone(),
                        Coord {
                            name: name.clone(),
                            axis,
                        },
                    );
                    return name;
                }
            }
        }
        unreachable!("the suffix search always terminates at an unused name")
    }

    fn get(&self, name: &str) -> &Coord {
        self.by_name
            .get(name)
            .expect("coordinates are only looked up by a name this map handed out")
    }
}

/// A compact stand-in for a value list, so comparing two horizontal axes
/// doesn't mean comparing a few thousand formatted floats.
fn signature(values: &[String]) -> String {
    if values.len() <= 8 {
        values.join(",")
    } else {
        format!(
            "{}|{}|{}|{}",
            values.len(),
            values[0],
            values[1],
            values[values.len() - 1]
        )
    }
}

/// One message's metadata alongside its position in the file, which names
/// it if it ends up in the `unaligned` group.
type Numbered<'a> = (usize, &'a MessageMetadata);

/// What makes one variable: parameter, level type, statistical process —
/// the same grouping cfgrib uses.
type GroupKey = (String, String, String);

/// Splits the messages into hypercube variables and leftovers.
fn build_variables(metas: &[MessageMetadata]) -> (Vec<VarSummary>, Vec<VarSummary>) {
    // BTreeMap keeps the walk (and so every generated name) deterministic.
    let mut groups: BTreeMap<GroupKey, Vec<Numbered<'_>>> = BTreeMap::new();
    for (index, metadata) in metas.iter().enumerate() {
        let key = (
            metadata.var.clone(),
            level_type(&metadata.first_fixed_surface_type).to_string(),
            step_type(metadata),
        );
        groups.entry(key).or_default().push((index, metadata));
    }

    // A parameter appearing at more than one level type (2 m and 500 mb
    // temperature, say) needs the level type in its name to stay distinct;
    // one that appears once keeps the bare name.
    let mut base_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (parameter, _, _) in groups.keys() {
        *base_counts.entry(parameter.to_lowercase()).or_default() += 1;
    }

    let mut coords = Coords::default();
    let mut used_names: BTreeSet<String> = BTreeSet::new();
    let mut data_vars = Vec::new();
    let mut unaligned = Vec::new();

    for ((parameter, level, step), members) in &groups {
        match hypercube(members) {
            Some(axes) => {
                let name = variable_name(
                    &parameter.to_lowercase(),
                    level,
                    step,
                    base_counts[&parameter.to_lowercase()] > 1,
                    &mut used_names,
                );
                data_vars.push(hypercube_var(name, axes, members, &mut coords));
            }
            None => unaligned.extend(
                members
                    .iter()
                    .map(|(index, metadata)| unaligned_var(*index, metadata)),
            ),
        }
    }

    let mut vars: Vec<VarSummary> = coords.by_name.values().map(Coord::to_var).collect();
    vars.extend(data_vars);
    (vars, unaligned)
}

/// The recovered axes of one variable, slowest-varying first.
struct Axes {
    axes: Vec<Axis>,
    grid_type: &'static str,
    proj: String,
}

fn hypercube(members: &[Numbered<'_>]) -> Option<Axes> {
    let (_, first) = members[0];

    // Every message in a variable must sit on the same grid; a group that
    // mixes grids is not one variable, whatever its parameter says.
    if members
        .iter()
        .any(|(_, m)| m.grid_shape != first.grid_shape || m.proj != first.proj)
    {
        return None;
    }

    let times = distinct(members, |m| m.reference_date.to_string());
    let steps = distinct(members, |m| {
        format_number((m.forecast_date - m.reference_date).num_seconds() as f64 / 3600.0)
    });
    let (levels, level_units) = levels(members, &first.first_fixed_surface_type);

    // A full hypercube has exactly one message per (time, step, level)
    // cell. Anything else — a ragged level set, a duplicated field, a
    // half-downloaded file — is reported message by message instead.
    if times.len() * steps.len() * levels.len() != members.len() {
        return None;
    }

    let (ny, nx) = first.grid_shape;
    let horizontal = if first.is_regular_grid {
        let (lats, lngs) = first.projector.lat_lng();
        if lats.len() == ny && lngs.len() == nx {
            [
                degrees("latitude", "degrees_north", lats),
                degrees("longitude", "degrees_east", lngs),
            ]
        } else {
            // A regular grid whose projector disagrees with the declared
            // shape: keep the shape, drop the (untrustworthy) values.
            [placeholder("latitude", ny), placeholder("longitude", nx)]
        }
    } else {
        // Projected grids (Lambert, polar stereographic, …) have 2-D
        // latitude/longitude fields, which this model has no place for, so
        // the projected axes are named `y`/`x` as CF prescribes. Their
        // values are deliberately left unmaterialized: for a projected
        // grid `lat_lng()` inverse-projects every single cell.
        [placeholder("y", ny), placeholder("x", nx)]
    };

    let mut axes = vec![
        Axis {
            base: "time",
            dtype: "datetime64[ns]",
            values: times,
            materialized: true,
            units: None,
        },
        Axis {
            base: "step",
            dtype: "timedelta64[ns]",
            values: steps,
            materialized: true,
            units: Some("hours".to_string()),
        },
        Axis {
            base: level_type(&first.first_fixed_surface_type),
            dtype: "float64",
            values: levels,
            materialized: true,
            units: level_units,
        },
    ];
    axes.extend(horizontal);

    Some(Axes {
        axes,
        grid_type: if first.is_regular_grid {
            "regular_ll"
        } else {
            "projected"
        },
        proj: first.proj.clone(),
    })
}

/// The distinct level values in a group, plus their unit.
///
/// Isobaric levels are converted from GRIB's pascals to hectopascals so
/// that the `isobaricInhPa` name is honest and the values read the way
/// forecasters write them (500, not 50000).
fn levels(members: &[Numbered<'_>], surface: &FixedSurfaceType) -> (Vec<String>, Option<String>) {
    let isobaric = matches!(surface, FixedSurfaceType::IsobaricSurface);
    let values = distinct(members, |m| {
        m.first_fixed_surface_value.map_or_else(
            || m.first_fixed_surface_type.name().to_string(),
            |value| format_number(if isobaric { value / 100.0 } else { value }),
        )
    });

    let units = if isobaric {
        Some("hPa".to_string())
    } else {
        Some(surface.unit())
            .filter(|unit| !unit.is_empty())
            .map(str::to_string)
    };
    (values, units)
}

fn degrees(base: &'static str, units: &str, values: Vec<f64>) -> Axis {
    Axis {
        base,
        dtype: "float64",
        values: values.into_iter().map(format_number).collect(),
        materialized: true,
        units: Some(units.to_string()),
    }
}

/// An axis whose coordinate values aren't materialized: index placeholders
/// carry the length (and keep two differently-sized axes from interning to
/// the same name), and the coordinate is reported without a preview.
fn placeholder(base: &'static str, len: usize) -> Axis {
    Axis {
        base,
        dtype: "float64",
        values: (0..len).map(|i| i.to_string()).collect(),
        materialized: false,
        units: None,
    }
}

fn distinct<F: Fn(&MessageMetadata) -> String>(members: &[Numbered<'_>], value: F) -> Vec<String> {
    let set: BTreeSet<String> = members.iter().map(|(_, m)| value(m)).collect();
    set.into_iter().collect()
}

fn hypercube_var(
    name: String,
    axes: Axes,
    members: &[Numbered<'_>],
    coords: &mut Coords,
) -> VarSummary {
    let (_, first) = members[0];

    let grid_type = axes.grid_type;
    let proj = axes.proj;
    let interned: Vec<String> = axes
        .axes
        .into_iter()
        .map(|axis| coords.intern(axis))
        .collect();

    let mut dims = Vec::new();
    let mut shape = Vec::new();
    let mut scalars = Vec::new();
    for coord_name in &interned {
        let coord = coords.get(coord_name);
        if coord.is_scalar() {
            scalars.push(coord_name.clone());
        } else {
            dims.push(coord_name.clone());
            shape.push(coord.axis.values.len() as u64);
        }
    }

    let mut attrs = vec![
        ("long_name".to_string(), AttrValue::Text(first.name.clone())),
        ("units".to_string(), AttrValue::Text(first.units.clone())),
    ];
    if !scalars.is_empty() {
        // xarray's convention, which `GroupSummary::from_parts` follows:
        // this is what promotes the scalar time/step/level variables above
        // out of the data variables and into the coordinates.
        attrs.push((
            "coordinates".to_string(),
            AttrValue::Text(scalars.join(" ")),
        ));
    }
    attrs.extend([
        (
            "GRIB_shortName".to_string(),
            AttrValue::Text(first.var.clone()),
        ),
        (
            "GRIB_typeOfLevel".to_string(),
            AttrValue::Text(level_type(&first.first_fixed_surface_type).to_string()),
        ),
        (
            "GRIB_stepType".to_string(),
            AttrValue::Text(step_type(first)),
        ),
        (
            "GRIB_gridType".to_string(),
            AttrValue::Text(grid_type.to_string()),
        ),
        (
            "GRIB_discipline".to_string(),
            AttrValue::Text(first.discipline.clone()),
        ),
        (
            "GRIB_parameterCategory".to_string(),
            AttrValue::Text(first.category.clone()),
        ),
        (
            "GRIB_parameterNumber".to_string(),
            AttrValue::Int(i64::from(first.parameter_value)),
        ),
        (
            "GRIB_dataCompression".to_string(),
            AttrValue::Text(first.data_compression.clone()),
        ),
        (
            "GRIB_numberOfMessages".to_string(),
            AttrValue::Int(members.len() as i64),
        ),
        ("GRIB_proj".to_string(), AttrValue::Text(proj)),
    ]);

    VarSummary {
        name,
        dtype: "float64".to_string(),
        dims,
        shape,
        // GRIB's unit of storage is the message, not a chunk grid.
        chunks: None,
        attrs,
        preview: None,
    }
}

/// One message that didn't fit any hypercube, described on its own terms.
///
/// The horizontal dimensions carry their size in their names so that two
/// messages on different grids can share the `unaligned` group without one
/// silently redefining the other's `y`/`x`.
fn unaligned_var(index: usize, metadata: &MessageMetadata) -> VarSummary {
    let (ny, nx) = metadata.grid_shape;
    VarSummary {
        name: format!("{}_{index}", metadata.var.to_lowercase()),
        dtype: "float64".to_string(),
        dims: vec![format!("y_{ny}"), format!("x_{nx}")],
        shape: vec![ny as u64, nx as u64],
        chunks: None,
        attrs: vec![
            (
                "long_name".to_string(),
                AttrValue::Text(metadata.name.clone()),
            ),
            ("units".to_string(), AttrValue::Text(metadata.units.clone())),
            (
                "GRIB_shortName".to_string(),
                AttrValue::Text(metadata.var.clone()),
            ),
            (
                "GRIB_typeOfLevel".to_string(),
                AttrValue::Text(level_type(&metadata.first_fixed_surface_type).to_string()),
            ),
            (
                "GRIB_level".to_string(),
                AttrValue::Text(metadata.first_fixed_surface_value.map_or_else(
                    || metadata.first_fixed_surface_type.name().to_string(),
                    format_number,
                )),
            ),
            (
                "GRIB_stepType".to_string(),
                AttrValue::Text(step_type(metadata)),
            ),
            (
                "GRIB_referenceTime".to_string(),
                AttrValue::Text(metadata.reference_date.to_string()),
            ),
            (
                "GRIB_validTime".to_string(),
                AttrValue::Text(metadata.forecast_date.to_string()),
            ),
        ],
        preview: None,
    }
}

/// cfgrib's `typeOfLevel`, which also names the level dimension.
///
/// The common surfaces get ecCodes' spelling so a GRIB user recognizes
/// them; anything else falls back to `gribberish`'s own short name.
fn level_type(surface: &FixedSurfaceType) -> &'static str {
    match surface {
        FixedSurfaceType::IsobaricSurface => "isobaricInhPa",
        FixedSurfaceType::SpecifiedHeightLevelAboveGround => "heightAboveGround",
        FixedSurfaceType::SpecificAltitudeAboveMeanSeaLevel => "heightAboveSea",
        FixedSurfaceType::MeanSeaLevel => "meanSea",
        FixedSurfaceType::GroundOrWater => "surface",
        FixedSurfaceType::HybridLevel => "hybrid",
        FixedSurfaceType::SigmaLevel => "sigma",
        FixedSurfaceType::DepthBelowSeaLevel => "depthBelowSea",
        FixedSurfaceType::DepthBelowLandSurface => "depthBelowLandLayer",
        FixedSurfaceType::PotentialVorticitySurface => "potentialVorticity",
        FixedSurfaceType::EntireAtmosphereAsSingleLayer | FixedSurfaceType::EntireAtmosphere => {
            "atmosphere"
        }
        FixedSurfaceType::Tropopause => "tropopause",
        other => {
            let name = other.coordinate_name();
            if name.is_empty() { "level" } else { name }
        }
    }
}

/// cfgrib's `stepType`: the statistical process over the step range, or
/// `instant` for a field valid at a single moment.
fn step_type(metadata: &MessageMetadata) -> String {
    metadata
        .statistical_process
        .as_ref()
        .map_or_else(|| "instant".to_string(), |process| process.abbv())
}

fn variable_name(
    base: &str,
    level: &str,
    step: &str,
    qualify: bool,
    used: &mut BTreeSet<String>,
) -> String {
    let candidates = if qualify {
        vec![format!("{base}_{level}"), format!("{base}_{level}_{step}")]
    } else {
        vec![base.to_string()]
    };
    for candidate in candidates {
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    for suffix in 2.. {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("the suffix search always terminates at an unused name")
}

/// Formats a float the way the other readers' previews do: a plain decimal,
/// with `.0` appended when it would otherwise look like an integer.
fn format_number(value: f64) -> String {
    let text = format!("{value}");
    if text.contains(['.', 'e', 'E']) || text == "inf" || text == "-inf" || text == "NaN" {
        text
    } else {
        format!("{text}.0")
    }
}

/// `10.0 12.5 15.0 ... 42.0`, matching the netCDF reader's preview shape.
fn preview_of(values: &[String]) -> Option<String> {
    match values.len() {
        0 => None,
        1..=4 => Some(values.join(" ")),
        len => Some(format!("{} ... {}", values[..3].join(" "), values[len - 1])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_is_an_error_not_a_panic() {
        let error = summarize_grib_bytes(b"", Path::new("empty.grib2"))
            .expect_err("an empty file holds no messages");
        assert!(format!("{error}").contains("no GRIB messages"));
    }

    #[test]
    fn edition_is_read_from_the_indicator_section() {
        // `GRIB`, two reserved octets, the discipline, then the edition.
        assert_eq!(edition(b"GRIB\0\0\0\x02rest"), 2);
        assert_eq!(edition(b"GRIB\0\0\0\x01rest"), 1);
        assert_eq!(edition(b"nope"), 2);
    }

    #[test]
    fn preview_elides_long_value_lists() {
        let short: Vec<String> = ["1.0", "2.0"].iter().map(|s| s.to_string()).collect();
        assert_eq!(preview_of(&short), Some("1.0 2.0".to_string()));

        let long: Vec<String> = (0..10).map(|i| format_number(f64::from(i))).collect();
        assert_eq!(
            preview_of(&long),
            Some("0.0 1.0 2.0 ... 9.0".to_string()),
            "long lists show a head and the final value"
        );
    }

    /// Two variables can want the same coordinate name for different
    /// values — 2 m temperature and 10 m winds both sit on
    /// `heightAboveGround`. Sharing one would misreport a level, so the
    /// second set gets its own name.
    #[test]
    fn coordinates_with_different_values_do_not_share_a_name() {
        let height = |metres: &str| Axis {
            base: "heightAboveGround",
            dtype: "float64",
            values: vec![metres.to_string()],
            materialized: true,
            units: Some("m".to_string()),
        };

        let mut coords = Coords::default();
        let two = coords.intern(height("2.0"));
        let ten = coords.intern(height("10.0"));
        let two_again = coords.intern(height("2.0"));

        assert_eq!(two, "heightAboveGround");
        assert_eq!(ten, "heightAboveGround_2");
        assert_eq!(two_again, two, "matching values reuse the same coordinate");
    }
}
