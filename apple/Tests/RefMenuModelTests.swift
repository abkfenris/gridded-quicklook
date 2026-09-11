//
//  RefMenuModelTests.swift
//  NDLookTests
//
//  Covers the two bugs that showed up while switching refs in the running
//  app, plus the ref-string round trip they both depend on.
//
//  These are pure-logic tests: `RefMenuModel` and the model types it reads
//  are compiled straight into this bundle (see apple/project.yml), so no
//  app has to launch and no Rust library has to load.
//

import XCTest

// MARK: - Fixtures

/// The Icechunk fixture's real shape: three snapshots, one branch, one tag.
/// Matches `fixtures/data/icechunk_repo.icechunk` as the reader reports it.
private func snapshots(_ count: Int) -> [SnapshotInfo] {
    let all = [
        SnapshotInfo(
            id: "BVP8CH0SP7HHKXN4XFDG",
            message: "update global attrs",
            wroteAt: "2026-09-05T23:45:49.906701+00:00"
        ),
        SnapshotInfo(
            id: "YF19J4V0GM66MT2N8670",
            message: "initial data",
            wroteAt: "2026-09-05T23:45:49.892195+00:00"
        ),
        SnapshotInfo(
            id: "1CECHNKREP0F1RSTCMT0",
            message: "Repository initialized",
            wroteAt: "2026-09-05T23:45:49.862079+00:00"
        ),
    ]
    return Array(all.prefix(count))
}

/// The default load: viewing `main`, full three-snapshot ancestry.
private func defaultLoad() -> VersionInfo {
    VersionInfo(
        branch: "main",
        refKind: "branch",
        branches: ["main"],
        tags: ["v1"],
        ancestry: snapshots(3),
        truncated: false
    )
}

/// What the reader returns after switching to `tag:v1`, which points at the
/// second snapshot: the ancestry walk starts from *there*, so it is shorter.
/// This is the shape that made the menu shrink.
private func tagLoad() -> VersionInfo {
    VersionInfo(
        branch: "v1",
        refKind: "tag",
        branches: ["main"],
        tags: ["v1"],
        ancestry: snapshots(2),
        truncated: false
    )
}

private func snapshotLoad(id: String) -> VersionInfo {
    VersionInfo(
        branch: id,
        refKind: "snapshot",
        branches: ["main"],
        tags: ["v1"],
        ancestry: snapshots(1),
        truncated: false
    )
}

// MARK: - Menu stability (bug A)

final class RefMenuStabilityTests: XCTestCase {

    /// The regression test for the reported bug: after switching to a ref
    /// with a shorter ancestry, the menu must still offer every snapshot it
    /// offered before. Driving the menu from the live `VersionInfo` made
    /// these options disappear, stranding the user on an old ref.
    func testMenuRowsSurviveSwitchToShorterAncestry() {
        let pinned = defaultLoad()
        let before = RefMenuModel(pinned: pinned, current: pinned)
        let after = RefMenuModel(pinned: pinned, current: tagLoad())

        XCTAssertEqual(
            before.sections.map(\.title),
            after.sections.map(\.title),
            "sections must not come and go as the viewed ref changes"
        )
        XCTAssertEqual(
            before.sections.map { $0.rows.map(\.ref) },
            after.sections.map { $0.rows.map(\.ref) },
            "every ref offered before the switch must still be offered after it"
        )
        XCTAssertEqual(
            before.sections.map { $0.rows.map(\.label) },
            after.sections.map { $0.rows.map(\.label) },
            "row labels are part of the menu's stability, not just its refs"
        )
    }

    /// Even navigating all the way to a single-snapshot view must not shrink
    /// the menu -- that is the state with no way back if the contents track
    /// the live value.
    func testMenuRowsSurviveSwitchToSingleSnapshot() {
        let pinned = defaultLoad()
        let deepest = snapshotLoad(id: "1CECHNKREP0F1RSTCMT0")
        let menu = RefMenuModel(pinned: pinned, current: deepest)

        let snapshotRows = menu.sections.first { $0.title == "Snapshots" }?.rows ?? []
        XCTAssertEqual(snapshotRows.count, 3, "all three snapshots stay reachable")
        XCTAssertEqual(
            menu.sections.first { $0.title == "Branches" }?.rows.map(\.name),
            ["main"],
            "the branch you came from stays reachable"
        )
    }

