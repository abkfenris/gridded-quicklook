//
//  GriddedQuickLookApp.swift
//  GriddedQuickLook
//
//  The host app is deliberately minimal: its only real job is to carry
//  the PreviewExtension app extension so macOS has something to install
//  and enable in System Settings. See ContentView for the instructions
//  shown to the user.
//

import SwiftUI

@main
struct GriddedQuickLookApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
        .windowResizability(.contentSize)
    }
}
