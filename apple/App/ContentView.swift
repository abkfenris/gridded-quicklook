//
//  ContentView.swift
//  ndLook
//

import SwiftUI

private let supportedFormats: [(title: String, extensions: String)] = [
    ("NetCDF", "nc, nc4, cdf"),
    ("HDF5", "h5, hdf5, he5"),
    ("GRIB", "grib, grib2, grb, grb2, gb2"),
    ("Zarr store", "zarr"),
    ("Icechunk repo", "icechunk"),
]

struct ContentView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(alignment: .center, spacing: 14) {
                Image(nsImage: NSApplication.shared.applicationIconImage)
                    .resizable()
                    .interpolation(.high)
                    .frame(width: 64, height: 64)
                    .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: 6) {
                    Text("ndLook")
                        .font(.title)
                        .bold()
                    Text("A QuickLook preview extension for gridded scientific data.")
                        .foregroundStyle(.secondary)
                }
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Enable the extension")
                    .font(.headline)
                Text(
                    """
                    This app (currently) doesn't do much on its own. It provides the \
                    Quick Look preview extension. To turn previews on:
                    """
                )
                .fixedSize(horizontal: false, vertical: true)

                VStack(alignment: .leading, spacing: 4) {
                    Label("Open System Settings \u{2192} General \u{2192} Login Items & Extensions", systemImage: "1.circle")
                    Label("Find \"Quick Look\" in the extensions list", systemImage: "2.circle")
                    Label("Enable \u{201C}ndLook Preview\u{201D}", systemImage: "3.circle")
                }
                .padding(.leading, 4)
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Supported formats")
                    .font(.headline)
                ForEach(supportedFormats, id: \.title) { format in
                    HStack {
                        Text(format.title)
                            .frame(width: 100, alignment: .leading)
                            .fontWeight(.medium)
                        Text(format.extensions)
                            .foregroundStyle(.secondary)
                            .font(.system(.body, design: .monospaced))
                    }
                }
            }

            Spacer()

            Text("Once enabled, select a supported file in Finder and press Space to preview it.")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .padding(24)
        .frame(width: 480, height: 420, alignment: .topLeading)
    }
}

#Preview {
    ContentView()
}
