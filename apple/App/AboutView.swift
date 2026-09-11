//
//  AboutView.swift
//  ndLook
//
//  The About window, which is also where the app's onboarding lives.
//
//  This absorbs what used to be `ContentView` -- the app's single static
//  window back when it existed only to carry the Quick Look extension.
//  Now that the app opens documents, a `WindowGroup` full of instructions
//  is no longer the right front door: the front door is the Open dialog.
//  But the instructions still matter, because installing the app is not
//  enough to get previews working -- the extension has to be enabled by
//  hand in System Settings -- so they moved here, where someone looking for
//  "what is this and why don't previews work" will actually go.
//

import AppKit
import SwiftUI

/// The formats the Rust core can read, and the extensions each is
/// recognized by. Mirrors the UTI tag specifications in `App/Info.plist`.
private let supportedFormats: [(title: String, extensions: String)] = [
    ("NetCDF", "nc, nc4, cdf"),
    ("HDF5", "h5, hdf5, he5"),
    ("GRIB", "grib, grib2, grb, grb2, gb2"),
    ("Zarr store (folder)", "zarr"),
    ("Icechunk repo (folder)", "icechunk"),
]

struct AboutView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            header

            VStack(alignment: .leading, spacing: 8) {
                Text("Enable Quick Look previews")
                    .font(.headline)
                Text(
                    """
                    ndLook also installs a Quick Look preview extension, so \
                    supported files can be previewed straight from the Finder. \
                    macOS requires you to turn it on by hand:
                    """
                )
                .fixedSize(horizontal: false, vertical: true)

                VStack(alignment: .leading, spacing: 4) {
                    Label(
                        "Open System Settings \u{2192} General \u{2192} Login Items & Extensions",
                        systemImage: "1.circle"
                    )
                    Label("Find \"Quick Look\" in the extensions list", systemImage: "2.circle")
                    Label("Enable \u{201C}ndLook Preview\u{201D}", systemImage: "3.circle")
                }
                .padding(.leading, 4)

                Button("Open Extension Settings") {
                    Self.openExtensionSettings()
                }
                .padding(.top, 2)
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Supported formats")
                    .font(.headline)
                ForEach(supportedFormats, id: \.title) { format in
                    HStack(alignment: .firstTextBaseline) {
                        Text(format.title)
                            .frame(width: 150, alignment: .leading)
                            .fontWeight(.medium)
                        Text(format.extensions)
                            .foregroundStyle(.secondary)
                            .font(.system(.body, design: .monospaced))
                    }
                }
            }

            Spacer(minLength: 0)

            Text("Open a file from the File menu, or select one in the Finder and press Space.")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .padding(24)
        .frame(width: 520, height: 560, alignment: .topLeading)
    }

    // MARK: - Header

    private var header: some View {
        HStack(alignment: .center, spacing: 16) {
            // The icon the system is actually showing for this app right
            // now, rather than a hard-coded asset name: it stays correct if
            // the icon set is ever renamed or re-slotted.
            if let icon = NSApplication.shared.applicationIconImage {
                Image(nsImage: icon)
                    .resizable()
                    .frame(width: 72, height: 72)
            }

            VStack(alignment: .leading, spacing: 4) {
                Text(Self.appName)
                    .font(.title)
                    .bold()
                Text("A viewer and Quick Look preview extension for gridded scientific data.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text(Self.versionSummary)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
            }
        }
    }

    // MARK: - Bundle metadata

    private static var appName: String {
        let info = Bundle.main.infoDictionary
        return info?["CFBundleDisplayName"] as? String
            ?? info?["CFBundleName"] as? String
            ?? "ndLook"
    }

    /// `Version 1.0 (1)` -- the marketing version with the build number,
    /// which is what a bug report needs to identify a build.
    private static var versionSummary: String {
        let info = Bundle.main.infoDictionary
        let version = info?["CFBundleShortVersionString"] as? String ?? "unknown"
        let build = info?["CFBundleVersion"] as? String ?? "unknown"
        return "Version \(version) (\(build))"
    }

    // MARK: - Settings

    /// Opens the Login Items & Extensions pane, where the Quick Look
    /// extension is enabled.
    ///
    /// The deep link is a System Settings URL scheme, which is not API and
    /// has been renamed before (this spelling is the Ventura-and-later one;
    /// the floor here is macOS 15). `NSWorkspace.open` reports whether it
    /// resolved, so a future rename degrades to opening System Settings at
    /// whatever pane it last showed -- one extra click for the user --
    /// rather than to a button that silently does nothing.
    private static func openExtensionSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension"),
           NSWorkspace.shared.open(url) {
            return
        }

        let settings = URL(fileURLWithPath: "/System/Applications/System Settings.app")
        NSWorkspace.shared.openApplication(at: settings, configuration: .init())
    }
}

#Preview {
    AboutView()
}
