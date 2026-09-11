//
//  SidebarView.swift
//  ndLook
//
//  The datatree navigator: every group, coordinate and data variable in the
//  dataset, as one selectable outline.
//
//  This file also defines the vocabulary the detail pane uses to talk about
//  a selection (`SidebarItem` and the path helpers on it), because the
//  sidebar is what mints those paths and there should be exactly one
//  spelling of how a path is built.
//

import SwiftUI

// MARK: - Selection

/// Whether a variable was classified as a coordinate or as a data variable.
///
/// The Rust reader applies xarray's heuristic and hands back two separate
/// lists; this carries that classification through the selection so the
/// detail pane can resolve a path without re-deriving it.
enum VariableKind: Hashable {
    case coordinate
    case dataVariable

    var symbol: String {
        switch self {
        case .coordinate: "ruler"
        case .dataVariable: "cube"
        }
    }
}

/// One selectable thing in the sidebar, identified by its path from the
/// dataset root.
///
/// Paths are slash-joined and absolute: the root group is `"/"`, a subgroup
/// is `"/model"`, and a variable inside it is `"/model/temperature"`. Names
/// alone would not do -- a variable name is unique only within its own
/// group, so two groups can each hold a `time` -- and the path is also what
/// lets the detail pane walk back down to the node without holding a
/// reference to it.
enum SidebarItem: Hashable, Identifiable {
    case group(path: String)
    case variable(path: String, kind: VariableKind)

    var id: String {
        switch self {
        case .group(let path): "group:\(path)"
        // A group and a variable can never share a path, but the kind is
        // part of the case's identity, so it belongs in the id too.
        case .variable(let path, let kind): "var:\(kind):\(path)"
        }
    }

    /// The path this item names, whichever case it is.
    var path: String {
        switch self {
        case .group(let path): path
        case .variable(let path, _): path
        }
    }

    /// The path of the dataset root group.
    static let rootPath = "/"

    /// Appends `name` to `parent`, producing the child's absolute path.
    ///
    /// Special-cased for the root so its children come out as `"/model"`
    /// rather than `"//model"`.
    static func childPath(_ parent: String, _ name: String) -> String {
        parent == rootPath ? "\(rootPath)\(name)" : "\(parent)/\(name)"
    }

    /// The path of the group containing `path`; the root for a top-level
    /// item.
    ///
    /// Group and variable names cannot contain a slash in any of the
    /// formats we read, so splitting on the last one is unambiguous.
    static func parentPath(of path: String) -> String {
        guard let slash = path.lastIndex(of: "/") else { return rootPath }
        let parent = String(path[path.startIndex..<slash])
        return parent.isEmpty ? rootPath : parent
    }

    /// The final component of `path` -- the group's or variable's own name.
    static func name(of path: String) -> String {
        guard let slash = path.lastIndex(of: "/") else { return path }
        return String(path[path.index(after: slash)...])
    }
}

// MARK: - Display tree

/// One row in the outline, pre-built from the model.
///
/// The sidebar renders this rather than walking `GroupSummary` directly
/// because `OutlineGroup` needs a single recursive element type with one
/// children key path, and because each row's path has to be threaded down
/// from its parent -- something the model deliberately does not store (see
/// `VarSummary`).
struct SidebarNode: Identifiable, Hashable {
    let item: SidebarItem
    let title: String
    /// Secondary text on variable rows: the dims and dtype, e.g.
    /// `(time, lat, lon) float32`. `nil` on group rows.
    let subtitle: String?
    let symbol: String
    /// `nil`, not `[]`, for anything that should render without a
    /// disclosure triangle -- `OutlineGroup` draws one for an empty array.
    let children: [SidebarNode]?

    var id: SidebarItem { item }

    /// Builds the node for `group` and, recursively, everything beneath it.
    ///
    /// Child order matches how the formats' own reprs read: coordinates,
    /// then data variables, then subgroups.
    static func make(group: GroupSummary, at path: String) -> SidebarNode {
        var children: [SidebarNode] = []
        children += group.coords.map { make(variable: $0, kind: .coordinate, in: path) }
        children += group.dataVars.map { make(variable: $0, kind: .dataVariable, in: path) }
        children += group.children.map {
            make(group: $0, at: SidebarItem.childPath(path, $0.name))
        }

        return SidebarNode(
            item: .group(path: path),
            // The root group's name is the empty string in the model; show
            // it as the path separator so it reads as "the whole dataset".
            title: group.name.isEmpty ? SidebarItem.rootPath : group.name,
            subtitle: nil,
            symbol: "folder",
            children: children.isEmpty ? nil : children
        )
    }

    static func make(variable: VarSummary, kind: VariableKind, in parentPath: String) -> SidebarNode {
        SidebarNode(
            item: .variable(
                path: SidebarItem.childPath(parentPath, variable.name),
                kind: kind
            ),
            title: variable.name,
            subtitle: Self.signature(of: variable),
            symbol: kind.symbol,
            children: nil
        )
    }

    /// `(time, lat, lon) float32`, or just `float32` for a scalar.
    private static func signature(of variable: VarSummary) -> String {
        guard !variable.dims.isEmpty else { return variable.dtype }
        return "(\(variable.dims.joined(separator: ", "))) \(variable.dtype)"
    }
}

// MARK: - View

