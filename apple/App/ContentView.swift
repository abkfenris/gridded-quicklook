//
//  ContentView.swift
//  GriddedQuickLook
//

import SwiftUI

private let supportedFormats: [(title: String, extensions: String)] = [
    ("NetCDF", "nc, nc4, cdf"),
    ("HDF5", "h5, hdf5, he5"),
]

struct ContentView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Gridded QuickLook")
                    .font(.title)
                    .bold()
                Text("A QuickLook preview extension for gridded scientific data.")
                    .foregroundStyle(.secondary)
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Enable the extension")
                    .font(.headline)
                Text(
                    """
                    This app doesn't do anything on its own -- it just carries the \
                    Quick Look preview extension. To turn previews on:
                    """
                )
                .fixedSize(horizontal: false, vertical: true)

                VStack(alignment: .leading, spacing: 4) {
                    Label("Open System Settings \u{2192} General \u{2192} Login Items & Extensions", systemImage: "1.circle")
                    Label("Find \"Quick Look\" in the extensions list", systemImage: "2.circle")
                    Label("Enable \u{201C}Gridded QuickLook Preview\u{201D}", systemImage: "3.circle")
                }
                .padding(.leading, 4)
            }

            VStack(alignment: .leading, spacing: 8) {
                Text("Supported formats")
                    .font(.headline)
                ForEach(supportedFormats, id: \.title) { format in
                    HStack {
                        Text(format.title)
                            .frame(width: 80, alignment: .leading)
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
