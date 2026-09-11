//
//  NDLookFFI.swift
//  ndLook
//
//  The app's single point of contact with `ndlook-ffi`'s C ABI. Keeping
//  the unsafe pointer handling and the JSON envelope shape in one place
//  means the view layer only ever sees Swift values and Swift errors.
//
//  The C functions are declared in `apple/include/ndlook_ffi.h`, which
//  cargo's build.rs regenerates and `App/BridgingHeader.h` imports.
//

import Foundation

/// Namespace for the Rust core's C entry points.
///
/// An uninhabited `enum` rather than a `struct` or `class`: there is no
/// instance state and nothing to construct, and an empty enum makes that
/// uninstantiable at the type level instead of by convention.
enum NDLookFFI {

    /// Everything that can go wrong on this boundary, already reduced to
    /// one displayable sentence.
    ///
    /// Every failure -- bad path, malformed ref, unsupported format,
    /// unreadable file, even a Rust panic -- arrives from the FFI as a
    /// single message string, so there is nothing to model beyond that
    /// string. It is a named type only because `Result`'s failure type must
    /// conform to `Error` and `String` does not; retroactively conforming
    /// the standard library's `String` to `Error` to avoid this wrapper
    /// would be a project-wide change to a very common type in exchange for
    /// saving one `.message`.
    struct SummaryError: Error, LocalizedError, Hashable {
        let message: String

        init(_ message: String) {
            self.message = message
        }

        var errorDescription: String? { message }
    }

    /// Summarizes the dataset at `path`, returning either the decoded
    /// summary or a human-readable message describing what went wrong.
    ///
    /// **This call is synchronous and blocking, and can take a long time.**
    /// It opens the underlying file or store: a large netCDF read, or
    /// spinning up a Tokio runtime and walking Icechunk snapshot files.
    /// Call it from `Task.detached` (as `PreviewProvider` does for the
    /// render path) so it runs on its own thread rather than pinning one of
    /// Swift concurrency's cooperative-pool threads -- never from the main
    /// actor.
    ///
    /// - Parameters:
    ///   - path: Filesystem path to the file, Zarr store, or Icechunk repo.
    ///   - ref: Which version of an Icechunk repo to read, as
    ///     `"branch:NAME"`, `"tag:NAME"`, or `"snapshot:ID"`. `nil`
    ///     requests the default, which is `main`'s tip for an Icechunk repo
    ///     and is simply ignored for anything without version history.
    /// - Returns: `.success` with the decoded summary, or `.failure`
    ///   carrying a message suitable for showing to the user.
    static func summarize(path: String, ref: String?) -> Result<DatasetSummary, SummaryError> {
        // Two nested `withCString` bodies rather than one: the ref is
        // optional, and the borrowed pointers are only valid inside their
        // own closures, so the call has to happen at the innermost level.
        // Factored through `callSummarize` so the NULL-ref and non-NULL-ref
        // paths share the identical body.
        path.withCString { pathPointer -> Result<DatasetSummary, SummaryError> in
            guard let ref else {
                return callSummarize(pathPointer, nil)
            }
            return ref.withCString { refPointer -> Result<DatasetSummary, SummaryError> in
                callSummarize(pathPointer, refPointer)
            }
        }
    }

    /// Performs the C call and decodes its result envelope.
    ///
    /// The two pointer parameters are borrowed from enclosing
    /// `withCString` closures and must not escape this function -- which
    /// they do not: the C side copies whatever it needs before returning,
    /// and everything this returns is a Swift value.
    private static func callSummarize(
        _ path: UnsafePointer<CChar>,
        _ ref: UnsafePointer<CChar>?
    ) -> Result<DatasetSummary, SummaryError> {
        // Documented to be non-null always, but a NULL here would be a
        // crash rather than a diagnosable bug, so it gets a branch.
        guard let cString = ndlook_summarize_json(path, ref) else {
            return .failure(SummaryError("ndlook-ffi returned no data."))
        }
        // The Rust side hands over ownership of this allocation; it must be
        // released exactly once, on every path out of this function.
        defer { ndlook_free_string(cString) }

        // `String(cString:)` copies, so the bytes are safe once the defer
        // above runs. Re-encoding to UTF-8 `Data` for JSONDecoder is a
        // second copy, but the payload is metadata (kilobytes), not data.
        let json = Data(String(cString: cString).utf8)
        return decodeEnvelope(json)
    }

    /// Unwraps `ndlook_summarize_json`'s result envelope.
    ///
    /// The contract is a JSON object with exactly one of two keys:
    /// `{"summary": <DatasetSummary>}` on success, `{"error": "<message>"}`
    /// on any failure. Both are tried, and anything that matches neither is
    /// itself reported as an error -- an unparsable envelope means the
    /// Swift model and the Rust model have drifted apart, which the user
    /// should see rather than have swallowed.
    private static func decodeEnvelope(_ json: Data) -> Result<DatasetSummary, SummaryError> {
        let decoder = JSONDecoder()

        if let envelope = try? decoder.decode(ErrorEnvelope.self, from: json) {
            return .failure(SummaryError(envelope.error))
        }

        // Not `try?`: when the payload is neither envelope, the decoding
        // error is the only clue about *how* the Swift and Rust models
        // drifted, and "could not decode" with no detail is close to
        // useless. Attempting the error envelope first (above) keeps a
        // genuine Rust-side error message from being reported as a
        // decoding failure.
        do {
            return .success(try decoder.decode(SummaryEnvelope.self, from: json).summary)
        } catch {
            return .failure(
                SummaryError("Could not decode the summary returned by ndlook-ffi: \(error)")
            )
        }
    }

    /// `{"summary": <DatasetSummary>}` -- the success half of the envelope.
    private struct SummaryEnvelope: Decodable {
        let summary: DatasetSummary
    }

    /// `{"error": "<message>"}` -- the failure half of the envelope.
    private struct ErrorEnvelope: Decodable {
        let error: String
    }
}
