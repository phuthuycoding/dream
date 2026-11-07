import SwiftUI

@main
struct DreamApp: App {
    @State private var appState = AppState()
    @State private var attentionCenter = AttentionCenter()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(appState)
                .environment(attentionCenter)
        }
    }
}
