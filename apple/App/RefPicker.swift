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
        // No width frame at all, deliberately. There was a `maxWidth: 120`
        // ceiling here as an overflow backstop; it was reserving the full
        // 120pt regardless of the label, which is what left a visible gap
        // between the control and the sidebar toggle. A toolbar measures an
        // item's *intrinsic* width, and a maximum-width frame reports its
        // maximum as the ideal -- so "at most 120" reads as "give me 120",
        // with the short label leading-aligned in all that space.
        //
        // It is redundant now in any case: `RefMenuModel.presentation`
        // collapses the label to an icon before the control can outgrow the
        // slot, so the measured decision is the real protection and this
        // frame was only ever a second, worse guess at the same thing.
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
            // Re-picking the ref already on screen does nothing. Assigning
            // anyway would be a visible round trip to an identical result:
            // on the default load `selectedRef` is still nil, so choosing
            // "main" would write "branch:main", change the reload key, and
            // re-read the whole repository to arrive back where it started.
            guard !row.isCurrent else { return }
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
