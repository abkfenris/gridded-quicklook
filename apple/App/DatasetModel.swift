//
//  DatasetModel.swift
//  ndLook
//
//  Swift mirrors of the Rust metadata model in
//  `crates/ndlook-meta/src/model.rs`, decoded from the JSON that
//  `ndlook_summarize_json` returns. This file is deliberately
//  model-only: no FFI, no views, no formatting beyond what the detail
//  tables need to print a value.
//
//  Encoding contract (derived from the Rust side, not guessed):
//
//  - The Rust types derive serde with no `rename_all`, so JSON keys are the
//    Rust field names verbatim -- snake_case for the multi-word ones
//    (`data_vars`, `is_unlimited`, `version_info`, `ref_kind`, `wrote_at`).
//    Every type below spells those out in an explicit `CodingKeys` rather
//    than relying on `.convertFromSnakeCase`, so a grep for a JSON key
//    lands on the Swift property that reads it.
//  - `SourceFormat` is a fieldless serde enum, which serializes as a bare
//    string ("NetCdf", "ZarrV2", ...).
//  - Fields marked `#[serde(default)]` on the Rust side may be absent from
//    older JSON; the Swift decoders match that by supplying the same
//    defaults instead of throwing.
//
//  These types are `Decodable` only. Nothing in the app ever writes this
//  JSON back out -- the Rust core is the sole producer -- so adding
//  `Encodable` would only create a second, unverified spelling of the wire
//  format.
//

import Foundation

// MARK: - Source format

/// Which reader produced the summary; rendered as a format badge.
///
/// The raw values are the exact serde unit-variant spellings; the case
/// names are the Swift-idiomatic ones, so call sites read `.netCDF`
/// while the decoder still matches `"NetCdf"` on the wire.
enum SourceFormat: String, Decodable, Hashable {
    case netCDF = "NetCdf"
    case hdf5 = "Hdf5"
    case zarrV2 = "ZarrV2"
    case zarrV3 = "ZarrV3"
    case icechunk = "Icechunk"
    case grib = "Grib"

    /// Short human-readable label for the format badge.
    var displayName: String {
        switch self {
        case .netCDF: "NetCDF"
        case .hdf5: "HDF5"
        case .zarrV2: "Zarr v2"
        case .zarrV3: "Zarr v3"
        case .icechunk: "Icechunk"
        case .grib: "GRIB"
        }
    }
}

// MARK: - Attribute values

/// One attribute value, preserving the type fidelity the Rust model keeps.
///
/// The Rust enum is `#[serde(untagged)]`, so the wire form is a bare JSON
/// value with no discriminator and the decoder has to reconstruct the case
/// by trying each shape in turn. Order matters: JSON has one number type,
/// and a whole number decodes happily as either an `Int64` or a `Double`,
/// so the integer cases must be attempted first or every integer attribute
/// would come back as a float. Hence `int` before `float`, and `intList`
/// before `floatList`.
///
/// Known limitation of that ordering: a `Float` whose value happens to be
/// whole serializes as `3.0` and decodes back as `.int(3)`, so an attribute
/// like `scale_factor: 1.0` prints as `1` here where the Quick Look
/// renderer (which never round-trips through JSON) prints `1.0`. JSON has
/// no way to carry the distinction that survives decoding -- `Decimal`
/// normalizes the trailing zero away too -- so recovering it would mean
/// parsing the raw number text by hand. Cosmetic, and not worth that.
///
/// Non-finite floats are the sharp edge here. `serde_json` cannot represent
/// NaN or +/-inf and writes them as `null`, so a `Float` attribute can
/// arrive as a bare `null` and a `FloatList` can arrive as a list that
/// mixes numbers and nulls. Both are decoded back to `.nan`: the value's
/// exact non-finite flavor is unrecoverable from the JSON, and NaN is the
/// honest stand-in for "a float that could not be written".
enum AttrValue: Decodable, Hashable {
    case text(String)
    case int(Int64)
    case float(Double)
    case intList([Int64])
    case floatList([Double])
    case textList([String])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()

        // A bare `null` is serde_json's rendering of a non-finite float
        // (there is no other reason for a null to appear here), so it
        // decodes to NaN rather than failing.
        if container.decodeNil() {
            self = .float(.nan)
            return
        }

