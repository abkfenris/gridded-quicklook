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
    }
}
