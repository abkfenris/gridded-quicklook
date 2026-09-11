//
//  RefPicker.swift
//  ndLook
//
//  The toolbar control for choosing which version of an Icechunk
//  repository to view: a branch, a tag, or a bare snapshot from the
//  ancestry of whatever is currently loaded.
//
//  Only Icechunk has version history, so this is the one piece of the UI
//  that is format-specific; `DocumentView` puts it in the toolbar only when
//  the loaded summary carries a `VersionInfo`.
//

import Foundation
import SwiftUI

/// The three kinds of ref `ndlook_summarize_json` accepts.
///
/// The raw values are the wire spellings on both sides of the FFI: they are
/// what `VersionInfo.refKind` reports back, and what the `"kind:value"` ref
/// string is built from. One enum for both directions keeps the two from
/// drifting.
enum RefKind: String {
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
    func ref(_ name: String) -> String {
        "\(rawValue):\(name)"
    }
}

/// Menu listing every branch, tag and recent snapshot in the repository.
struct RefPicker: View {
    let versionInfo: VersionInfo
    @Binding var selectedRef: String?

    var body: some View {
        Menu {
            if !versionInfo.branches.isEmpty {
                Section("Branches") {
                    ForEach(versionInfo.branches, id: \.self) { name in
                        entry(kind: .branch, name: name, label: name)
                    }
                }
            }

            if !versionInfo.tags.isEmpty {
                Section("Tags") {
                    ForEach(versionInfo.tags, id: \.self) { name in
                        entry(kind: .tag, name: name, label: name)
                    }
                }
            }

            if !versionInfo.ancestry.isEmpty {
                Section("Snapshots") {
                    ForEach(versionInfo.ancestry) { snapshot in
                        entry(
                            kind: .snapshot,
                            name: snapshot.id,
                            label: Self.label(for: snapshot)
                        )
                    }
                    if versionInfo.truncated {
                        // The reader caps the ancestry walk, so this list is
                        // the most recent snapshots rather than all of them.
                        // Disabled because there is nothing to select -- it
                        // is a note, not an option.
                        Button("Older history not shown") {}
                            .disabled(true)
                    }
                }
            }
        } label: {
            Label(currentName, systemImage: currentKind.symbol)
        }
        .help("Choose which version of this repository to view")
    }

    /// One selectable ref.
    ///
    /// The checkmark tracks `versionInfo` -- what is actually on screen --
    /// rather than `selectedRef`. That is what makes the initial state
    /// correct: `selectedRef` starts `nil` (meaning "the default"), and only
    /// the loaded summary knows the default resolved to `main`.
    private func entry(kind: RefKind, name: String, label: String) -> some View {
        let isCurrent = kind == currentKind && name == versionInfo.branch
        return Button {
            selectedRef = kind.ref(name)
        } label: {
            if isCurrent {
                Label(label, systemImage: "checkmark")
            } else {
                Text(label)
            }
        }
    }

    /// What kind of ref is on screen. `refKind` is absent in JSON produced
    /// before the field existed, where a branch was the only possibility.
    private var currentKind: RefKind {
        versionInfo.refKind.flatMap(RefKind.init(rawValue:)) ?? .branch
    }

    /// The control's own label: the branch or tag name, or an abbreviated
    /// snapshot id (a full one is 20 characters of base32 and would crowd
    /// the toolbar).
    private var currentName: String {
        currentKind == .snapshot ? Self.abbreviate(versionInfo.branch) : versionInfo.branch
    }

    // MARK: - Formatting

    /// A snapshot's menu row: `BVP8CH0S \u{00B7} update global attrs \u{00B7} Sep 5, 2026 at 11:45 PM`.
    ///
    /// Built as one line because macOS menu items do not lay out stacked
    /// text reliably; the message and timestamp are dropped when absent
    /// rather than leaving empty separators behind.
    private static func label(for snapshot: SnapshotInfo) -> String {
        var parts = [abbreviate(snapshot.id)]
        if let message = snapshot.message, !message.isEmpty {
            parts.append(truncate(message))
        }
        if let wroteAt = snapshot.wroteAt, let formatted = formatTimestamp(wroteAt) {
            parts.append(formatted)
        }
        return parts.joined(separator: " \u{00B7} ")
    }

    private static func abbreviate(_ id: String) -> String {
        String(id.prefix(8))
    }

    private static func truncate(_ message: String, limit: Int = 48) -> String {
        message.count <= limit ? message : "\(message.prefix(limit))\u{2026}"
    }

    /// Renders an RFC 3339 timestamp in the viewer's locale, or returns
    /// `nil` so the caller can leave it out.
    ///
    /// Two parsers because Icechunk writes microsecond precision
    /// (`...:49.906701+00:00`) but not every producer does, and
    /// `ISO8601DateFormatter` fails outright on a fractional part it was not
    /// configured to expect rather than ignoring it.
    private static func formatTimestamp(_ raw: String) -> String? {
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
