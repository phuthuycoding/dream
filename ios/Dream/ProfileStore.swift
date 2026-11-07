import Foundation

/// Profile validation failures surfaced to the pairing/rename forms.
enum ProfileError: Error, Equatable, LocalizedError {
    /// Another profile already holds that name — profiles are named
    /// user-facing labels, so a collision misleads.
    case duplicateName(String)

    var errorDescription: String? {
        switch self {
        case .duplicateName(let name):
            return "A daemon named \"\(name)\" already exists — pick another name."
        }
    }
}

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
    /// Set when the persisted file existed, would not decode, and the
    /// quarantine move failed — persist() must refuse rather than
    /// overwrite the only copy.
    private var persistenceBlocked = false

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
    /// failure never leaves a listed profile without credentials. A name
    /// another profile already uses is rejected before any write.
    func save(_ profile: DaemonProfile, token: String) throws {
        try checkUniqueName(profile)
        try keychain.setToken(token, for: profile.id.uuidString)
        if let index = profiles.firstIndex(where: { $0.id == profile.id }) {
            profiles[index] = profile
        } else {
            profiles.append(profile)
        }
        try persist()
    }

    /// Update profile metadata without touching the Keychain token —
    /// rename has no new credential to write. Unknown ids are a no-op.
    /// A name another profile already uses is rejected.
    func update(_ profile: DaemonProfile) throws {
        guard let index = profiles.firstIndex(where: { $0.id == profile.id }) else {
            return
        }
        try checkUniqueName(profile)
        profiles[index] = profile
        try persist()
    }

    /// Profile names are the labels the user picks a daemon by — a
    /// duplicate silently misidentifies the target. The profile's own
    /// id is excluded so re-saving itself never self-collides.
    private func checkUniqueName(_ profile: DaemonProfile) throws {
        if Self.hasDuplicateName(profile, among: profiles) {
            throw ProfileError.duplicateName(profile.name)
        }
    }

    /// The duplicate-name predicate `checkUniqueName` enforces, pure
    /// and internal so a unit test can pin it without touching disk or
    /// the Keychain.
    static func hasDuplicateName(_ profile: DaemonProfile, among profiles: [DaemonProfile]) -> Bool {
        profiles.contains { $0.id != profile.id && $0.name == profile.name }
    }

    /// Drop a profile's Keychain token while keeping the profile — the
    /// 401 path clears the credential so the user can re-pair.
    func clearToken(for id: UUID) throws {
        try keychain.deleteToken(for: id.uuidString)
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

    /// Where an undecodable file is moved: a sibling carrying a
    /// `corrupt-<unix-timestamp>` extension, so a second corruption
    /// never overwrites a previous quarantine.
    static func quarantineURL(for fileURL: URL, at date: Date) -> URL {
        fileURL.appendingPathExtension("corrupt-\(Int(date.timeIntervalSince1970))")
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
            // An undecodable file must be moved aside before the store
            // starts empty — the next save/write would otherwise
            // overwrite it and orphan the Keychain tokens of every
            // profile it held.
            let quarantined = Self.quarantineURL(for: fileURL, at: Date())
            do {
                try FileManager.default.moveItem(at: fileURL, to: quarantined)
                NSLog("ProfileStore: quarantined undecodable profile file as \(quarantined.lastPathComponent): \(error)")
            } catch {
                persistenceBlocked = true
                NSLog("ProfileStore: undecodable profile file at \(fileURL.path) could not be quarantined: \(error)")
            }
        }
    }

    private func persist() throws {
        guard !persistenceBlocked else {
            throw NSError(
                domain: "ProfileStore", code: 1,
                userInfo: [NSLocalizedDescriptionKey:
                    "refusing to overwrite the unreadable profiles file at \(fileURL.path)"]
            )
        }
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
