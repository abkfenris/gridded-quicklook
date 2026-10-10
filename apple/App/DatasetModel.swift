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
    ///
    /// Spelled exactly as `format_badge` in `crates/ndlook-html/src/lib.rs`
    /// spells it, so the app window and the Quick Look preview label the same
    /// file identically -- note the lowercase `n` in `netCDF`, which is the
    /// project's house spelling.
    var displayName: String {
        switch self {
        case .netCDF: "netCDF"
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
/// # Wire format
///
/// Externally tagged, matching the `AttrValue` enum in
/// `crates/ndlook-meta/src/model.rs`: every value is a single-key object
/// naming its variant -- `{"Text":"degC"}`, `{"Int":3}`, `{"Float":1.0}`,
/// `{"IntList":[1,2]}`, `{"FloatList":[1.0,2.0]}`,
/// `{"TextList":["a","b"]}`.
///
/// The tag is what makes this decodable at all. The wire form used to be
/// serde's *untagged* representation -- a bare JSON value -- which left a
/// decoder guessing from shape alone, in some arbitrary order. JSON has one
/// number type, so `{"Float":1.0}` written untagged as `1.0` came back as
/// `.int(1)` here, and a one-element list was indistinguishable from a
/// scalar to a decoder that ordered its arms differently. Reading the tag
/// removes the guesswork: `1.0` stays a float.
///
/// Non-finite floats arrive as *strings* -- `{"Float":"NaN"}`,
/// `{"Float":"inf"}`, `{"Float":"-inf"}`, and the same spellings for
/// entries inside a `FloatList` -- because JSON has no literal for them.
/// This is the common case rather than an exotic one: `_FillValue` is NaN
/// on nearly every CF dataset. Finite floats stay JSON numbers, and both
/// spellings are accepted on the way in.
enum AttrValue: Decodable, Hashable {
    case text(String)
    case int(Int64)
    case float(Double)
    case intList([Int64])
    case floatList([Double])
    case textList([String])

    /// The variant tags, spelled exactly as serde writes the Rust enum's
    /// case names.
    private enum CodingKeys: String, CodingKey {
        case text = "Text"
        case int = "Int"
        case float = "Float"
        case intList = "IntList"
        case floatList = "FloatList"
        case textList = "TextList"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        // Exactly one key: more than one is not an `AttrValue` serde would
        // ever have written, and silently taking the first would hide a
        // real drift between the two models.
        guard container.allKeys.count == 1, let key = container.allKeys.first else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: """
                        Expected a single-key AttrValue object, found keys: \
                        \(container.allKeys.map(\.stringValue).sorted())
                        """
                )
            )
        }

        switch key {
        case .text:
            self = .text(try container.decode(String.self, forKey: .text))
        case .int:
            self = .int(try container.decode(Int64.self, forKey: .int))
        case .float:
            self = .float(try container.decode(WireFloat.self, forKey: .float).value)
        case .intList:
            self = .intList(try container.decode([Int64].self, forKey: .intList))
        case .floatList:
            self = .floatList(
                try container.decode([WireFloat].self, forKey: .floatList).map(\.value)
            )
        case .textList:
            self = .textList(try container.decode([String].self, forKey: .textList))
        }
    }

    /// One float as the wire carries it: a JSON number when finite, a string
    /// when not.
    ///
    /// Mirrors the `WireFloat` glue on the Rust side, including its
    /// tolerance on the way in -- the parse accepts any spelling Swift's
    /// `Double` initializer understands (`inf`, `Infinity`, `nan`, any
    /// letter case), not just the three canonical ones we emit.
    private struct WireFloat: Decodable {
        let value: Double

        init(from decoder: Decoder) throws {
            let container = try decoder.singleValueContainer()

            if let number = try? container.decode(Double.self) {
                value = number
                return
            }

            let text = try container.decode(String.self)
            guard let parsed = Double(text) else {
                throw DecodingError.dataCorruptedError(
                    in: container,
                    debugDescription: "\(text) is not a number or a non-finite float spelling."
                )
            }
            value = parsed
        }
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
            "[" + values.map { "\'\($0)\'" }.joined(separator: ", ") + "]"
        }
    }

    /// Matches `format_float` in `crates/ndlook-html/src/render.rs`, which
    /// is `{:.1}` for whole floats and Rust's `Display` otherwise.
    ///
    /// Whole floats print with a trailing `.0`, which neither Swift's nor
    /// Rust's bare `Double` description does; `%.1f` covers that and, unlike
    /// Swift's `String(_:)`, never switches to exponent form for large
    /// magnitudes.
    ///
    /// The non-whole branch is where the two languages genuinely differ.
    /// Rust's `Display` for `f64` never uses exponent notation -- `1e-7`
    /// prints as `0.0000001` -- while Swift's `String(_:)` switches to
    /// `1e-07`. Both produce the same shortest round-tripping *digits*, so
    /// the fix is to take Swift's digits and write them out positionally
    /// rather than to reimplement float formatting.
    ///
    /// The non-finite spellings are Rust's (`NaN`, `inf`, `-inf`), not
    /// Swift's lowercase `nan`.
    static func formatFloat(_ value: Double) -> String {
        if value.isNaN {
            return "NaN"
        }
        if value.isInfinite {
            return value < 0 ? "-inf" : "inf"
        }
        if value.truncatingRemainder(dividingBy: 1) == 0 {
            return String(format: "%.1f", value)
        }
        return expandingExponent(String(value))
    }

    /// Rewrites `1e-07` as `0.0000001`, leaving text without an exponent
    /// alone.
    ///
    /// Shifts the decimal point through the digit string by the exponent,
    /// padding with zeros on whichever side runs out. Anything that does not
    /// parse is returned unchanged -- a wrong-looking number beats a crash.
    static func expandingExponent(_ text: String) -> String {
        guard let marker = text.firstIndex(where: { $0 == "e" || $0 == "E" }),
              let exponent = Int(text[text.index(after: marker)...])
        else {
            return text
        }

        var mantissa = String(text[text.startIndex..<marker])
        var sign = ""
        if mantissa.hasPrefix("-") {
            sign = "-"
            mantissa.removeFirst()
        } else if mantissa.hasPrefix("+") {
            mantissa.removeFirst()
        }

        var whole = mantissa
        var fraction = ""
        if let dot = mantissa.firstIndex(of: ".") {
            whole = String(mantissa[mantissa.startIndex..<dot])
            fraction = String(mantissa[mantissa.index(after: dot)...])
        }

        // Every digit, with the point's position tracked as an offset into
        // them rather than as a character.
        var digits = whole + fraction
        var point = whole.count + exponent

        // A point at or left of the start needs leading zeros to sit after;
        // one past the end needs trailing zeros to sit before.
        if point < 1 {
            digits = String(repeating: "0", count: 1 - point) + digits
            point = 1
        }
        if point > digits.count {
            digits += String(repeating: "0", count: point - digits.count)
        }

        let split = digits.index(digits.startIndex, offsetBy: point)
        let integerPart = String(digits[digits.startIndex..<split])
        let fractionPart = String(digits[split...])

        return fractionPart.isEmpty ? "\(sign)\(integerPart)" : "\(sign)\(integerPart).\(fractionPart)"
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

    fileprivate enum CodingKeys: String, CodingKey {
        case branch
        case refKind = "ref_kind"
        case branches
        case tags
        case ancestry
        case truncated
    }
}

/// The decoder lives in an extension so the struct above keeps its
/// *synthesized* memberwise initializer: declaring any `init` in the type's
/// own body suppresses it, which previously meant hand-writing a memberwise
/// initializer that had to be kept in step with the stored properties by
/// hand. Tests build `VersionInfo` values directly, so that initializer has
/// to exist -- this just lets the compiler keep writing it.
extension VersionInfo {
    /// Hand-written so `branches` and `tags` can default to empty.
    ///
    /// Both are `#[serde(default)]` in Rust (they were added after the type
    /// shipped, and older snapshot fixtures omit them). Swift's synthesized
    /// decoder would throw `keyNotFound` on their absence, so the two
    /// optional-key reads below are what keep that older JSON decodable.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            branch: try container.decode(String.self, forKey: .branch),
            refKind: try container.decodeIfPresent(String.self, forKey: .refKind),
            branches: try container.decodeIfPresent([String].self, forKey: .branches) ?? [],
            tags: try container.decodeIfPresent([String].self, forKey: .tags) ?? [],
            ancestry: try container.decode([SnapshotInfo].self, forKey: .ancestry),
            truncated: try container.decode(Bool.self, forKey: .truncated)
        )
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