    /// The one thing that *should* differ between the two loads is which row
    /// is checked -- the contents are pinned, the highlight is live.
    func testOnlyCheckmarkDiffersAcrossSwitch() {
        let pinned = defaultLoad()
        let before = RefMenuModel(pinned: pinned, current: pinned)
        let after = RefMenuModel(pinned: pinned, current: tagLoad())

        XCTAssertNotEqual(before, after, "the checkmark must move")
        XCTAssertEqual(
            before.sections.map { $0.rows.map(\.id) },
            after.sections.map { $0.rows.map(\.id) },
            "row identity must be stable so SwiftUI does not rebuild the menu"
        )
    }

    /// Guards the guard.
    ///
    /// Proves these fixtures actually reproduce the reported bug, so
    /// `testMenuRowsSurviveSwitchToShorterAncestry` is not passing
    /// vacuously. Passing the live `VersionInfo` as *both* arguments is
    /// precisely what the original view-embedded code did; it must produce
    /// the shrunken menu, while the pinned form must not.
    func testDrivingContentsFromLiveInfoWouldShrinkTheMenu() {
        let live = tagLoad()
        let buggy = RefMenuModel(pinned: live, current: live)
        let fixed = RefMenuModel(pinned: defaultLoad(), current: live)

        func snapshotCount(_ menu: RefMenuModel) -> Int? {
            menu.sections.first { $0.title == "Snapshots" }?.rows.count
        }

        XCTAssertEqual(snapshotCount(buggy), 2, "the bug: history seen from the tag only")
        XCTAssertEqual(snapshotCount(fixed), 3, "the fix: the repository's full history")
    }

    /// The truncation note belongs to the pinned ancestry, so it must not
    /// flicker on and off as refs change either.
    func testTruncationNoteFollowsPinnedHistory() {
        var pinned = defaultLoad()
        pinned = VersionInfo(
            branch: pinned.branch,
            refKind: pinned.refKind,
            branches: pinned.branches,
            tags: pinned.tags,
            ancestry: pinned.ancestry,
            truncated: true
        )

        let menu = RefMenuModel(pinned: pinned, current: tagLoad())
        let snapshotSection = menu.sections.first { $0.title == "Snapshots" }
        XCTAssertEqual(snapshotSection?.showsTruncationNote, true)
    }
}

// MARK: - Checkmark and label

final class RefMenuSelectionTests: XCTestCase {

    private func currentRow(_ menu: RefMenuModel) -> RefMenuModel.Row? {
        menu.sections.flatMap(\.rows).first { $0.isCurrent }
    }

    /// The default (`selectedRef == nil`) case. The picker has no selection
    /// of its own yet, so the checkmark has to be derived from what the
    /// summary says the default resolved to.
    func testDefaultLoadChecksTheDefaultBranch() {
        let pinned = defaultLoad()
        let menu = RefMenuModel(pinned: pinned, current: pinned)

        XCTAssertEqual(currentRow(menu)?.ref, "branch:main")
        XCTAssertEqual(menu.currentKind, .branch)
        XCTAssertEqual(menu.currentLabel, "main")
    }

    func testTagIsChecked() {
        let menu = RefMenuModel(pinned: defaultLoad(), current: tagLoad())

        XCTAssertEqual(currentRow(menu)?.ref, "tag:v1")
        XCTAssertEqual(menu.currentKind, .tag)
        XCTAssertEqual(menu.currentLabel, "v1")
    }

    func testSnapshotIsCheckedAndLabelIsAbbreviated() {
        let id = "YF19J4V0GM66MT2N8670"
        let menu = RefMenuModel(pinned: defaultLoad(), current: snapshotLoad(id: id))

        XCTAssertEqual(currentRow(menu)?.ref, "snapshot:\(id)")
        XCTAssertEqual(menu.currentKind, .snapshot)
        XCTAssertEqual(menu.currentLabel, "YF19J4V0", "a 20-char id would crowd the titlebar")
    }

    /// Exactly one row is ever checked. Name-only matching would tick both
    /// the branch and its tip snapshot when those happen to coincide.
    func testExactlyOneRowIsCheckedWhenViewingABranch() {
        let pinned = defaultLoad()
        let menu = RefMenuModel(pinned: pinned, current: pinned)

        let checked = menu.sections.flatMap(\.rows).filter(\.isCurrent)
        XCTAssertEqual(checked.count, 1)
        XCTAssertEqual(checked.first?.kind, .branch, "not the tip snapshot")
    }

