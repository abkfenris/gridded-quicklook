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

    /// True while a *replacement* load is running -- one that started while
    /// a summary was already on screen, i.e. an Icechunk ref switch.
    ///
    /// This exists so a ref switch does not blank the window back to a
    /// spinner. `phase` stays `.loaded` with the old summary and the view
    /// shows a small toolbar spinner instead, which keeps the sidebar's
    /// scroll position and selection visible across the switch. Keeping it
    /// as a separate flag rather than adding a `.reloading(DatasetSummary)`
    /// case means the view's `phase` switch is untouched -- the reload is
    /// genuinely orthogonal to which of the three states we are in.
    private(set) var isReloading = false

    /// Which version of an Icechunk repo to read, in
    /// `ndlook_summarize_json`'s `"kind:value"` form (`"branch:main"`,
    /// `"tag:v1"`, `"snapshot:ID"`), or `nil` for the default -- `main`'s
    /// tip for Icechunk, and ignored entirely for formats without version
    /// history.
    ///
    /// Set by the toolbar ref picker. The view reloads on a change by
    /// folding this into its `.task(id:)` key, so nothing here has to
    /// trigger the reload itself.
    var selectedRef: String?

    /// The version metadata from this document's *default* load, kept to
    /// populate the ref menu.
    ///
    /// The Rust reader walks ancestry backwards from whichever ref was
    /// asked for, so the `VersionInfo` that comes back with a tag or an old
    /// snapshot describes only that ref's history. Driving the menu from
    /// the live value therefore makes the menu shrink as you navigate --
    /// select an older snapshot and the newer ones you came from disappear,
    /// with no way back. Pinning the first default load's metadata keeps
    /// the menu a stable picture of the repository.
    ///
    /// Only the menu's *contents* come from here. The checkmark and the
    /// control's label still track the live summary, because those are
    /// meant to say what is on screen right now.
    private(set) var pinnedVersionInfo: VersionInfo?

    /// The URL `pinnedVersionInfo` belongs to, so the pin can be dropped if
    /// the window is ever pointed at a different document.
    @ObservationIgnored private var pinnedURL: URL?

    /// The last ref that loaded successfully, so a failed switch can be
    /// rolled back to something that works.
    @ObservationIgnored private var lastGoodRef: String?

    /// Set when a *reload* fails while a summary is already on screen.
    ///
    /// Reported alongside the surviving dataset rather than replacing it.
    /// Replacing it was a dead end: the error card takes over the whole
    /// window, taking the ref picker with it, while `selectedRef` still
    /// holds the ref that just failed -- so the reload key never changes,
    /// nothing retries, and the last good summary is gone. Keeping the tree
    /// up means the picker stays reachable and another ref is one click
    /// away.
    var reloadError: String?

    /// Dismisses the reload-failure notice. The dataset on screen is
    /// unaffected -- it was never replaced.
    func dismissReloadError() {
        reloadError = nil
    }

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

        // A pin describes one repository's history; pointing the window at
        // a different document invalidates it -- and so does the ref, which
        // names a branch/tag/snapshot in the *old* repository.
        //
        // Clearing `selectedRef` alongside the pin is load-bearing, not
        // tidiness. The pin is only ever taken from a default (`ref == nil`)
        // load, so leaving a stale ref set means no load can ever re-pin:
        // the ref menu never comes back for the life of the window.
        if pinnedURL != url {
            pinnedURL = url
            pinnedVersionInfo = nil
            selectedRef = nil
            lastGoodRef = nil
            reloadError = nil
        }

        // A first load has nothing to show, so it blanks to a spinner; a
        // reload keeps the previous summary up (see `isReloading`). A
        // reload *after a failure* blanks too -- there is no stale tree to
        // preserve in that case, only an error card.
        if case .loaded = phase {
            isReloading = true
        } else {
            phase = .loading
            isReloading = false
        }

        // Read both inputs here, on the main actor, so the detached task
        // below captures two plain values instead of reaching back into
        // `self` from another isolation domain.
        let path = url.path(percentEncoded: false)
        let ref = selectedRef

        // Clear a previous failure only when this load is a genuine move to
        // a different ref. Rolling `selectedRef` back after a failure itself
        // triggers a reload of the last good ref, and clearing on *that*
        // would wipe the message before anyone could read it -- the error
        // would flash and vanish. Comparing against `lastGoodRef` tells the
        // two apart: a rollback reloads what already worked, a real switch
        // does not.
        if ref != lastGoodRef {
            reloadError = nil
        }

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
            // A cancelled load leaves `isReloading` set on purpose: the
            // load that superseded it is still running, and it is the one
            // that will clear the flag.
            guard !Task.isCancelled, let self else { return }

            self.isReloading = false

            switch result {
            case .success(let summary):
                // Pin the repository's history the first time we see any.
                // A default (`ref == nil`) load is preferred, because it
                // walks back from the repository's own tip and so sees the
                // fullest ancestry -- but any successful load is better than
                // no menu at all, which is what a window opened straight
                // onto a non-default ref would otherwise get.
                if self.pinnedVersionInfo == nil, let versionInfo = summary.versionInfo {
                    self.pinnedVersionInfo = versionInfo
                }
                self.lastGoodRef = ref
                self.phase = .loaded(summary)

            case .failure(let error):
                if case .loaded = self.phase {
                    // A ref switch that failed. Keep the dataset that is
                    // already on screen and report the failure beside it:
                    // replacing the window with an error card would take the
                    // ref picker down too, stranding the user on a ref that
                    // cannot load. Rolling `selectedRef` back to the last
                    // good value also restores the reload key, so picking
                    // the same ref again genuinely retries.
                    self.reloadError = error.message
                    self.selectedRef = self.lastGoodRef
                } else {
                    // Nothing on screen to preserve -- the error card is the
                    // whole story.
                    self.phase = .failed(error.message)
                }
            }
        }
    }
}
