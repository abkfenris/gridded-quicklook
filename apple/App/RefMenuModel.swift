//
//  RefMenuModel.swift
//  ndLook
//
//  The contents of the Icechunk ref menu, derived as plain data.
//
//  Deliberately free of SwiftUI: everything here is a pure function of two
//  `VersionInfo` values, which is what makes it unit-testable without
//  standing up a view. `RefPicker` renders what this produces and decides
//  nothing itself.
//
//  The two inputs are not interchangeable, and keeping them apart is the
//  whole point of this type:
//
//  - `pinned` is the version metadata from the *default* load of the repo
//    (ref = nil, i.e. main's tip). It supplies the menu's contents.
//  - `current` is the version metadata of whatever is on screen right now.
//    It supplies only the checkmark and the control's label.
//
//  Using `current` for both -- which is what the first implementation did
//  -- makes the menu shrink as you navigate: the Rust reader walks ancestry
//  backwards from the *viewed* ref, so selecting an older tag or snapshot
//  returns a shorter history, and the options you used to get there vanish
//  from the menu. Pinning the contents to the default load keeps the menu a
//  stable picture of the repository instead of a view-dependent one.
//

// AppKit, but no SwiftUI: text measurement needs a real font, and
// `NSAttributedString` is the honest way to ask how wide a string draws.
// Keeping views out is what matters for testability, not keeping AppKit out.
import AppKit
import Foundation

/// How much of the ref control fits in the sidebar's slice of the titlebar.
///
/// The control must *never* be too wide. A toolbar that decides an item does
/// not fit sweeps it into the overflow menu, and that is sticky -- the item
/// stays there until something forces a toolbar re-layout, such as toggling
/// the sidebar. So the choice is made conservatively: when in doubt, shrink.
enum RefControlPresentation: Equatable {
    /// Kind icon plus the ref name.
    case fullLabel
    /// Kind icon only, with the name moved to the tooltip.
    case iconOnly
}

/// The three kinds of ref `ndlook_summarize_json` accepts.
///
/// The raw values are the wire spellings on both sides of the FFI: they are
/// what `VersionInfo.refKind` reports back, and what the `"kind:value"` ref
/// string is built from. One enum for both directions keeps the two from
/// drifting.
enum RefKind: String, CaseIterable {
    case branch
    case tag
    case snapshot

    var symbol: String {
        switch self {
        case .branch: "arrow.triangle.branch"
        case .tag: "tag"
        case .snapshot: "clock.arrow.circlepath"
        }
    }

    /// Builds the `"kind:value"` ref string the FFI expects.
    ///
    /// The only place this grammar is spelled out on the Swift side. There
    /// was an inverse `parse` here too, but nothing in the app ever needed
    /// to take a ref string apart -- refs are built here and handed straight
    /// to the FFI -- so it existed only to be tested, while giving the wire
    /// grammar a second definition that could drift from this one.
    func ref(_ name: String) -> String {
        "\(rawValue):\(name)"
    }
}

/// Everything the ref menu needs in order to render.
struct RefMenuModel: Equatable {

    /// One selectable ref.
    struct Row: Identifiable, Equatable {
        let kind: RefKind
        let name: String
        /// What the menu row reads. For branches and tags this is just the
        /// name; snapshots get an abbreviated id, message and date.
        let label: String
        /// Whether this row is the ref currently on screen.
        let isCurrent: Bool

        /// The `"kind:value"` string to hand the FFI when this is chosen.
        var ref: String { kind.ref(name) }
        var id: String { ref }
    }

    struct Section: Identifiable, Equatable {
        let title: String
        let rows: [Row]
        /// Appends the "older history not shown" note, for the section that
        /// lists a capped ancestry walk.
        let showsTruncationNote: Bool

        var id: String { title }
    }

    /// Populated sections only -- an empty one is omitted rather than
    /// rendered as a bare header.
    let sections: [Section]
    /// The kind of ref on screen, for the control's icon.
    let currentKind: RefKind
    /// The control's own text: a branch or tag name, or an abbreviated
    /// snapshot id (a full one is 20 characters of base32).
    let currentLabel: String