    /// `refKind` is absent in JSON written before the field existed; a
    /// branch was the only possibility then.
    func testMissingRefKindIsTreatedAsBranch() {
        let legacy = VersionInfo(
            branch: "main",
            refKind: nil,
            branches: ["main"],
            tags: [],
            ancestry: snapshots(1),
            truncated: false
        )
        let menu = RefMenuModel(pinned: legacy, current: legacy)

        XCTAssertEqual(menu.currentKind, .branch)
        XCTAssertEqual(currentRow(menu)?.ref, "branch:main")
    }

    func testEmptySectionsAreOmitted() {
        let noTags = VersionInfo(
            branch: "main",
            refKind: "branch",
            branches: ["main"],
            tags: [],
            ancestry: snapshots(1),
            truncated: false
        )
        let menu = RefMenuModel(pinned: noTags, current: noTags)

        XCTAssertEqual(menu.sections.map(\.title), ["Branches", "Snapshots"])
    }

    /// Snapshot rows carry the abbreviated id and the message; the date is
    /// locale-dependent, so it is checked for presence via the separator
    /// rather than by exact text.
    func testSnapshotRowLabelCarriesIdAndMessage() {
        let pinned = defaultLoad()
        let menu = RefMenuModel(pinned: pinned, current: pinned)
        let row = menu.sections.first { $0.title == "Snapshots" }?.rows.first

        let label = row?.label
        XCTAssertEqual(label?.hasPrefix("BVP8CH0S"), true)
        XCTAssertEqual(label?.contains("update global attrs"), true)
        XCTAssertEqual(label?.contains(" \u{00B7} "), true, "id, message and date are separated")
    }
}

// MARK: - Control label width

/// The control's label is what the toolbar measures when deciding whether
/// the item fits; an unbounded one gets the whole picker swept into the
/// overflow menu. These are the data-level guard for that regression --
/// view-level truncation cannot be asserted on, but the string can.
final class RefControlLabelTests: XCTestCase {

    /// Read from the model rather than restated, so tightening the cap is a
    /// one-line change that these tests follow automatically.
    private let limit = RefMenuModel.controlLabelLimit

    func testBranchAndTagNamesAreShownWhole() {
        XCTAssertEqual(RefMenuModel.controlLabel(kind: .branch, name: "main"), "main")
        XCTAssertEqual(RefMenuModel.controlLabel(kind: .tag, name: "v1"), "v1")
    }

    /// The 20-character base32 id collapses to the same 8-character prefix
    /// the menu rows use.
    func testSnapshotIdCollapsesToShortForm() {
        let id = "YF19J4V0GM66MT2N8670"
        XCTAssertEqual(id.count, 20, "fixture is a realistic full-length id")
        XCTAssertEqual(RefMenuModel.controlLabel(kind: .snapshot, name: id), "YF19J4V0")
    }

    func testOverLongBranchNameIsMiddleTruncated() {
        let name = "release/2026-09-05-experimental-rebuild-of-everything"
        let label = RefMenuModel.controlLabel(kind: .branch, name: name)

        XCTAssertLessThanOrEqual(label.count, limit)
        XCTAssertTrue(label.contains("\u{2026}"), "elided in the middle, not simply cut")
        // Derived from the name rather than hard-coded, so these keep
        // meaning the same thing if the cap is retuned.
        XCTAssertTrue(label.hasPrefix(String(name.prefix(3))), "the head survives")
        XCTAssertTrue(label.hasSuffix(String(name.suffix(3))), "and so does the tail")
    }

    /// No ref of any kind, at any length, may exceed the budget.
    func testNoLabelExceedsTheBudget() {
        let names = [
            "main",
            "v1",
            "YF19J4V0GM66MT2N8670",
            String(repeating: "x", count: 200),
            "release/2026-09-05-experimental-rebuild-of-everything",
        ]
        for kind in RefKind.allCases {
            for name in names {
                let label = RefMenuModel.controlLabel(kind: kind, name: name)
                XCTAssertLessThanOrEqual(
                    label.count,
                    limit,
                    "\(kind.rawValue):\(name) produced an over-wide label"
                )
            }
        }
    }

    /// The label the menu model publishes is the bounded one -- the guard is
    /// worthless if `currentLabel` bypasses `controlLabel`.
    func testCurrentLabelUsesTheBoundedForm() {
        let longBranch = String(repeating: "b", count: 60)
        let info = VersionInfo(
            branch: longBranch,
            refKind: "branch",
            branches: [longBranch],
            tags: [],
            ancestry: snapshots(1),
            truncated: false
        )
        let menu = RefMenuModel(pinned: info, current: info)

        XCTAssertLessThanOrEqual(menu.currentLabel.count, limit)
        XCTAssertEqual(
            menu.currentLabel,
            RefMenuModel.controlLabel(kind: .branch, name: longBranch)
        )
    }

