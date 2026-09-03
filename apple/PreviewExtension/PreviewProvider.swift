//
//  PreviewProvider.swift
//  PreviewExtension
//
//  The QuickLook preview app extension's entry point. This is a
//  data-based preview: rather than handing QuickLook a file URL to render
//  itself (`QLPreviewReply(fileURL:)`), we render the HTML ourselves (via
//  `gridlook-ffi`, the Rust core) and hand back the bytes directly.
//

import Foundation
import OSLog
import QuickLookUI
import UniformTypeIdentifiers

private let logger = Logger(subsystem: "dev.gridlook.quicklook", category: "preview")

@objc(PreviewProvider)
final class PreviewProvider: QLPreviewProvider, QLPreviewingController {

    /// The size QuickLook should allocate for the preview panel. The HTML
    /// itself is responsive, but QuickLook still wants a hint up front.
    private static let previewContentSize = CGSize(width: 800, height: 600)

    override init() {
        super.init()
        logger.info("PreviewProvider initialized")
    }

    func providePreview(for request: QLFilePreviewRequest) async throws -> QLPreviewReply {
        logger.info("providePreview called for \(request.fileURL.path, privacy: .public)")
        let html = renderHTML(forFileAt: request.fileURL)
        logger.info("rendered \(html.utf8.count, privacy: .public) bytes of HTML")

        let reply = QLPreviewReply(
            dataOfContentType: .html,
            contentSize: Self.previewContentSize
        ) { _ in
            Data(html.utf8)
        }
        reply.title = request.fileURL.lastPathComponent
        return reply

        // Fallback for reference: if we ever wanted QuickLook to render
        // the file itself (e.g. handing it a pre-rendered HTML file on
        // disk) instead of data we produce inline, the file-based reply
        // looks like this:
        //
        //   return QLPreviewReply(fileURL: someRenderedHTMLFileURL)
        //
        // We don't use it: gridlook-ffi renders straight to an in-memory
        // string, so there's no intermediate file to hand QuickLook.
    }

    /// Calls into `gridlook-ffi`'s C ABI to render `url` as a complete HTML
    /// document. `gridlook_render_html` never fails in the FFI sense -- on
    /// any error it returns a styled HTML error card instead of a distinct
    /// error code -- so there is no Swift-side error branch to write here.
    private func renderHTML(forFileAt url: URL) -> String {
        url.withUnsafeFileSystemRepresentation { fsPath -> String in
            guard let fsPath else {
                return Self.fallbackErrorHTML
            }
            guard let cString = gridlook_render_html(fsPath) else {
                return Self.fallbackErrorHTML
            }
            defer { gridlook_free_string(cString) }
            return String(cString: cString)
        }
    }

    /// Used only if `gridlook_render_html` itself returns NULL, which its
    /// documented contract says it never does; kept as a last-resort
    /// safety net so a Swift-side surprise still produces *some* preview
    /// rather than a crash or a blank panel.
    private static let fallbackErrorHTML =
        "<!doctype html><html><body><p>Preview unavailable: gridlook-ffi returned no data.</p></body></html>"
}