        if let value = try? container.decode(Int64.self) {
            self = .int(value)
            return
        }
        if let value = try? container.decode(Double.self) {
            self = .float(value)
            return
        }
        if let value = try? container.decode(String.self) {
            self = .text(value)
            return
        }
        if let value = try? container.decode([Int64].self) {
            self = .intList(value)
            return
        }
        // Decoded as optionals because a float list containing a NaN or an
        // infinity comes across with nulls interleaved among the numbers;
        // a plain `[Double]` decode would fail on the whole list.
        if let value = try? container.decode([Double?].self) {
            self = .floatList(value.map { $0 ?? .nan })
            return
        }
        if let value = try? container.decode([String].self) {
            self = .textList(value)
            return
        }

        throw DecodingError.dataCorruptedError(
            in: container,
            debugDescription: "Attribute value did not match any AttrValue shape."
        )
    }

    /// The value rendered the way the Rust HTML renderer's
    /// `attr_value_display` renders it, so the app's detail tables and the
    /// Quick Look preview agree on how an attribute reads.
    ///
    /// That means Python's `str()` conventions, which xarray's repr uses:
    /// scalars plain, lists comma-joined inside square brackets, strings in
    /// a list quoted (but a bare string unquoted), and floats always
    /// carrying a decimal point so a whole number prints as `1.0` rather
    /// than `1`.
    var displayString: String {
        switch self {
        case .text(let value):
            value
        case .int(let value):
            String(value)
        case .float(let value):
            Self.formatFloat(value)
        case .intList(let values):
            "[" + values.map(String.init).joined(separator: ", ") + "]"
        case .floatList(let values):
            "[" + values.map(Self.formatFloat).joined(separator: ", ") + "]"
        case .textList(let values):
            "[" + values.map { "'\($0)'" }.joined(separator: ", ") + "]"
        }
    }

    /// Matches `format_float` in `crates/ndlook-html/src/render.rs`:
    /// Python prints whole floats with a trailing `.0`, which neither
    /// Swift's nor Rust's default `Double` description does.
    ///
    /// The non-finite spellings are Rust's (`NaN`, `inf`, `-inf`), not
    /// Swift's lowercase `nan`, so the two renderers still agree on a value
    /// that reached the app by some route other than JSON.
    private static func formatFloat(_ value: Double) -> String {
        if value.isNaN {
            return "NaN"
        }
        if value.isInfinite {
            return value < 0 ? "-inf" : "inf"
        }
        if value.truncatingRemainder(dividingBy: 1) == 0 {
            return String(format: "%.1f", value)
        }
        return String(value)
    }
}

/// One `(name, value)` attribute pair.
///
/// The Rust model stores attributes as `Vec<(String, AttrValue)>` to keep
/// the reader's insertion order, and serde encodes a tuple as a JSON array
/// -- so the wire form is `[["units", "m"], ["long_name", "Depth"]]`, an
/// array of two-element arrays rather than an object. This type decodes one
/// of those inner arrays positionally.
///
/// `id` is the attribute name: names are unique within a single attribute
/// list, and SwiftUI's `ForEach` needs something stable to key rows by.
struct AttrEntry: Decodable, Identifiable, Hashable {
    let name: String
    let value: AttrValue

    var id: String { name }

    init(from decoder: Decoder) throws {
        var container = try decoder.unkeyedContainer()
        name = try container.decode(String.self)
        value = try container.decode(AttrValue.self)
    }
}

// MARK: - Structure

/// One dimension in a group's dimension list.
struct DimInfo: Decodable, Identifiable, Hashable {
    let name: String
    let size: UInt64
    /// netCDF's unlimited/appendable flag. Always `false` for formats with
    /// no such concept (Zarr, Icechunk).
    let isUnlimited: Bool

    var id: String { name }

    private enum CodingKeys: String, CodingKey {
        case name
        case size
        case isUnlimited = "is_unlimited"
    }
}

/// One variable's or array's structure.
///
/// Deliberately *not* `Identifiable`: a variable's name is unique only
/// within its own group, so two groups can each hold a `time` and the
/// sidebar would key both rows the same. Stable identity is a slash-joined
/// path (`"/model/time"`), which only the tree walk that visits the group
/// hierarchy can compose -- so it is built there, not stored here.
/// `Hashable` is enough for this file's job.
struct VarSummary: Decodable, Hashable {
    let name: String
    /// Human-readable dtype, e.g. `float32`, `int64`, `|S8`.
    let dtype: String
    let dims: [String]
    let shape: [UInt64]
    /// Absent for formats without chunking, or for unchunked arrays.
    let chunks: [UInt64]?
    let attrs: [AttrEntry]
    /// Short inline value peek for small variables, already formatted for
    /// display by the Rust reader.
    let preview: String?