    func testMiddleTruncateLeavesShortTextAlone() {
        XCTAssertEqual(RefMenuModel.middleTruncate("short", limit: 20), "short")
        XCTAssertEqual(RefMenuModel.middleTruncate("", limit: 20), "")
    }

    /// Menu rows are not the control and are free to be long -- clamping
    /// them would throw away the message and date that make a snapshot
    /// identifiable.
    func testMenuRowLabelsAreNotClamped() {
        let pinned = defaultLoad()
        let menu = RefMenuModel(pinned: pinned, current: pinned)
        let row = menu.sections.first { $0.title == "Snapshots" }?.rows.first

        XCTAssertGreaterThan(row?.label.count ?? 0, limit)
    }
}

// MARK: - Adaptive presentation

/// The control shows its label when it fits and collapses to the icon when
/// it does not. Getting this wrong in the permissive direction is not a
/// clipped label -- the toolbar sweeps the whole control into its overflow
/// menu and leaves it there until something forces a re-layout. Every
/// boundary case below therefore asserts the conservative answer.
final class RefControlPresentationTests: XCTestCase {

    /// A font-free stand-in, so these tests pin the arithmetic rather than
    /// the system font's metrics (which vary by OS version and settings).
    private let measure: (String) -> CGFloat = { CGFloat($0.count) * 10 }

    /// Everything the slot math subtracts before any text is considered.
    private var reserved: CGFloat {
        RefMenuModel.Slot.trafficLights
            + RefMenuModel.Slot.sidebarToggle
            + RefMenuModel.Slot.margins
            + RefMenuModel.Slot.safety
    }

    private func presentation(_ label: String, _ width: CGFloat) -> RefControlPresentation {
        RefMenuModel.presentation(label: label, sidebarWidth: width, measure: measure)
    }

    func testWideSidebarShowsTheFullLabel() {
        XCTAssertEqual(presentation("main", 500), .fullLabel)
    }

    func testNarrowSidebarCollapsesToIconOnly() {
        XCTAssertEqual(presentation("main", 160), .iconOnly)
    }

    /// At exactly the available width the item is already at the edge of
    /// what the toolbar accepts, so a tie collapses.
    func testExactBoundaryCollapses() {
        let label = "abcd"
        let required = measure(label) + RefMenuModel.Slot.controlChrome
        let exact = reserved + required

        XCTAssertEqual(presentation(label, exact), .iconOnly, "a tie must collapse")
        XCTAssertEqual(presentation(label, exact + 1), .fullLabel, "one point more fits")
        XCTAssertEqual(presentation(label, exact - 1), .iconOnly, "one point less does not")
    }

    /// The first layout pass reports zero width. Starting collapsed and
    /// expanding is the safe direction; starting expanded risks an overflow
    /// that does not undo itself.
    func testUnknownWidthCollapses() {
        XCTAssertEqual(presentation("main", 0), .iconOnly)
        XCTAssertEqual(presentation("main", -100), .iconOnly)
        XCTAssertEqual(presentation("main", reserved), .iconOnly, "nothing left for the control")
    }

    /// A longer ref collapses at a width where a shorter one still fits --
    /// the decision has to actually depend on the label.
    func testDecisionDependsOnLabelLength() {
        let width = reserved + RefMenuModel.Slot.controlChrome + 45

        XCTAssertEqual(presentation("main", width), .fullLabel, "4 chars = 40pt, fits in 45")
        XCTAssertEqual(presentation("YF19J4V0", width), .iconOnly, "8 chars = 80pt, does not")
    }

    /// The scenario from the bug report, at the default sidebar width,
    /// using the fake measurer's 10pt-per-character so the expected answers
    /// follow from the constants rather than from font metrics.
    ///
    /// available = 260 - 170 = 90; chrome takes 45, leaving 45pt of text.
    /// "main" needs 40 and fits; an 8-character snapshot id needs 80 and
    /// does not -- which is exactly the case that used to overflow.
    func testDefaultWidthKeepsMainFullAndCollapsesSnapshots() {
        XCTAssertEqual(presentation("main", 260), .fullLabel)
        XCTAssertEqual(presentation("v1", 260), .fullLabel)
        XCTAssertEqual(presentation("YF19J4V0", 260), .iconOnly)
    }