/// The dataset's structure as a selectable outline, with the Icechunk ref
/// picker in the sidebar's section of the window's titlebar.
struct SidebarView: View {
    let root: GroupSummary
    /// The ref menu's contents, or `nil` for the formats with no version
    /// history -- which is every format but Icechunk.
    let refMenu: RefMenuModel?
    @Binding var selectedRef: String?
    /// Drives the spinner beside the picker. It belongs to this view rather
    /// than to the window because the ref picker is what triggers the
    /// reload: showing progress next to the control that caused it is what
    /// explains why the tree below has not changed yet.
    let isReloading: Bool
    @Binding var selection: SidebarItem?

    /// The sidebar column's current width, observed from the list's own
    /// geometry. This is the input to the ref control's fit decision -- the
    /// toolbar itself will not tell an item how much room it has, so the
    /// column underneath it is the closest available proxy, and it tracks
    /// live as the split divider is dragged.
    @State private var sidebarWidth: CGFloat = 0

    var body: some View {
        list
            .toolbar { toolbarContent }
    }

    /// The ref picker.
    ///
    /// Declared on the sidebar column's content deliberately: items
    /// declared here disappear when the sidebar collapses, which is the
    /// behavior we want for the picker specifically -- a collapsed sidebar
    /// has no tree for a ref to apply to.
    ///
    /// The `ToolbarItem` itself is unconditional, and that is load-bearing:
    /// a `ToolbarItem` that comes and goes with a condition gets torn out
    /// of and put back into the window's toolbar on each rebuild, and
    /// AppKit does not reliably re-add it -- the symptom was the whole
    /// control vanishing after a ref switch until the sidebar was toggled.
    /// Keeping one stable item and varying only its *contents* keeps the
    /// toolbar's own structure fixed for the window's lifetime.
    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        // `.automatic`, not `.navigation`: `.navigation` names the window's
        // title area, which is on the detail side of the split, so it drags
        // the item across the divider regardless of which column declared
        // it. `.automatic` lets the item stay with its own column's toolbar
        // section.
        ToolbarItem(placement: .automatic) {
            // One item holding both controls, not two: a second
            // `ToolbarItem` is free to be reordered or swept into the
            // overflow menu independently, and the spinner is only
            // meaningful directly beside the control that started the
            // reload.
            HStack(spacing: 6) {
                if let refMenu {
                    // No width constraint here: `RefPicker` sets its own
                    // ceiling, and a second competing frame would only
                    // obscure which one the toolbar actually measures.
                    RefPicker(
                        menu: refMenu,
                        presentation: RefMenuModel.presentation(
                            label: refMenu.currentLabel,
                            sidebarWidth: sidebarWidth
                        ),
                        selectedRef: $selectedRef
                    )
                }

                // Always built, faded rather than removed: appearing and
                // disappearing mid-reload is exactly the kind of churn that
                // destabilizes the enclosing toolbar item.
                ProgressView()
                    .controlSize(.small)
                    .opacity(isReloading ? 1 : 0)
            }
        }
    }

    private var list: some View {
        List(selection: $selection) {
            // The root group gets its own row, above and outside the
            // sections, so its global attributes are reachable. Its
            // *contents* are hoisted into the sections below rather than
            // nested under it: a top-level dataset is the common case, and
            // burying all of it one disclosure triangle deep would mean an
            // empty-looking sidebar on open.
            Section {
                row(for: rootNode)
            }

            if !root.coords.isEmpty {
                Section("Coordinates") {
                    ForEach(coordinateNodes) { row(for: $0) }
                }
            }

            if !root.dataVars.isEmpty {
                Section("Data variables") {
                    ForEach(dataVariableNodes) { row(for: $0) }
                }
            }

            if !root.children.isEmpty {
                Section("Groups") {
                    // Subgroups keep the full recursive treatment: each one
                    // expands to its own coords, data vars and subgroups.
                    ForEach(childGroupNodes) { node in
                        OutlineGroup(node, children: \.children) { row(for: $0) }
                    }
                }
            }
        }
        // Reports the column's width so the ref control can decide whether
        // its label fits. A background `GeometryReader` measures without
        // participating in layout, and re-reports live while the split
        // divider is dragged. `initial: true` covers the first pass, where
        // the width would otherwise stay 0 until something moved.
        .background {
            GeometryReader { proxy in
                Color.clear
                    .onChange(of: proxy.size.width, initial: true) { _, width in
                        sidebarWidth = width
                    }
            }
        }
    }

    /// The root row itself, without children -- the sections render those.
    private var rootNode: SidebarNode {
        SidebarNode(
            item: .group(path: SidebarItem.rootPath),
            title: SidebarItem.rootPath,
            subtitle: nil,
            symbol: "folder",
            children: nil
        )
    }

    private var coordinateNodes: [SidebarNode] {
        root.coords.map {
            SidebarNode.make(variable: $0, kind: .coordinate, in: SidebarItem.rootPath)
        }
    }

    private var dataVariableNodes: [SidebarNode] {
        root.dataVars.map {
            SidebarNode.make(variable: $0, kind: .dataVariable, in: SidebarItem.rootPath)
        }
    }

    private var childGroupNodes: [SidebarNode] {
        root.children.map {
            SidebarNode.make(group: $0, at: SidebarItem.childPath(SidebarItem.rootPath, $0.name))
        }
    }

    /// One outline row. The explicit `.tag` is what ties it to the `List`'s
    /// selection -- rows built inside a `ViewBuilder` (rather than from a
    /// `List(data)` collection) do not get one from their identity.
    private func row(for node: SidebarNode) -> some View {
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text(node.title)
                if let subtitle = node.subtitle {
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        } icon: {
            Image(systemName: node.symbol)
        }
        .tag(node.item)
    }
}
