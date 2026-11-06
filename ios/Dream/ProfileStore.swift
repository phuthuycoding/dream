import Foundation

/// Persists daemon pairing profiles and their active pointer as JSON
/// under Application Support. Bearer tokens are handled separately by
/// `KeychainTokenStore` — only profile metadata ever reaches disk.
final class ProfileStore {
    /// On-disk shape: profile metadata plus the active pointer. Tokens
    /// are absent by construction.
    private struct PersistedState: Codable {
        var profiles: [DaemonProfile]
        var activeProfileId: UUID?
    }

    private(set) var profiles: [DaemonProfile] = []
    /// The profile the app connects through. Persisted on every change;
    /// a setter cannot throw, so a failed write is logged rather than
    /// propagated — in-memory state stays correct either way.
    var activeProfileId: UUID? {
        didSet { persistOrLog() }
    }

    private let keychain: KeychainTokenStore
    private let fileURL: URL

    init(
        keychain: KeychainTokenStore = KeychainTokenStore(),
        fileURL: URL? = nil
    ) {
        self.keychain = keychain
        self.fileURL = fileURL ?? ProfileStore.defaultFileURL()
        load()
    }

    /// Insert a new profile or update an existing one (matched by `id`);
    /// either way its Keychain token is written first so a metadata-only
    /// failure never leaves a listed profile without credentials.
    func save(_ profile: DaemonProfile, token: String) throws {
        try keychain.setToken(token, for: profile.id.uuidString)
        if let index = profiles.firstIndex(where: { $0.id == profile.id }) {
            profiles[index] = profile
        } else {
            profiles.append(profile)
        }
        try persist()
    }

    /// Remove a profile, its Keychain token, and any active pointer to
    /// it. The token is deleted first so a metadata write failure leaves
    /// a retryable profile rather than a orphaned secret.
    func delete(_ id: UUID) throws {
        try keychain.deleteToken(for: id.uuidString)
        profiles.removeAll { $0.id == id }
        if activeProfileId == id {
            activeProfileId = nil
        }
        try persist()
    }

    func token(for id: UUID) -> String? {
        keychain.token(for: id.uuidString)
    }

    // MARK: - Persistence

    private static func defaultFileURL() -> URL {
        let support = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        )[0]
        return support
            .appendingPathComponent("Dream", isDirectory: true)
            .appendingPathComponent("profiles.json")
    }

    private func load() {
        guard let data = try? Data(contentsOf: fileURL) else {
            return // first launch — nothing persisted yet
        }
        do {
            let state = try JSONDecoder().decode(PersistedState.self, from: data)
            profiles = state.profiles
            activeProfileId = state.activeProfileId
        } catch {
            NSLog("ProfileStore: ignoring undecodable profile file at \(fileURL.path): \(error)")
        }
    }

    private func persist() throws {
        let state = PersistedState(profiles: profiles, activeProfileId: activeProfileId)
        let data = try JSONEncoder().encode(state)
        try FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try data.write(to: fileURL, options: .atomic)
    }

    private func persistOrLog() {
        do {
            try persist()
        } catch {
            NSLog("ProfileStore: failed to persist profiles to \(fileURL.path): \(error)")
        }
    }
}