    private enum CodingKeys: String, CodingKey {
        case name
        case dtype
        case dims
        case shape
        case chunks
        case attrs
        case preview
    }
}

/// One group: a netCDF-4/HDF5 group, a Zarr group, or a DataTree node.
///
/// `children` makes this a tree, so anything consuming it has to recurse.
/// Like `VarSummary`, it is `Hashable` but not `Identifiable` -- a group's
/// stable id is its slash-joined path from the root, which is a property of
/// where it sits in the tree rather than of the group itself.
struct GroupSummary: Decodable, Hashable {
    /// Group name; the empty string for the root group.
    let name: String
    let dims: [DimInfo]
    /// Variables classified as coordinates by xarray's heuristic (name
    /// matches one of its own dims, or it is named in a sibling's
    /// `coordinates` attribute).
    let coords: [VarSummary]
    let dataVars: [VarSummary]
    let attrs: [AttrEntry]
    let children: [GroupSummary]

    private enum CodingKeys: String, CodingKey {
        case name
        case dims
        case coords
        case dataVars = "data_vars"
        case attrs
        case children
    }
}

// MARK: - Version history

/// One snapshot in a version-controlled store's history.
struct SnapshotInfo: Decodable, Identifiable, Hashable {
    let id: String
    let message: String?
    /// RFC 3339 timestamp.
    let wroteAt: String?

    private enum CodingKeys: String, CodingKey {
        case id
        case message
        case wroteAt = "wrote_at"
    }
}

/// Version metadata for an Icechunk repo, scoped to whichever ref was
/// previewed.
struct VersionInfo: Decodable, Hashable {
    /// Display name of the previewed ref: a branch name, a tag name, or a
    /// snapshot id, depending on `refKind`. The Rust field kept the name
    /// `branch` for backwards compatibility with callers that only ever
    /// previewed `main`.
    let branch: String
    /// `"branch"`, `"tag"`, or `"snapshot"`, labeling what `branch` names.
    /// Absent in JSON produced before the field existed -- treat `nil` as
    /// "assume branch".
    let refKind: String?
    /// Every branch in the repo, sorted by name.
    let branches: [String]
    /// Every tag in the repo, sorted by name.
    let tags: [String]
    /// Newest first; the tip snapshot is `ancestry[0]`.
    let ancestry: [SnapshotInfo]
    /// `true` if the ancestry walk was capped before reaching the repo's
    /// initial snapshot.
    let truncated: Bool

    private enum CodingKeys: String, CodingKey {
        case branch
        case refKind = "ref_kind"
        case branches
        case tags
        case ancestry
        case truncated
    }

    /// Memberwise initializer, written out by hand.
    ///
    /// Declaring `init(from:)` below suppresses the one Swift would
    /// otherwise synthesize, and tests need to build `VersionInfo` values
    /// directly rather than round-tripping every fixture through JSON.
    init(
        branch: String,
        refKind: String?,
        branches: [String],
        tags: [String],
        ancestry: [SnapshotInfo],
        truncated: Bool
    ) {
        self.branch = branch
        self.refKind = refKind
        self.branches = branches
        self.tags = tags
        self.ancestry = ancestry
        self.truncated = truncated
    }

    /// Hand-written so `branches` and `tags` can default to empty.
    ///
    /// Both are `#[serde(default)]` in Rust (they were added after the type
    /// shipped, and older snapshot fixtures omit them). Swift's synthesized
    /// decoder would throw `keyNotFound` on their absence, so the two
    /// optional-key reads below are what keep that older JSON decodable.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        branch = try container.decode(String.self, forKey: .branch)
        refKind = try container.decodeIfPresent(String.self, forKey: .refKind)
        branches = try container.decodeIfPresent([String].self, forKey: .branches) ?? []
        tags = try container.decodeIfPresent([String].self, forKey: .tags) ?? []
        ancestry = try container.decode([SnapshotInfo].self, forKey: .ancestry)
        truncated = try container.decode(Bool.self, forKey: .truncated)
    }
}

// MARK: - Top level

/// Top-level summary of one dataset, store, or repo -- the payload of
/// `ndlook_summarize_json`'s `{"summary": ...}` envelope.
struct DatasetSummary: Decodable, Hashable {
    let format: SourceFormat
    let root: GroupSummary
    /// Present only for version-controlled stores (Icechunk).
    let versionInfo: VersionInfo?

    private enum CodingKeys: String, CodingKey {
        case format
        case root
        case versionInfo = "version_info"
    }
}
