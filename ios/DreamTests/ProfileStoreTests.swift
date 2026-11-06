import Security
import XCTest
@testable import Dream

/// Profile storage: profile metadata persists across store instances and
/// the bearer token lives only in the Keychain — never in UserDefaults,
/// files, or the encoded profile itself.
final class ProfileStoreTests: XCTestCase {
    private var store: ProfileStore!
    /// Profiles created by the running test, deleted on teardown.
    private var createdProfileIDs: [UUID] = []
    /// Tokens issued by the running test so teardown can scrub leftover
    /// Keychain items even when deletion itself is under test.
    private var issuedTokens: Set<String> = []

    override func setUp() {
        super.setUp()
        store = ProfileStore()
    }

    override func tearDownWithError() throws {
        for id in createdProfileIDs {
            try store.delete(id)
        }
        createdProfileIDs.removeAll()
        purgeIssuedTokensFromKeychain()
        store = nil
        try super.tearDownWithError()
    }

    // MARK: - Save + persistence

    func testSavingProfileListsItAndStoresTokenInKeychain() throws {
        let profile = makeProfile(name: "mac-studio")
        let token = issueToken()

        try store.save(profile, token: token)

        guard let listed = store.profiles.first(where: { $0.id == profile.id }) else {
            return XCTFail("saved profile must appear in the profile list")
        }
        XCTAssertEqual(listed.name, profile.name)
        XCTAssertEqual(listed.baseURL, profile.baseURL)
        XCTAssertEqual(store.token(for: profile.id), token)
        XCTAssertTrue(
            keychainContains(token),
            "the token must be stored as a Keychain generic password"
        )
    }

    func testSavedProfileSurvivesANewStoreInstance() throws {
        let profile = makeProfile(name: "home-server")
        let token = issueToken()
        try store.save(profile, token: token)

        let reloaded = ProfileStore()
        XCTAssertTrue(
            reloaded.profiles.contains(where: { $0.id == profile.id }),
            "a fresh store instance must list the persisted profile"
        )
        XCTAssertEqual(
            reloaded.token(for: profile.id), token,
            "the Keychain token must outlive the store instance"
        )
    }

    func testTokenIsNeverPersistedOutsideKeychain() throws {
        let profile = makeProfile(name: "vps")
        let token = issueToken()
        try store.save(profile, token: token)

        XCTAssertFalse(
            userDefaultsContains(token),
            "the token must not be written to UserDefaults"
        )
        XCTAssertTrue(
            sandboxFilesContaining(token).isEmpty,
            "the token must not be written to files under the app sandbox: "
                + sandboxFilesContaining(token).map(\.lastPathComponent).joined(separator: ", ")
        )
        let encoded = try JSONEncoder().encode(profile)
        XCTAssertFalse(
            String(decoding: encoded, as: UTF8.self).contains(token),
            "the token must not be a field on the persisted profile model"
        )
    }

    // MARK: - Active profile

    func testSwitchingActiveProfileMovesPointerAndKeepsBothProfiles() throws {
        let first = makeProfile(name: "first")
        let second = makeProfile(name: "second")
        try store.save(first, token: issueToken())
        try store.save(second, token: issueToken())

        store.activeProfileId = first.id
        XCTAssertEqual(store.activeProfileId, first.id)

        store.activeProfileId = second.id
        XCTAssertEqual(store.activeProfileId, second.id)

        let ids = store.profiles.map(\.id)
        XCTAssertTrue(ids.contains(first.id), "the previously active profile must remain listed")
        XCTAssertTrue(ids.contains(second.id), "the active profile must remain listed")
        XCTAssertEqual(
            ProfileStore().activeProfileId, second.id,
            "the active pointer must persist across store instances"
        )
    }

    // MARK: - Edit + delete

    func testUpdatingProfileReplacesItsKeychainToken() throws {
        let profile = makeProfile(name: "old-name")
        try store.save(profile, token: issueToken())

        let newToken = issueToken()
        var edited = profile
        edited.name = "new-name"
        try store.save(edited, token: newToken)

        XCTAssertEqual(store.token(for: profile.id), newToken)
        XCTAssertTrue(keychainContains(newToken))
        XCTAssertEqual(
            store.profiles.first(where: { $0.id == profile.id })?.name,
            "new-name",
            "re-saving a profile id must update its stored metadata"
        )
    }

