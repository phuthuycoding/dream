import Foundation
import Observation

/// Observable app state. `ProfileStore` stays the source of truth; the
/// profile list and active pointer are mirrored into tracked properties
/// so SwiftUI re-renders on change. Every profile mutation funnels
/// through here to keep the mirror fresh.
@Observable
final class AppState {
    private let store: ProfileStore

    private(set) var profiles: [DaemonProfile]
    private(set) var activeProfileId: UUID?
    /// The daemon rejected the active profile's token (401) and the
    /// token was cleared. Keychain reads bypass @Observable tracking,
    /// so this flag forces the render that routes back to pairing.
    private(set) var needsRepair = false

    init(store: ProfileStore = ProfileStore()) {
        self.store = store
        profiles = store.profiles
        activeProfileId = store.activeProfileId
    }

    var activeProfile: DaemonProfile? {
        profiles.first { $0.id == activeProfileId }
    }

    func token(for profileId: UUID) -> String? {
        store.token(for: profileId)
    }

    /// A client bound to the profile's base URL. The token provider
    /// re-reads the Keychain on every request, so a cleared token stops
    /// authorizing immediately.
    func makeClient(for profile: DaemonProfile) -> APIClient {
        APIClient(baseURL: profile.baseURL) { [store] in
            store.token(for: profile.id)
        }
    }

    /// Pairing (or re-pairing) succeeded — persist the profile and its
    /// token and mark it active.
    func completePairing(profile: DaemonProfile, token: String) throws {
        try store.save(profile, token: token)
        store.activeProfileId = profile.id
        needsRepair = false
        sync()
    }

    func setActiveProfile(_ id: UUID) {
        store.activeProfileId = id
        // The repair flag describes the profile being left — a deliberate
        // switch is the escape hatch out of the re-pair screen.
        needsRepair = false
        sync()
    }

    /// Metadata-only rename — the Keychain token is untouched.
    func renameProfile(_ id: UUID, to name: String) throws {
        guard var profile = profiles.first(where: { $0.id == id }) else {
            return
        }
        profile.name = name
        try store.update(profile)
        sync()
    }

    /// Removes the profile, its Keychain token, and the active pointer.
    func deleteProfile(_ id: UUID) throws {
        try store.delete(id)
        sync()
    }

    /// A 401 from the daemon: drop the stored credential of the profile
    /// whose request was rejected — a stale in-flight request from a
    /// profile the user already left must not touch the active profile's
    /// token. The re-pair prompt only applies when the rejected profile
    /// is still the active one.
    ///
    /// `rejectedToken` is the credential the request carried: the
    /// Keychain entry is cleared only when it still holds that same
    /// value, so a stale response can't wipe a freshly re-paired token.
    func handleUnauthorized(profileId: UUID, rejectedToken: String? = nil) {
        let stored = store.token(for: profileId)
        guard stored == rejectedToken else {
            NSLog("AppState: ignored stale 401 for profile \(profileId.uuidString) — stored token differs from the rejected one")
            return
        }
        do {
            try store.clearToken(for: profileId)
        } catch {
            NSLog("AppState: failed to clear rejected token for profile \(profileId.uuidString): \(error)")
        }
        guard profileId == activeProfileId else { return }
        needsRepair = true
    }

    private func sync() {
        profiles = store.profiles
        activeProfileId = store.activeProfileId
    }
}

extension APIError {
    /// User-facing phrasing for the pairing and session screens.
    var userMessage: String {
        switch self {
        case .unauthorized:
            return "The daemon rejected the token — pair again."
        case .notFound:
            return "The session no longer exists."
        case .unreachable(let error):
            return "Can't reach the daemon: \(error.localizedDescription)"
        case .unexpectedStatus(let code):
            return "Unexpected response from the daemon (HTTP \(code))."
        case .invalidResponse:
            return "The daemon sent a response that wasn't HTTP."
        }
    }

    /// Display text for any thrown error, API or otherwise.
    static func describe(_ error: Error) -> String {
        if let apiError = error as? APIError {
            return apiError.userMessage
        }
        return error.localizedDescription
    }
}
