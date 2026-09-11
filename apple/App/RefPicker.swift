//
//  RefPicker.swift
//  ndLook
//
//  The control for choosing which version of an Icechunk repository to
//  view: a branch, a tag, or a bare snapshot from the repository's history.
//
//  Only Icechunk has version history, so this is the one piece of the UI
//  that is format-specific; `SidebarView` contributes it to the sidebar's
//  section of the titlebar. It belongs on the sidebar side because what it
//  changes is the tree directly beneath it -- the ref decides which
//  variables exist at all.
//
//  Purely a renderer: what the menu contains, which row is checked, and how
//  each row reads are all decided by `RefMenuModel`, which is testable
//  without a view. Anything resembling a decision belongs there, not here.
//

import SwiftUI

/// Menu listing every branch, tag and recent snapshot in the repository.
struct RefPicker: View {
    let menu: RefMenuModel
    /// Whether the ref name fits beside the icon. Decided by
    /// `RefMenuModel.presentation` from the sidebar's measured width, not
    /// by the layout system: a toolbar asks an item how wide it wants to be
    /// and never proposes a width back, so no amount of `ViewThatFits` or
    /// truncation can make the control notice it is running out of room.
    let presentation: RefControlPresentation
    @Binding var selectedRef: String?

    var body: some View {
        Menu {
            ForEach(menu.sections) { section in
                Section(section.title) {
                    ForEach(section.rows) { row in
                        entry(row)
                    }
                    if section.showsTruncationNote {
                        // Disabled because there is nothing to select -- it
                        // is a note, not an option.
                        Button("Older history not shown") {}
                            .disabled(true)
                    }
                }
            }
        } label: {
            // Middle truncation, not tail: the informative part of a long
            // ref name is usually at both ends (a dated branch, a
            // slash-namespaced tag), and a snapshot id is opaque enough
            // that losing the middle costs nothing.
            controlLabel
        }
        // `.button`, not `.borderlessButton`: the borderless pop-up style
        // sizes itself from its widest *menu row*, and the snapshot rows
        // carry an id, a message and a date. That is what pushed the whole
        // control into the toolbar's overflow menu once a snapshot was
        // selected -- the control was being measured against content the
        // user never sees until the menu opens. The button style sizes from
        // the label instead, which `RefMenuModel.controlLabel` already caps.
        .menuStyle(.button)
        .buttonStyle(.borderless)
        // A maximum, deliberately not a fixed width. A fixed frame *demands*
        // its width whether or not the label needs it, which is how a 150pt
        // frame ended up in overflow even on "main": the sidebar's slot
        // between the traffic lights and the toggle is narrower than that.
        // A maximum only clamps, so a short label still measures short and
        // the item keeps fitting.
        .frame(maxWidth: 120, alignment: .leading)
        // Collapsed, the icon alone says only "a ref" -- the tooltip is
        // where the name goes so it stays discoverable.
        .help(
            presentation == .fullLabel
                ? "Choose which version of this repository to view"
                : "Viewing \(menu.currentLabel) \u{2014} choose another version"
        )
    }

    /// The control's own face.
    ///
    /// Two branches rather than a ternary on `.labelStyle`, because
    /// `.titleAndIcon` and `.iconOnly` are different types and cannot be
    /// selected between in an expression.
    @ViewBuilder
    private var controlLabel: some View {
        let label = Label(menu.currentLabel, systemImage: menu.currentKind.symbol)
            .lineLimit(1)
            .truncationMode(.middle)

        if presentation == .fullLabel {
            label.labelStyle(.titleAndIcon)
        } else {
            label.labelStyle(.iconOnly)
        }
    }

    /// One selectable ref. `isCurrent` is decided by `RefMenuModel` against
    /// what is actually on screen, not against `selectedRef` -- which starts
    /// `nil`, meaning "whatever the default resolves to".
    private func entry(_ row: RefMenuModel.Row) -> some View {
        Button {
            selectedRef = row.ref
        } label: {
            if row.isCurrent {
                Label(row.label, systemImage: "checkmark")
            } else {
                Text(row.label)
            }
        }
    }
}
