import XCTest
@testable import Dream

/// The stale-401 guard on `AppState.handleUnauthorized`: a rejected
/// credential may only clear the Keychain entry it actually presented —
/// a late 401 carrying a superseded token must not wipe a fresh re-pair,
/// and only the active profile's rejection routes to the re-pair screen.
final class AppStateUnauthorizedTests: XCTestCase {
    private var store: ProfileStore!
    private var appState: AppState!
    private var fileURL: URL!
    /// Profiles created by the running test, deleted on teardown so
    /// their Keychain tokens don't leak between runs.
    private var profileIDs: [UUID] = []

    override func setUp() {
        super.setUp()
        fileURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("dream-appstate-tests-\(UUID().uuidString).json")
        store = ProfileStore(fileURL: fileURL)
        appState = AppState(store: store)
    }

    override func tearDownWithError() throws {
        for id in profileIDs {
            try store.delete(id)
        }
        profileIDs.removeAll()
        appState = nil
        store = nil
        if let fileURL, FileManager.default.fileExists(atPath: fileURL.path) {
            try FileManager.default.removeItem(at: fileURL)
        }
        fileURL = nil
        try super.tearDownWithError()
    }

    func testRejectedTokenMatchingTheStoredOneClearsItAndPromptsRePair() throws {
        let profile = tracked(makeProfile(name: "studio"))
        try appState.completePairing(profile: profile, token: "live-token")

        appState.handleUnauthorized(profileId: profile.id, rejectedToken: "live-token")

        XCTAssertNil(
            appState.token(for: profile.id),
            "the Keychain must drop the credential the daemon rejected"
        )
        XCTAssertTrue(
            appState.needsRepair,
            "the active profile's rejection must route to the re-pair screen"
        )
    }

    func testRejectedTokenSupersededByRePairLeavesTheFreshCredential() throws {
        let profile = tracked(makeProfile(name: "studio"))
        try appState.completePairing(profile: profile, token: "superseded-token")
        try appState.completePairing(profile: profile, token: "fresh-token")

        appState.handleUnauthorized(profileId: profile.id, rejectedToken: "superseded-token")

        XCTAssertEqual(
            appState.token(for: profile.id), "fresh-token",
            "a stale 401 must not wipe the freshly re-paired token"
        )
        XCTAssertFalse(
            appState.needsRepair,
            "a credential that was never rejected must not prompt re-pair"
        )
    }

    func testUnauthorizedWithoutAReportedTokenClearsNothing() throws {
        let profile = tracked(makeProfile(name: "studio"))
        try appState.completePairing(profile: profile, token: "live-token")

        appState.handleUnauthorized(profileId: profile.id)

        XCTAssertEqual(
            appState.token(for: profile.id), "live-token",
            "a 401 that cannot name its credential must not clear the stored one"
        )
        XCTAssertFalse(appState.needsRepair)
    }

    func testRejectedTokenForABackgroundProfileClearsItWithoutRepairPrompt() throws {
        let background = tracked(makeProfile(name: "background"))
        try appState.completePairing(profile: background, token: "background-token")
        let active = tracked(makeProfile(name: "active"))
        try appState.completePairing(profile: active, token: "active-token")

        appState.handleUnauthorized(profileId: background.id, rejectedToken: "background-token")

        XCTAssertNil(
            appState.token(for: background.id),
            "the background profile's rejected credential is still dropped"
        )
        XCTAssertFalse(
            appState.needsRepair,
            "only the active profile's rejection prompts re-pair"
        )
        XCTAssertEqual(
            appState.token(for: active.id), "active-token",
            "the active profile's credential is never touched by another profile's 401"
        )
    }

    private func makeProfile(name: String) -> DaemonProfile {
        DaemonProfile(
            id: UUID(),
            name: name,
            baseURL: URL(string: "http://192.168.1.5:4020")!
        )
    }

    private func tracked(_ profile: DaemonProfile) -> DaemonProfile {
        profileIDs.append(profile.id)
        return profile
    }
}