    init(pinned: VersionInfo, current: VersionInfo) {
        // `refKind` is absent in JSON produced before the field existed,
        // where a branch was the only possibility.
        let currentKind = current.refKind.flatMap(RefKind.init(rawValue:)) ?? .branch
        self.currentKind = currentKind
        self.currentLabel = Self.controlLabel(kind: currentKind, name: current.branch)

        // A row is current when both its kind and its name match what is on
        // screen. Checking the kind as well as the name is what keeps
        // `ancestry[0]` unchecked while you are viewing the branch whose tip
        // it happens to be.
        func isCurrent(_ kind: RefKind, _ name: String) -> Bool {
            kind == currentKind && name == current.branch
        }

        var sections: [Section] = []

        if !pinned.branches.isEmpty {
            sections.append(
                Section(
                    title: "Branches",
                    rows: pinned.branches.map {
                        Row(kind: .branch, name: $0, label: $0, isCurrent: isCurrent(.branch, $0))
                    },
                    showsTruncationNote: false
                )
            )
        }

        if !pinned.tags.isEmpty {
            sections.append(
                Section(
                    title: "Tags",
                    rows: pinned.tags.map {
                        Row(kind: .tag, name: $0, label: $0, isCurrent: isCurrent(.tag, $0))
                    },
                    showsTruncationNote: false
                )
            )
        }

        if !pinned.ancestry.isEmpty {
            sections.append(
                Section(
                    title: "Snapshots",
                    rows: pinned.ancestry.map { snapshot in
                        Row(
                            kind: .snapshot,
                            name: snapshot.id,
                            label: Self.label(for: snapshot),
                            isCurrent: isCurrent(.snapshot, snapshot.id)
                        )
                    },
                    // The reader caps the ancestry walk, so a truncated list
                    // is the most recent snapshots rather than all of them.
                    showsTruncationNote: pinned.truncated
                )
            )
        }

        self.sections = sections
    }

    // MARK: - Formatting

    /// A snapshot's menu row: `BVP8CH0S \u{00B7} update global attrs \u{00B7} Sep 5, 2026 at 11:45 PM`.
    ///
    /// Built as one line because macOS menu items do not lay out stacked
    /// text reliably; the message and timestamp are dropped when absent
    /// rather than leaving empty separators behind.
    static func label(for snapshot: SnapshotInfo) -> String {
        var parts = [abbreviate(snapshot.id)]
        if let message = snapshot.message, !message.isEmpty {
            parts.append(truncate(message))
        }
        if let wroteAt = snapshot.wroteAt, let formatted = formatTimestamp(wroteAt) {
            parts.append(formatted)
        }
        return parts.joined(separator: " \u{00B7} ")
    }

    // MARK: - Fitting the toolbar slot

    /// Empirical geometry of the sidebar's slice of the titlebar.
    ///
    /// None of this is queryable: AppKit exposes no API for "how much room
    /// is left between the traffic lights and the sidebar toggle", so these
    /// are empirical constants, calibrated against observed behavior rather
    /// than derived.
    ///
    /// The calibration data, from the default 260pt sidebar (see
    /// `navigationSplitViewColumnWidth` in `DocumentView`): a `main` label
    /// fits, and an 8-character snapshot id does not. In the toolbar font
    /// those measure 29pt and 62pt of text, so with `controlChrome` they
    /// need 74pt and 107pt respectively. The real slot therefore lies
    /// somewhere in `(74, 107]`, which brackets `260 - reserved` and pins
    /// the total reserved width to `(153, 186]`. The values below total 170
    /// -- near the middle of that range, leaving roughly 16pt of clearance
    /// on both sides of the decision.
    ///
    /// Bias when retuning: over-reserving merely collapses the label to an
    /// icon a little sooner, while under-reserving drops the whole control
    /// into the overflow menu, where it *stays* until something forces a
    /// toolbar re-layout. Those outcomes are not remotely symmetric, which
    /// is what the deliberately large `safety` term is paying for.
    enum Slot {
        /// Close/minimize/zoom plus their leading inset.
        static let trafficLights: CGFloat = 78
        /// The system sidebar toggle, which sits after our item.
        static let sidebarToggle: CGFloat = 40
        /// Inter-item spacing at both ends.
        static let margins: CGFloat = 20
        /// Deliberate slack, because the failure mode is sticky.
        static let safety: CGFloat = 32
        /// The control's own furniture around the text: kind icon, the menu
        /// chevron, and the button's internal padding.
        static let controlChrome: CGFloat = 45
    }

