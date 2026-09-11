//
//  DocumentView.swift
//  ndLook
//
//  The root view of a document window. Deliberately a placeholder for now:
//  the real metadata browser (variable list, attribute tables, Icechunk
//  version picker) lands in a later step. Keeping this tiny means the
//  document plumbing in `NDLookDocument` / `NDLookApp` can be built and
//  verified on its own, before any FFI or layout work is layered on top.
//

import SwiftUI

/// Displays one open dataset.
///
/// Takes the file URL rather than the `NDLookDocument` itself because the
/// document holds no state worth reading (see `NDLookDocument`): the URL
/// is the entire input the Rust core needs, and SwiftUI hands it to us from
/// `ReferenceFileDocumentConfiguration.fileURL`.
///
/// The URL is optional because SwiftUI's document configuration reports it
/// that way -- a document can briefly exist without a backing file on disk
/// (for example while a new, never-saved document is being set up). ndLook
/// can't reach that state in practice, but the type forces us to handle it.
struct DocumentView: View {
    let fileURL: URL?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let fileURL {
                Text(fileURL.lastPathComponent)
                    .font(.title2)
                    .bold()
                Text(fileURL.path)
                    .font(.system(.footnote, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            } else {
                Text("No file")
                    .foregroundStyle(.secondary)
            }
        }
        .padding(24)
        .frame(minWidth: 480, minHeight: 320, alignment: .topLeading)
    }
}

#Preview {
    DocumentView(fileURL: URL(fileURLWithPath: "/tmp/example.nc"))
}