    /// Dragging the divider narrower must collapse the control before it
    /// can ever be too wide: the decision is monotonic in width, so there is
    /// no width at which a label fits but a narrower one does not.
    func testCollapseIsMonotonicInWidth() {
        var sawIconOnly = false
        for width in stride(from: 400.0, through: 100.0, by: -5.0) {
            let result = presentation("main", CGFloat(width))
            if result == .iconOnly {
                sawIconOnly = true
            } else {
                XCTAssertFalse(sawIconOnly, "expanded again at \(width) after collapsing")
            }
        }
        XCTAssertTrue(sawIconOnly, "narrow enough widths must collapse")
    }

    /// The production measurer is font-dependent, so it is checked for
    /// sane behavior rather than exact values.
    func testDefaultMeasurerIsPositiveAndMonotonic() {
        XCTAssertGreaterThan(RefMenuModel.measureLabel("m"), 0)
        XCTAssertGreaterThan(
            RefMenuModel.measureLabel("a long branch name"),
            RefMenuModel.measureLabel("main")
        )
        XCTAssertEqual(RefMenuModel.measureLabel(""), 0)
    }

    /// The calibration itself, with the real font and the real constants at
    /// the default 260pt sidebar. These two assertions are what the `Slot`
    /// values were tuned to satisfy, and they encode the user-observed
    /// behavior directly: `main` must keep its label (or the feature is
    /// pointless) and an 8-character snapshot id must collapse (or it
    /// overflows, which is the bug).
    func testRealMeasurerMatchesObservedBehaviorAtDefaultWidth() {
        let defaultSidebar: CGFloat = 260

        XCTAssertEqual(
            RefMenuModel.presentation(label: "main", sidebarWidth: defaultSidebar),
            .fullLabel,
            "the common case must not collapse"
        )
        for id in ["YF19J4V0", "BVP8CH0S", "1CECHNKR"] {
            XCTAssertEqual(
                RefMenuModel.presentation(label: id, sidebarWidth: defaultSidebar),
                .iconOnly,
                "\(id) is the width that used to overflow"
            )
        }
    }

    /// Guards the clearance the calibration relies on, so a future tweak to
    /// `Slot` cannot silently land right on the boundary.
    func testCalibrationKeepsClearanceOnBothSides() {
        let available = 260 - reserved
        let main = RefMenuModel.measureLabel("main") + RefMenuModel.Slot.controlChrome
        let snapshot = RefMenuModel.measureLabel("YF19J4V0") + RefMenuModel.Slot.controlChrome

        XCTAssertGreaterThan(available - main, 10, "main should fit with room to spare")
        XCTAssertGreaterThan(snapshot - available, 10, "snapshots should collapse decisively")
    }
}

// MARK: - Ref strings

final class RefKindTests: XCTestCase {

    func testRefStringsUseTheWireSpelling() {
        XCTAssertEqual(RefKind.branch.ref("main"), "branch:main")
        XCTAssertEqual(RefKind.tag.ref("v1"), "tag:v1")
        XCTAssertEqual(RefKind.snapshot.ref("ABC123"), "snapshot:ABC123")
    }

    func testRoundTrip() {
        for kind in RefKind.allCases {
            let parsed = RefKind.parse(kind.ref("some-name"))
            XCTAssertEqual(parsed?.kind, kind)
            XCTAssertEqual(parsed?.name, "some-name")
        }
    }

    /// Splitting on the first colon only, so a name containing one survives.
    func testNameMayContainAColon() {
        let parsed = RefKind.parse("tag:release:2026-09")
        XCTAssertEqual(parsed?.kind, .tag)
        XCTAssertEqual(parsed?.name, "release:2026-09")
    }

    func testMalformedRefsAreRejected() {
        XCTAssertNil(RefKind.parse("garbage"), "no separator")
        XCTAssertNil(RefKind.parse("bogus:main"), "unknown kind")
        XCTAssertNil(RefKind.parse("branch:"), "empty name")
        XCTAssertNil(RefKind.parse(""), "empty string")
    }

    /// Every row's `ref` must parse back to the kind and name it was built
    /// from -- that string is what gets handed to the FFI.
    func testEveryMenuRowProducesAParseableRef() {
        let pinned = defaultLoad()
        let menu = RefMenuModel(pinned: pinned, current: pinned)

        for row in menu.sections.flatMap(\.rows) {
            let parsed = RefKind.parse(row.ref)
            XCTAssertEqual(parsed?.kind, row.kind, "round trip for \(row.ref)")
            XCTAssertEqual(parsed?.name, row.name, "round trip for \(row.ref)")
        }
    }
}
