//
//  DocumentView.swift
//  ndLook
//
//  The root view of a document window: owns the load state and the
//  sidebar selection, and switches between the loading, failed and loaded
//  renderings.
//
//  It takes the file URL rather than the `NDLookDocument` because the
//  document holds no state worth reading (see `NDLookDocument`): the URL
//  is the entire input the Rust core needs, and SwiftUI hands it over from
//  `ReferenceFileDocumentConfiguration.fileURL`.
//

import SwiftUI

struct DocumentView: View {
    /// Optional because SwiftUI's document configuration reports it that
    /// way -- a document can briefly exist without a backing file on disk.
    /// ndLook cannot reach that state in practice (it never creates new
    /// documents), but the type forces the case to be handled.
    let fileURL: URL?

    @State private var viewModel = DocumentViewModel()
    @State private var selection: SidebarItem?

    var body: some View {
        content
            // `.task(id:)` reruns whenever the URL changes and cancels the
            // previous run, which is exactly the reload trigger we want.
            // `load` itself returns immediately -- it owns its own task and
            // supersedes any load already in flight -- so nothing is
            // awaited here.
            .task(id: fileURL) {
                guard let fileURL else { return }
                viewModel.load(url: fileURL)
            }
            .navigationTitle(fileURL?.lastPathComponent ?? "ndLook")
            // The format is a property of what was read, not of the file
            // name, so it belongs beside the title rather than in it. Empty
            // until the summary lands, which reads as "not known yet".
            .navigationSubtitle(formatLabel)
    }

    @ViewBuilder
    private var content: some View {
        switch viewModel.phase {
        case .loading:
            // Opening a large store is slow enough to need this, and there
            // is nothing partial to show in the meantime -- the Rust core
            // returns the whole summary or an error, never a prefix.
            ProgressView("Reading dataset\u{2026}")
                .frame(maxWidth: .infinity, maxHeight: .infinity)

        case .failed(let message):
            errorCard(message)

        case .loaded(let summary):
            NavigationSplitView {
                SidebarView(root: summary.root, selection: $selection)
                    .navigationSplitViewColumnWidth(min: 200, ideal: 260)
            } detail: {
                DetailView(summary: summary, selection: selection)
            }
        }
    }

    /// The failure rendering.
    ///
    /// Every failure reaching here is already one human-readable sentence
    /// from the Rust core (see `NDLookFFI.SummaryError`), so this shows it
    /// verbatim rather than translating it. It is monospaced and selectable
    /// because the messages carry things worth copying into a bug report --
    /// library error codes, ref names, paths.
    private func errorCard(_ message: String) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.largeTitle)
                .foregroundStyle(.secondary)

            Text("Couldn't read this dataset")
                .font(.headline)

            Text(message)
                .font(.system(.body, design: .monospaced))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)

            if let fileURL {
                Text(fileURL.path(percentEncoded: false))
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var formatLabel: String {
        guard case .loaded(let summary) = viewModel.phase else { return "" }
        return summary.format.displayName
    }
}
