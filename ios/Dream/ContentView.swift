import SwiftUI

/// Root router: an active profile with a live Keychain token shows the
/// session browser; anything else (never paired, deleted profile, or a
/// token cleared after a 401) shows the pairing form — prefilled with
/// the profile when one exists.
struct ContentView: View {
    @Environment(AppState.self) private var appState

    var body: some View {
        if let profile = appState.activeProfile,
           !appState.needsRepair,
           appState.token(for: profile.id) != nil {
            SessionListView(
                profile: profile,
                client: appState.makeClient(for: profile)
            )
            // Switching profiles rebuilds the browser — fresh client,
            // fresh poll, no stale rows.
            .id(profile.id)
        } else {
            PairingView(existing: appState.activeProfile)
        }
    }
}
