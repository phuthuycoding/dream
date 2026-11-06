import SwiftUI

/// Row-tap destination. TASK-004 replaces this with the real terminal
/// attach — until then it confirms the tapped session's identity.
struct TerminalPlaceholderView: View {
    let session: DreamSession

    var body: some View {
        ContentUnavailableView {
            Label(session.name, systemImage: "terminal")
        } description: {
            Text("Terminal attach lands in the next task — session \(session.id.prefix(8)) is reachable.")
        }
        .navigationTitle(session.name)
    }
}
