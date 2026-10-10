//
//  DatasetModelTests.swift
//  NDLookTests
//
//  Covers the JSON wire contract with the Rust core: the externally tagged
//  `AttrValue` encoding, its string spellings for non-finite floats, and the
//  display formatting that has to agree with the Quick Look renderer.
//

import XCTest

private func decodeAttr(_ json: String) throws -> AttrValue {
    try JSONDecoder().decode(AttrValue.self, from: Data(json.utf8))
}

// MARK: - AttrValue wire format

final class AttrValueDecodingTests: XCTestCase {

    func testScalarVariants() throws {
        XCTAssertEqual(try decodeAttr(#"{"Text":"degC"}"#), .text("degC"))
        XCTAssertEqual(try decodeAttr(#"{"Int":3}"#), .int(3))
        XCTAssertEqual(try decodeAttr(#"{"Float":1.5}"#), .float(1.5))
    }

    func testListVariants() throws {
        XCTAssertEqual(try decodeAttr(#"{"IntList":[1,2,3]}"#), .intList([1, 2, 3]))
        XCTAssertEqual(try decodeAttr(#"{"FloatList":[1.5,2.5]}"#), .floatList([1.5, 2.5]))
        XCTAssertEqual(try decodeAttr(#"{"TextList":["a","b"]}"#), .textList(["a", "b"]))
    }

    /// The reason the wire format is tagged at all.
    ///
    /// Untagged, `{"Float":1.0}` was written as a bare `1.0`, and a decoder
    /// guessing from JSON's single number type brought it back as an integer.
    /// The tag settles it.
    func testWholeFloatStaysAFloat() throws {
        XCTAssertEqual(try decodeAttr(#"{"Float":1.0}"#), .float(1.0))
        XCTAssertEqual(try decodeAttr(#"{"Float":1}"#), .float(1.0), "a trimmed .0 still decodes")
        XCTAssertEqual(try decodeAttr(#"{"Int":1}"#), .int(1), "and an integer stays an integer")
    }

    /// A one-element list is likewise indistinguishable from a scalar
    /// without the tag.
    func testSingleElementListIsNotAScalar() throws {
        XCTAssertEqual(try decodeAttr(#"{"IntList":[7]}"#), .intList([7]))
        XCTAssertEqual(try decodeAttr(#"{"Int":7}"#), .int(7))
    }

    // MARK: Non-finite floats

    /// `_FillValue` is NaN on nearly every CF dataset, so this is the common
    /// path, not an exotic one.
    func testNonFiniteScalarsArriveAsStrings() throws {
        guard case .float(let nan) = try decodeAttr(#"{"Float":"NaN"}"#) else {
            return XCTFail("expected a float")
        }
        XCTAssertTrue(nan.isNaN)

        XCTAssertEqual(try decodeAttr(#"{"Float":"inf"}"#), .float(.infinity))
        XCTAssertEqual(try decodeAttr(#"{"Float":"-inf"}"#), .float(-.infinity))
    }

    /// The sign of an infinity has to survive. Under the old encoding every
    /// non-finite value flattened to `null` and came back as NaN, losing it.
    func testNegativeInfinityKeepsItsSign() throws {
        guard case .float(let value) = try decodeAttr(#"{"Float":"-inf"}"#) else {
            return XCTFail("expected a float")
        }
        XCTAssertTrue(value.isInfinite)
        XCTAssertLessThan(value, 0)
    }

    /// A valid-range or fill-value list is exactly where these turn up.
    func testFloatListMixesNumbersAndNonFiniteStrings() throws {
        guard case .floatList(let values) = try decodeAttr(#"{"FloatList":[1.5,"NaN","-inf",2.5]}"#)
        else {
            return XCTFail("expected a float list")
        }
        XCTAssertEqual(values.count, 4)
        XCTAssertEqual(values[0], 1.5)
        XCTAssertTrue(values[1].isNaN)
        XCTAssertEqual(values[2], -.infinity)
        XCTAssertEqual(values[3], 2.5)
    }

    /// The Rust side parses non-finite spellings with Rust's own float
    /// parser, so other producers' spellings decode too.
    func testAlternativeNonFiniteSpellings() throws {
        XCTAssertEqual(try decodeAttr(#"{"Float":"Infinity"}"#), .float(.infinity))
        XCTAssertEqual(try decodeAttr(#"{"Float":"-Infinity"}"#), .float(-.infinity))

        guard case .float(let lower) = try decodeAttr(#"{"Float":"nan"}"#) else {
            return XCTFail("expected a float")
        }
        XCTAssertTrue(lower.isNaN)
    }

    // MARK: Malformed input

    func testUnknownOrMultipleTagsAreRejected() {
        XCTAssertThrowsError(try decodeAttr(#"{"Nope":1}"#), "unknown variant tag")
        XCTAssertThrowsError(try decodeAttr(#"{"Int":1,"Text":"x"}"#), "two tags is not a variant")
        XCTAssertThrowsError(try decodeAttr(#"{}"#), "no tag at all")
        XCTAssertThrowsError(try decodeAttr("1.0"), "the untagged form is no longer accepted")
        XCTAssertThrowsError(try decodeAttr(#"{"Float":"banana"}"#), "unparsable float string")
    }
}

// MARK: - Attribute pairs

final class AttrEntryDecodingTests: XCTestCase {

    /// Attributes are `Vec<(String, AttrValue)>` in Rust, and serde writes a
    /// tuple as a JSON array -- so the pair is positional, not an object.
    func testAttributePairsDecodePositionally() throws {
        let json = #"[["units",{"Text":"m"}],["_FillValue",{"Float":"NaN"}]]"#
        let entries = try JSONDecoder().decode([AttrEntry].self, from: Data(json.utf8))

        XCTAssertEqual(entries.count, 2)
        XCTAssertEqual(entries[0].name, "units")
        XCTAssertEqual(entries[0].value, .text("m"))
        XCTAssertEqual(entries[1].name, "_FillValue")
        guard case .float(let fill) = entries[1].value else { return XCTFail("expected a float") }
        XCTAssertTrue(fill.isNaN)
    }
}

// MARK: - Display formatting

/// `displayString` has to match `attr_value_display` in
/// `crates/ndlook-html/src/render.rs`, so the app window and the Quick Look
/// preview describe the same attribute identically.
final class AttrValueDisplayTests: XCTestCase {

    func testScalarsMatchTheRustRenderer() {
        XCTAssertEqual(AttrValue.text("degC").displayString, "degC")
        XCTAssertEqual(AttrValue.int(3).displayString, "3")
        XCTAssertEqual(AttrValue.float(3.5).displayString, "3.5")
    }

    /// Python prints whole floats with a trailing `.0`, and Rust's renderer
    /// follows suit via `{:.1}`.
    func testWholeFloatsKeepTheirDecimalPoint() {
        XCTAssertEqual(AttrValue.float(1).displayString, "1.0")
        XCTAssertEqual(AttrValue.float(-999).displayString, "-999.0")
        XCTAssertEqual(AttrValue.float(0).displayString, "0.0")
    }

    func testListsAreBracketedAndCommaJoined() {
        XCTAssertEqual(AttrValue.intList([1, 2, 3]).displayString, "[1, 2, 3]")
        XCTAssertEqual(AttrValue.floatList([1.5, 2]).displayString, "[1.5, 2.0]")
        XCTAssertEqual(AttrValue.textList(["a", "b"]).displayString, "['a', 'b']")
        XCTAssertEqual(AttrValue.intList([]).displayString, "[]")
    }

    func testNonFiniteUsesRustSpellings() {
        XCTAssertEqual(AttrValue.float(.nan).displayString, "NaN")
        XCTAssertEqual(AttrValue.float(.infinity).displayString, "inf")
        XCTAssertEqual(AttrValue.float(-.infinity).displayString, "-inf")
        XCTAssertEqual(
            AttrValue.floatList([1.5, .nan, -.infinity]).displayString,
            "[1.5, NaN, -inf]"
        )
    }

    /// Rust's `Display` for `f64` never uses exponent notation; Swift's
    /// `String(_:)` does. Left alone, a small `scale_factor` printed as
    /// `1e-07` in the app and `0.0000001` in the preview.
    func testSmallMagnitudesAvoidExponentNotation() {
        XCTAssertEqual(AttrValue.float(1e-7).displayString, "0.0000001")
        XCTAssertEqual(AttrValue.float(-1e-7).displayString, "-0.0000001")
        XCTAssertEqual(AttrValue.float(1.5e-7).displayString, "0.00000015")
        XCTAssertEqual(AttrValue.float(2.5e-10).displayString, "0.00000000025")
    }

    /// Whole values take the `%.1f` path, which also never uses exponents.
    func testLargeMagnitudesAvoidExponentNotation() {
        XCTAssertEqual(AttrValue.float(1e20).displayString, "100000000000000000000.0")
        XCTAssertFalse(AttrValue.float(1e300).displayString.contains("e"))
    }

    /// No value of any magnitude may reach the display with an exponent in
    /// it -- that is the property the renderers have to share.
    func testNoDisplayedFloatContainsAnExponent() {
        let values: [Double] = [
            0, 1, -1, 0.5, 1e-1, 1e-5, 1e-7, 1e-15, 1.5e-8,
            1e15, 1e16, 1e20, 1e300, -1e-7, -1e20, .pi,
        ]
        for value in values {
            let shown = AttrValue.float(value).displayString
            XCTAssertFalse(
                shown.lowercased().contains("e"),
                "\(value) displayed as \(shown), which uses exponent notation"
            )
        }
    }

    /// The expansion must not change the value, only its spelling.
    func testExpandedTextStillParsesBackToTheSameValue() {
        let values: [Double] = [1e-7, 1.5e-7, 2.5e-10, 0.5, 1e-15, -1e-7, .pi]
        for value in values {
            let shown = AttrValue.formatFloat(value)
            XCTAssertEqual(Double(shown), value, "\(shown) did not round-trip")
        }
    }

    func testTextWithoutAnExponentIsLeftAlone() {
        XCTAssertEqual(AttrValue.expandingExponent("3.5"), "3.5")
        XCTAssertEqual(AttrValue.expandingExponent("-42"), "-42")
        XCTAssertEqual(AttrValue.expandingExponent(""), "")
    }
}

// MARK: - Format badge

final class SourceFormatTests: XCTestCase {

    /// Spelled as `format_badge` in `crates/ndlook-html/src/lib.rs` spells
    /// them -- including the lowercase `n` in `netCDF`.
    func testDisplayNamesMatchTheRustBadges() {
        XCTAssertEqual(SourceFormat.netCDF.displayName, "netCDF")
        XCTAssertEqual(SourceFormat.hdf5.displayName, "HDF5")
        XCTAssertEqual(SourceFormat.zarrV2.displayName, "Zarr v2")
        XCTAssertEqual(SourceFormat.zarrV3.displayName, "Zarr v3")
        XCTAssertEqual(SourceFormat.icechunk.displayName, "Icechunk")
        XCTAssertEqual(SourceFormat.grib.displayName, "GRIB")
    }

    /// The raw values are the serde unit-variant spellings.
    func testDecodesFromSerdeVariantNames() throws {
        let decoded = try JSONDecoder().decode(
            [SourceFormat].self,
            from: Data(#"["NetCdf","Hdf5","ZarrV2","ZarrV3","Icechunk","Grib"]"#.utf8)
        )
        XCTAssertEqual(decoded, [.netCDF, .hdf5, .zarrV2, .zarrV3, .icechunk, .grib])
    }
}

// MARK: - VersionInfo

final class VersionInfoDecodingTests: XCTestCase {

    /// `branches` and `tags` are `#[serde(default)]` in Rust and are only
    /// populated via `ndlook_summarize_json`, so JSON without them must
    /// still decode.
    func testMissingBranchesAndTagsDefaultToEmpty() throws {
        let json = """
            {"branch":"main","ancestry":[{"id":"ABC"}],"truncated":false}
            """
        let info = try JSONDecoder().decode(VersionInfo.self, from: Data(json.utf8))

        XCTAssertEqual(info.branch, "main")
        XCTAssertNil(info.refKind)
        XCTAssertEqual(info.branches, [])
        XCTAssertEqual(info.tags, [])
        XCTAssertEqual(info.ancestry.count, 1)
    }

    /// The memberwise initializer has to survive the custom decoder living
    /// in an extension -- tests build these values directly.
    func testMemberwiseInitializerExists() {
        let info = VersionInfo(
            branch: "main",
            refKind: "branch",
            branches: ["main"],
            tags: ["v1"],
            ancestry: [],
            truncated: false
        )
        XCTAssertEqual(info.branch, "main")
        XCTAssertEqual(info.tags, ["v1"])
    }

    /// For a `snapshot:` ref the Rust side now reports the canonical
    /// uppercase id in `branch`, matching the ids in `ancestry`.
    func testSnapshotRefBranchMatchesAncestryIds() throws {
        let json = """
            {"branch":"BVP8CH0SP7HHKXN4XFDG","ref_kind":"snapshot",
             "ancestry":[{"id":"BVP8CH0SP7HHKXN4XFDG"}],"truncated":false}
            """
        let info = try JSONDecoder().decode(VersionInfo.self, from: Data(json.utf8))

        XCTAssertEqual(info.refKind, "snapshot")
        XCTAssertEqual(info.branch, info.ancestry.first?.id)
    }
}
