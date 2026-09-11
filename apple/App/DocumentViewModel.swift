//
//  DocumentViewModel.swift
//  ndLook
//
//  Owns one document window's load state: which ref is being viewed, the
//  in-flight load, and the summary (or error) that came back. The view
//  layer reads `phase` and nothing else.
//
//  Separate from the document itself on purpose. `NDLookDocument` models
//  what the *system* opened -- a URL and a content type -- and lives for as
//  long as the document does. This models what the *window* is currently
//  showing, which changes when the user picks a different Icechunk ref, and
//  which has to be re-fetched from the Rust core each time.
//

import Foundation
import Observation

@Observable
@MainActor
final class DocumentViewModel {

    /// What the window should be showing right now.
    ///
    /// A single enum rather than the usual `isLoading` / `summary` /
    /// `errorMessage` trio, because those three can express states that
    /// cannot happen (loading *and* failed, loaded with an error) and the
    /// view would have to pick a precedence between them. Here the view
    /// switches once and every case has exactly one rendering.
    enum Phase {
        case loading
        case loaded(DatasetSummary)
        case failed(String)
    }

    private(set) var phase: Phase = .loading

    /// Which version of an Icechunk repo to read, in
    /// `ndlook_summarize_json`'s `"kind:value"` form (`"branch:main"`,
    /// `"tag:v1"`, `"snapshot:ID"`), or `nil` for the default -- `main`'s
    /// tip for Icechunk, and ignored entirely for formats without version
    /// history.
    ///
    /// Set by the ref picker, which does not exist yet; `load(url:)`
    /// already reads it so that turning the picker on is a pure UI change.
    var selectedRef: String?

    /// The in-flight load, kept so a new one can cancel it.
    ///
    /// `@ObservationIgnored` because it is bookkeeping, not display state:
    /// without it, assigning the task would invalidate every view that had
    /// read anything on this object, for a change no view can see.
    @ObservationIgnored private var loadTask: Task<Void, Never>?

    /// Loads (or reloads) the dataset at `url` using the current
    /// `selectedRef`, replacing any load already running.
    ///
    /// Safe to call repeatedly -- on every `fileURL` change and on every ref
    /// change. The previous task is cancelled first so a slow load of an
    /// old ref cannot land after a fast load of a new one and leave the
    /// window showing the wrong thing.
    func load(url: URL) {
        loadTask?.cancel()
        phase = .loading

        // Read both inputs here, on the main actor, so the detached task
        // below captures two plain values instead of reaching back into
        // `self` from another isolation domain.
        let path = url.path(percentEncoded: false)
        let ref = selectedRef

        loadTask = Task { [weak self] in
            // `NDLookFFI.summarize` is synchronous and can block for a
            // long time (a large netCDF open, or spinning up a Tokio
            // runtime and walking Icechunk snapshot files). `Task.detached`
            // runs it on its own thread rather than pinning one of Swift
            // concurrency's cooperative-pool threads for the duration --
            // the same reasoning as the Quick Look render path in
            // `PreviewProvider`.
            let result = await Task.detached(priority: .userInitiated) {
                NDLookFFI.summarize(path: path, ref: ref)
            }.value

            // The FFI call cannot be interrupted mid-flight, so
            // cancellation is checked on the way out instead: a superseded
            // load must not publish its stale result over the newer one.
            guard !Task.isCancelled, let self else { return }

            switch result {
            case .success(let summary):
                self.phase = .loaded(summary)
            case .failure(let error):
                self.phase = .failed(error.message)
            }
        }
    }
}