    func testDeletingProfileRemovesKeychainTokenAndUnlistsIt() throws {
        let doomed = makeProfile(name: "doomed")
        let token = issueToken()
        try store.save(doomed, token: token)
        let survivor = makeProfile(name: "survivor")
        try store.save(survivor, token: issueToken())
        store.activeProfileId = doomed.id

        try store.delete(doomed.id)
        createdProfileIDs.removeAll { $0 == doomed.id }

        XCTAssertFalse(store.profiles.contains(where: { $0.id == doomed.id }))
        XCTAssertTrue(store.profiles.contains(where: { $0.id == survivor.id }))
        XCTAssertNil(store.token(for: doomed.id))
        XCTAssertFalse(
            keychainContains(token),
            "deleting a profile must delete its Keychain item"
        )
        XCTAssertNotEqual(
            store.activeProfileId, doomed.id,
            "the active pointer must never reference a deleted profile"
        )
    }

    // MARK: - Helpers

    private func makeProfile(name: String) -> DaemonProfile {
        DaemonProfile(
            id: UUID(),
            name: name,
            baseURL: URL(string: "http://192.168.1.5:4020")!
        )
    }

    private func issueToken() -> String {
        let token = "tok-" + UUID().uuidString
        issuedTokens.insert(token)
        return token
    }

    /// Every generic-password item in the test keychain. The store's
    /// service/account naming is its own — the contract is the class and
    /// the value, not the keys it files under.
    private func keychainItems() -> [[String: Any]] {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecReturnData as String: true,
            kSecReturnAttributes as String: true,
            kSecMatchLimit as String: kSecMatchLimitAll,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess else {
            return []
        }
        return result as? [[String: Any]] ?? []
    }

    private func keychainContains(_ token: String) -> Bool {
        keychainItems().contains { item in
            (item[kSecValueData as String] as? Data)
                .flatMap { String(data: $0, encoding: .utf8) } == token
        }
    }

    private func purgeIssuedTokensFromKeychain() {
        for item in keychainItems() {
            guard let data = item[kSecValueData as String] as? Data,
                  let value = String(data: data, encoding: .utf8),
                  issuedTokens.contains(value)
            else {
                continue
            }
            var delete: [String: Any] = [kSecClass as String: kSecClassGenericPassword]
            if let service = item[kSecAttrService as String] as? String {
                delete[kSecAttrService as String] = service
            }
            if let account = item[kSecAttrAccount as String] as? String {
                delete[kSecAttrAccount as String] = account
            }
            let status = SecItemDelete(delete as CFDictionary)
            if status != errSecSuccess && status != errSecItemNotFound {
                XCTFail("Keychain cleanup failed for a test token: OSStatus \(status)")
            }
        }
        issuedTokens.removeAll()
    }

    private func userDefaultsContains(_ secret: String) -> Bool {
        UserDefaults.standard.dictionaryRepresentation().values.contains { value in
            switch value {
            case let string as String:
                return string.contains(secret)
            case let data as Data:
                return String(data: data, encoding: .utf8)?.contains(secret) ?? false
            default:
                return false
            }
        }
    }

    /// Files that could plausibly hold profile metadata — Documents,
    /// Application Support, Caches. Files that cannot be read as UTF-8
    /// text are not a readable persistence format for a secret string.
    private func sandboxFilesContaining(_ secret: String) -> [URL] {
        let fm = FileManager.default
        let roots = [
            fm.urls(for: .documentDirectory, in: .userDomainMask),
            fm.urls(for: .applicationSupportDirectory, in: .userDomainMask),
            fm.urls(for: .cachesDirectory, in: .userDomainMask),
        ].flatMap { $0 }
        var hits: [URL] = []
        for root in roots {
            guard let enumerator = fm.enumerator(
                at: root,
                includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey]
            ) else {
                continue
            }
            for case let url as URL in enumerator {
                guard
                    let values = try? url.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey]),
                    values.isRegularFile == true,
                    (values.fileSize ?? 0) < 4_000_000,
                    let text = try? String(contentsOf: url, encoding: .utf8),
                    text.contains(secret)
                else {
                    continue
                }
                hits.append(url)
            }
        }
        return hits
    }
}