    /// Everything the slot math subtracts before any of the control is
    /// considered.
    static let reservedWidth = Slot.trafficLights
        + Slot.sidebarToggle
        + Slot.margins
        + Slot.safety

    /// The narrowest sidebar at which the control can appear at all -- the
    /// point where even the icon-only form stops fitting.
    static let minimumSidebarWidth = reservedWidth + Slot.controlChrome

    /// The narrowest the sidebar column may be dragged, wired into
    /// `navigationSplitViewColumnWidth` in `DocumentView`.
    ///
    /// Deliberately above `minimumSidebarWidth`, which is what makes "the
    /// sidebar is open" imply "at least the icon is visible": with the floor
    /// below that threshold there was a band where the column was open but
    /// the control had nowhere to go, so it rendered nothing at all.
    ///
    /// Raising the floor was chosen over trimming `Slot.safety` to reach the
    /// old 200pt minimum. The stage-14 calibration only brackets the true
    /// reserved width to `(153, 186]`; a smaller safety term would have this
    /// code believe the control fits at 200pt even when the real reserve is
    /// at the high end of that range, and being wrong that way means sticky
    /// overflow rather than a slightly narrower minimum. 15-odd points of
    /// floor is the cheaper thing to spend.
    ///
    /// These two live beside the slot arithmetic rather than at the call
    /// site precisely so they cannot drift apart from it.
    static let sidebarMinimumWidth: CGFloat = 220

    /// The sidebar's preferred width, and the width the `Slot` constants
    /// were calibrated against.
    static let sidebarIdealWidth: CGFloat = 260

    /// How long the sidebar's width must hold still before the full label is
    /// allowed back.
    ///
    /// Long enough to outlast a drag's frame-to-frame jitter, short enough
    /// that letting go feels immediate.
    static let resizeSettleInterval: Duration = .milliseconds(200)

    /// Decides how to draw the ref control, given how wide the sidebar
    /// column currently is -- or `nil` when it should not be drawn at all.
    ///
    /// The `nil` case is what keeps the control from flashing into the
    /// overflow menu while the sidebar drawer opens. Expanding animates the
    /// column's width up from nothing, so for the first frames the slot is
    /// far too narrow; a control rendered then is measured, judged not to
    /// fit, and swept into overflow before the animation finishes. Widths
    /// below `minimumSidebarWidth` therefore render nothing, and the
    /// control appears only once there is genuinely room for it.
    ///
    /// `measure` is injectable so tests can pin down the arithmetic with a
    /// predictable font-free stand-in; production uses `measureLabel`.
    ///
    /// Ties go the conservative way at both thresholds. At exactly the
    /// available width the item is already at the edge of what the toolbar
    /// will accept, and being one point out is not a clipped label but a
    /// vanished control.
    /// `isResizing` forces the icon-only form while the column's width is in
    /// motion, and it is a fix for a race rather than a stylistic choice.
    ///
    /// The width this function is given arrives through SwiftUI state: the
    /// `GeometryReader` reports a width, that write schedules a re-render,
    /// and only then does AppKit re-measure the toolbar. Drag the divider
    /// faster than that round trip and the control still on screen was sized
    /// for a slot that has already shrunk -- it is measured too wide, swept
    /// into the overflow menu, and stays there. No safety margin can close
    /// that gap, because the gap is in time, not in points.
    ///
    /// Downgrading the instant the width moves sidesteps it: the icon-only
    /// form fits at every width the column is allowed to have (see
    /// `sidebarMinimumWidth`), so whatever width AppKit ends up measuring
    /// against, the answer is the same. The full label returns once the
    /// width has been still for `resizeSettleInterval`.
    static func presentation(
        label: String,
        sidebarWidth: CGFloat,
        isResizing: Bool = false,
        measure: (String) -> CGFloat = measureLabel
    ) -> RefControlPresentation? {
        let available = sidebarWidth - reservedWidth

        // Also catches the first layout pass, where the width is still 0.
        // Checked before `isResizing` so a drag through the sub-minimum
        // range still renders nothing rather than an icon with no room.
        guard available > Slot.controlChrome else { return nil }

        guard !isResizing else { return .iconOnly }

        return measure(label) + Slot.controlChrome < available ? .fullLabel : .iconOnly
    }

