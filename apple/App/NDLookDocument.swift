//
//  NDLookDocument.swift
//  ndLook
//
//  The document model behind the app's DocumentGroup scene. ndLook is a
//  *viewer*, not an editor: this type exists so SwiftUI will give us the
//  standard document lifecycle (Open dialogs filtered to our types, the
//  Finder's "Open With" and double-click routing, one window per file,
//  recent documents, the proxy icon in the title bar) without ever
//  claiming the ability to write anything back.
//
//  Why `ReferenceFileDocument` rather than the value-type `FileDocument`:
//  `FileDocument`'s contract is "decode the whole file into a value in
//  `init(configuration:)`". Our data sources include multi-gigabyte netCDF
//  files and Zarr/Icechunk stores that are entire directory trees; loading
//  them eagerly (and copying them around as a struct) is exactly the wrong
//  shape. `ReferenceFileDocument` is a class, so the document can hold a
//  URL now and load metadata lazily and asynchronously later, and can
//  publish that state to the UI as it arrives.
//

import SwiftUI
import UniformTypeIdentifiers

/// A read-only document representing one gridded dataset on disk.
///
/// The document deliberately holds no dataset state yet -- opening a file
/// only establishes *which* file we are looking at. The actual metadata is
/// fetched from the Rust core (`ndlook_summarize_json`) by the view
/// layer, which knows the URL from
/// `ReferenceFileDocumentConfiguration.fileURL`.
final class NDLookDocument: ReferenceFileDocument {

    /// The five types the app can open, matching the UTI declarations in
    /// `App/Info.plist`.
    ///
    /// The `importedAs:` / `exportedAs:` split mirrors that plist exactly,
    /// and it matters: these initializers look the identifier up in the
    /// bundle's declarations and trap if the declaration is missing or
    /// declared with the opposite ownership. netCDF/HDF5/GRIB are
    /// *imported* (this project mints the identifiers, but does not claim
    /// to own those formats); Zarr and Icechunk are *exported* (directory
    /// stores whose UTIs this project does own, badge icons and all).
    static let readableContentTypes: [UTType] = [
        UTType(importedAs: "com.alexkerney.ndlook.netcdf"),
        UTType(importedAs: "com.alexkerney.ndlook.hdf5"),
        UTType(importedAs: "com.alexkerney.ndlook.grib"),
        UTType(exportedAs: "com.alexkerney.ndlook.zarr"),
        UTType(exportedAs: "com.alexkerney.ndlook.icechunk"),
    ]

    /// Empty on purpose: ndLook never saves. An empty
    /// `writableContentTypes` keeps SwiftUI from offering Save, Save As,
    /// Duplicate, or autosave-driven writes, and pairs with the
    /// `DocumentGroup(viewing:)` scene in `NDLookApp`.
    static var writableContentTypes: [UTType] { [] }

    /// Opens a document without reading its bytes.
    ///
    /// We intentionally never touch `configuration.file`. SwiftUI hands us
    /// a `FileWrapper`, and for our directory-backed types (`.zarr`,
    /// `.icechunk`) that wrapper is a *directory* wrapper: merely asking it
    /// for its contents walks and materializes a tree that can hold
    /// hundreds of thousands of chunk files and hundreds of gigabytes of
    /// data. Even for plain files, reading the whole thing into memory here
    /// would duplicate work the Rust reader does far more selectively.
    ///
    /// Everything downstream works from the file URL instead, which SwiftUI
    /// surfaces separately as
    /// `ReferenceFileDocumentConfiguration.fileURL` -- so this initializer
    /// has nothing left to do but succeed.
    init(configuration: ReadConfiguration) throws {
        // Intentionally empty; see the note above about `configuration.file`.
    }

    /// No snapshot state to capture: saving is unsupported, and
    /// `Snapshot` only exists to be handed to `fileWrapper(snapshot:_:)`.
    typealias Snapshot = Void

    func snapshot(contentType: UTType) throws -> Snapshot {
        ()
    }

    /// Always throws. `ReferenceFileDocument` requires a serialization
    /// entry point even for viewer-only documents; with
    /// `writableContentTypes` empty, SwiftUI should never call this, so
    /// reaching it means something asked for a write we cannot honor.
    /// `.featureUnsupported` is the closest Cocoa error for "this document
    /// type does not write".
    func fileWrapper(snapshot: Snapshot, configuration: WriteConfiguration) throws -> FileWrapper {
        throw CocoaError(.featureUnsupported)
    }
}
