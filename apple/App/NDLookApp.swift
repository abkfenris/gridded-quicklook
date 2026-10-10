//
//  NDLookApp.swift
//  ndLook
//
//  The app was originally a single static window whose only job was to
//  carry the PreviewExtension app extension so macOS had something to
//  install and enable in System Settings. It now also stands on its own as
//  a viewer: a document-based app over the same Rust metadata reader the
//  Quick Look extension uses, so a dataset can be opened in a real,
//  resizable, scrollable window instead of only a preview panel.
//

import SwiftUI

@main
struct NDLookApp: App {
    var body: some Scene {
        // `DocumentGroup(viewing:)` is the read-only flavor of the document
        // scene: it gives us Open (filtered to NDLookDocument's
        // readableContentTypes), Finder double-click and "Open With"
        // routing, one window per file, and the Recents list, while omitting
        // New, Save and Save As entirely -- which is exactly right for a
        // viewer that can never write its formats back.
        DocumentGroup(viewing: NDLookDocument.self) { configuration in
            // The document itself carries no state; the file URL is the
            // whole input the metadata reader needs. See NDLookDocument
            // for why nothing is loaded at open time.
            DocumentView(fileURL: configuration.fileURL)
        }
        .commands {
            // Replaces AppKit's stock "About ndLook", which would open a
            // standard panel showing only the icon, name and version. Ours
            // also carries the Quick Look setup instructions -- the thing
            // someone is most likely hunting for when previews don't work.
            CommandGroup(replacing: .appInfo) {
                AboutWindowButton()
            }
        }

        // A `Window`, not a `WindowGroup`: About is a singleton, and
        // `Window` gives that for free -- invoking it again brings the
        // existing window forward instead of opening a second copy.
        Window("About ndLook", id: Self.aboutWindowID) {
            AboutView()
        }
        // The content has a fixed frame, so let the window take its size
        // from that rather than offering a resize handle that only adds
        // empty space.
        .windowResizability(.contentSize)
    }

    /// Shared between the menu command that opens the window and the scene
    /// that declares it; they must agree exactly or the command silently
    /// opens nothing.
    static let aboutWindowID = "about"
}

/// The About menu item.
///
/// A separate `View` rather than a plain `Button` in the `CommandGroup`
/// because opening a window needs `@Environment(\.openWindow)`, and an
/// `App` has no environment to read it from. `CommandGroup`'s content is a
/// `ViewBuilder`, and SwiftUI injects the environment into the views it
/// builds, so a one-button view is the standard way to reach the action.
private struct AboutWindowButton: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button("About ndLook") {
            openWindow(id: NDLookApp.aboutWindowID)
        }
    }
}