    /// How wide `text` draws in the titlebar's font.
    static func measureLabel(_ text: String) -> CGFloat {
        let font = NSFont.systemFont(ofSize: NSFont.systemFontSize)
        let width = (text as NSString).size(withAttributes: [.font: font]).width
        // Round up: a fractional point that gets rounded the other way is
        // exactly the kind of off-by-a-hair that overflows.
        return width.rounded(.up)
    }

    /// The most characters the control's label may carry.
    ///
    /// A backstop, not the primary bound. `presentation` is what actually
    /// keeps the control inside the slot, measuring the real label against
    /// the real geometry -- see `Slot` for those figures, which are stated
    /// there once and deliberately not repeated here. This cap only stops a
    /// pathological name (a branch named after a whole sentence) from
    /// reaching the measurement at all.
    ///
    /// Twelve is about what the narrowest allowed slot holds at the system
    /// font. Erring short is the right bias: an over-long label does not
    /// merely clip, it pushes the entire control into the toolbar's overflow
    /// menu, which is far harder to find than a truncated name is to read.
    static let controlLabelLimit = 12

    /// The text on the control itself, bounded in length.
    ///
    /// Snapshots collapse to the same 8-character prefix the menu rows use;
    /// a full id is 20 characters of opaque base32 and says nothing extra.
    /// Branch and tag names are user-chosen and normally short, so they are
    /// shown whole -- but capped, for the reason on `controlLabelLimit`.
    ///
    /// The cap is applied to the *string*, not left to the view, because
    /// `.lineLimit`/`.truncationMode` change only how text draws, not the
    /// width it asks for -- and width is what the toolbar measures.
    static func controlLabel(kind: RefKind, name: String) -> String {
        let base = kind == .snapshot ? abbreviate(name) : name
        return middleTruncate(base, limit: controlLabelLimit)
    }

    /// Keeps both ends and elides the middle, which is where a long ref
    /// name carries the least information (a dated branch or a
    /// slash-namespaced tag is distinguished by its head and tail).
    static func middleTruncate(_ text: String, limit: Int) -> String {
        guard text.count > limit, limit > 1 else { return text }
        let kept = limit - 1
        let head = kept - kept / 2
        let tail = kept / 2
        return "\(text.prefix(head))\u{2026}\(text.suffix(tail))"
    }

    static func abbreviate(_ id: String) -> String {
        String(id.prefix(8))
    }

    static func truncate(_ message: String, limit: Int = 48) -> String {
        message.count <= limit ? message : "\(message.prefix(limit))\u{2026}"
    }

    /// Renders an RFC 3339 timestamp in the viewer's locale, or returns
    /// `nil` so the caller can leave it out.
    ///
    /// Two parsers because Icechunk writes microsecond precision
    /// (`...:49.906701+00:00`) but not every producer does, and
    /// `ISO8601DateFormatter` fails outright on a fractional part it was not
    /// configured to expect rather than ignoring it.
    static func formatTimestamp(_ raw: String) -> String? {
        guard let date = fractionalParser.date(from: raw) ?? wholeSecondParser.date(from: raw) else {
            return nil
        }
        return date.formatted(date: .abbreviated, time: .shortened)
    }

    private static let fractionalParser: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    private static let wholeSecondParser: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter
    }()
}
