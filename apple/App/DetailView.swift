//
//  DetailView.swift
//  ndLook
//
//  The detail pane: everything known about whichever group or variable is
//  selected in the sidebar.
//
//  Built as a `Form` with `.formStyle(.grouped)` rather than a `ScrollView`
//  of hand-rolled rows, or a `Table`. Grouped forms are what macOS itself
//  uses for exactly this shape of content -- labelled sections of
//  name/value pairs -- so the spacing, section headers and scrolling all
//  come out native for free. A `Table` was the other candidate for the
//  attribute lists, but it brings its own scroll view, which would fight
//  the outer one and split the pane into two independently scrolling
//  regions.
//

import SwiftUI

/// Where a `SidebarItem` actually points in the summary.
///
/// The sidebar's selection is a path, not a reference, so it has to be
/// resolved against the current summary on every render. That indirection
/// is deliberate: reloading a different Icechunk ref replaces the whole
/// `DatasetSummary`, and a path survives that where a captured struct
/// would go stale.
private enum ResolvedSelection {
    case group(GroupSummary, path: String)
    case variable(VarSummary, kind: VariableKind, path: String)
}

struct DetailView: View {
    let summary: DatasetSummary
    let selection: SidebarItem?

    var body: some View {
        Form {
            switch resolved {
            case .group(let group, let path):
                GroupDetail(group: group, path: path)
            case .variable(let variable, let kind, let path):
                VariableDetail(variable: variable, kind: kind, path: path)
            }
        }
        .formStyle(.grouped)
    }

    /// Resolves the selection, falling back to the root group.
    ///
    /// The fallback covers two cases that both want the same answer: no
    /// selection at all (a freshly opened window), and a selection that no
    /// longer resolves because the summary was replaced by one where that
    /// path does not exist. Showing the root beats showing an error for
    /// what is really just a stale highlight.
    private var resolved: ResolvedSelection {
        guard let selection, let found = Self.resolve(selection, in: summary.root) else {
            return .group(summary.root, path: SidebarItem.rootPath)
        }
        return found
    }

    private static func resolve(_ item: SidebarItem, in root: GroupSummary) -> ResolvedSelection? {
        switch item {
        case .group(let path):
            guard let group = findGroup(path: path, in: root, at: SidebarItem.rootPath) else {
                return nil
            }
            return .group(group, path: path)

        case .variable(let path, let kind):
            // A variable lives in its parent group's `coords` or `dataVars`
            // depending on how the reader classified it, and the kind was
            // carried through the selection precisely so this lookup does
            // not have to search both and guess.
            let parent = SidebarItem.parentPath(of: path)
            guard let group = findGroup(path: parent, in: root, at: SidebarItem.rootPath) else {
                return nil
            }
            let name = SidebarItem.name(of: path)
            let candidates = kind == .coordinate ? group.coords : group.dataVars
            guard let variable = candidates.first(where: { $0.name == name }) else { return nil }
            return .variable(variable, kind: kind, path: path)
        }
    }

    /// Depth-first search for the group at `path`, threading each group's
    /// own path down as it recurses (the model does not store it).
    private static func findGroup(
        path: String,
        in group: GroupSummary,
        at groupPath: String
    ) -> GroupSummary? {
        if groupPath == path { return group }
        for child in group.children {
            let childPath = SidebarItem.childPath(groupPath, child.name)
            // Prune whole subtrees that cannot contain the target: a
            // descendant's path always begins with its ancestor's.
            guard path == childPath || path.hasPrefix("\(childPath)/") else { continue }
            if let found = findGroup(path: path, in: child, at: childPath) { return found }
        }
        return nil
    }
}

// MARK: - Group detail

/// Dimensions and attributes of one group.
private struct GroupDetail: View {
    let group: GroupSummary
    let path: String

    var body: some View {
        Section {
            LabeledContent("Path", value: path)
        } header: {
            DetailHeader(
                title: group.name.isEmpty ? SidebarItem.rootPath : group.name,
                subtitle: group.name.isEmpty ? "Root group" : "Group"
            )
        }

        Section("Dimensions") {
            if group.dims.isEmpty {
                EmptyNote("No dimensions")
            } else {
                ForEach(group.dims) { dim in
                    LabeledContent(dim.name) {
                        // The unlimited flag only ever means anything for
                        // netCDF; it is always false elsewhere, so it is
                        // shown as an annotation rather than its own column.
                        Text(dim.isUnlimited ? "\(dim.size) (unlimited)" : "\(dim.size)")
                            .monospaced()
                    }
                }
            }
        }

        AttributeSection(attrs: group.attrs)
    }
}

// MARK: - Variable detail

/// Type, shape, chunking, value peek and attributes of one variable.
private struct VariableDetail: View {
    let variable: VarSummary
    let kind: VariableKind
    let path: String

    var body: some View {
        Section {
            LabeledContent("Path", value: path)
            LabeledContent("Type") {
                Text(variable.dtype).monospaced()
            }
            LabeledContent("Dimensions") {
                Text(variable.dims.isEmpty ? "scalar" : "(\(variable.dims.joined(separator: ", ")))")
                    .monospaced()
            }
            if !variable.shape.isEmpty {
                LabeledContent("Shape") {
                    Text(Self.extents(variable.shape)).monospaced()
                }
            }
            if let chunks = variable.chunks, !chunks.isEmpty {
                LabeledContent("Chunks") {
                    Text(Self.extents(chunks)).monospaced()
                }
            }
        } header: {
            DetailHeader(
                title: variable.name,
                subtitle: kind == .coordinate ? "Coordinate" : "Data variable"
            )
        }

        // The reader only fills this in for variables small enough to peek
        // at cheaply (a short 1-D coordinate, say), and it arrives already
        // formatted for display, so it is printed verbatim.
        if let preview = variable.preview {
            Section("Values") {
                Text(preview)
                    .monospaced()
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }

        AttributeSection(attrs: variable.attrs)
    }

    /// `24 × 180 × 360`.
    private static func extents(_ values: [UInt64]) -> String {
        values.map(String.init).joined(separator: " \u{00D7} ")
    }
}

// MARK: - Shared pieces

/// The bold name-and-role heading that opens a group's or variable's first
/// section.
private struct DetailHeader: View {
    let title: String
    let subtitle: String

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.headline)
                .textSelection(.enabled)
            Text(subtitle)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.bottom, 4)
    }
}

/// The attribute table, shared by groups and variables.
///
/// Values are monospaced and selectable: attributes are frequently things
/// people need to copy verbatim (a CRS string, a fill value, a units
/// spelling), and proportional text makes numeric ones harder to compare
/// down the column.
private struct AttributeSection: View {
    let attrs: [AttrEntry]

    var body: some View {
        Section("Attributes") {
            if attrs.isEmpty {
                EmptyNote("No attributes")
            } else {
                ForEach(attrs) { attr in
                    LabeledContent(attr.name) {
                        Text(attr.value.displayString)
                            .monospaced()
                            .textSelection(.enabled)
                            .multilineTextAlignment(.trailing)
                    }
                }
            }
        }
    }
}

/// Placeholder for a section that would otherwise render as a blank box.
private struct EmptyNote: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .foregroundStyle(.secondary)
    }
}
