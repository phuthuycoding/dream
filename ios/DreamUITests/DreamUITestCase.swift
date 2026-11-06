import XCTest

/// One daemon pairing target used by the specs. The suite expects two
/// real daemons on loopback plus an optional "gate" proxy that can be
/// told (via a flag file) to start answering 401 — that is how a live
/// session losing its token is exercised without restarting anything.
struct DaemonEndpoint {
    let baseURL: URL
    let token: String

    var pairingURL: String {
        "\(baseURL.absoluteString)/?token=\(token)"
    }

    static let primary = DaemonEndpoint(
        baseURL: URL(string: "http://127.0.0.1:4161")!,
        token: "e2e-ui-token-alpha"
    )

    static let secondary = DaemonEndpoint(
        baseURL: URL(string: "http://127.0.0.1:4162")!,
        token: "e2e-ui-token-beta"
    )

    /// Transparent proxy in front of `primary`. Writing
    /// `/tmp/dream-e2e/deny.flag` makes it answer 401 to new connections.
    static let gated = DaemonEndpoint(
        baseURL: URL(string: "http://127.0.0.1:4171")!,
        token: primary.token
    )

    static let denyFlagPath = "/tmp/dream-e2e/deny.flag"
}

/// A session exactly as the daemon serializes it — the fields the specs
/// assert on. This is the `dream ps` equivalent: the UI test process
/// can't shell out, so it asks the daemon over REST.
struct DaemonSession: Decodable {
    let id: String
    let name: String
    let cmd: String
    let status: String
    let logPath: String
    let resurrected: Bool
    let attention: Bool

    private enum CodingKeys: String, CodingKey {
        case id, name, cmd, status, resurrected, attention
        case logPath = "log_path"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        name = try container.decode(String.self, forKey: .name)
        cmd = try container.decode(String.self, forKey: .cmd)
        status = try container.decode(String.self, forKey: .status)
        logPath = try container.decode(String.self, forKey: .logPath)
        // `attention` is runtime-only — create responses omit it, and
        // older daemons omit `resurrected` too.
        resurrected = try container.decodeIfPresent(Bool.self, forKey: .resurrected) ?? false
        attention = try container.decodeIfPresent(Bool.self, forKey: .attention) ?? false
    }
}

enum DaemonAPIError: Error {
    case http(Int)
    case badURL
}

/// Minimal bearer-token REST client mirroring the app's API surface.
extension DaemonEndpoint {
    private func request(_ method: String, path: String, body: Data? = nil) -> URLRequest {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = body
        }
        return request
    }

    private func send(_ request: URLRequest) async throws -> (Data, Int) {
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? -1
        return (data, status)
    }

    func listSessions() async throws -> [DaemonSession] {
        let (data, status) = try await send(request("GET", path: "api/sessions"))
        guard (200...299).contains(status) else { throw DaemonAPIError.http(status) }
        return try JSONDecoder().decode([DaemonSession].self, from: data)
    }

    func session(id: String) async throws -> DaemonSession {
        let (data, status) = try await send(request("GET", path: "api/sessions/\(id)"))
        guard (200...299).contains(status) else { throw DaemonAPIError.http(status) }
        return try JSONDecoder().decode(DaemonSession.self, from: data)
    }

    /// Raw status lookup for "is it gone?" assertions.
    func sessionStatusCode(id: String) async throws -> Int {
        let (_, status) = try await send(request("GET", path: "api/sessions/\(id)"))
        return status
    }

    @discardableResult
    func createSession(name: String, cmd: String? = nil) async throws -> DaemonSession {
        var payload: [String: String] = ["name": name]
        if let cmd { payload["cmd"] = cmd }
        let body = try JSONSerialization.data(withJSONObject: payload)
        let (data, status) = try await send(request("POST", path: "api/sessions", body: body))
        guard (200...299).contains(status) else { throw DaemonAPIError.http(status) }
        return try JSONDecoder().decode(DaemonSession.self, from: data)
    }

    func deleteSession(id: String) async throws {
        let (_, status) = try await send(request("DELETE", path: "api/sessions/\(id)"))
        guard (200...299).contains(status) || status == 404 else {
            throw DaemonAPIError.http(status)
        }
    }
}

/// Shared fixture: launches the app, walks whatever state it persisted
/// back to the pairing screen, and gives every spec the same start.
/// XCUIAutomation only accepts calls on the main thread, which async
/// test methods would otherwise leave — hence @MainActor.
@MainActor
class DreamUITestCase: XCTestCase {
    var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
        app = XCUIApplication()
        app.terminate()
        app.launch()
        bringAppToPairingScreen()
    }

    override func tearDown() async throws {
        // The daemons are dedicated to this suite: wipe every session so
        // no spec inherits rows or replay logs from an earlier one.
        for endpoint in [DaemonEndpoint.primary, DaemonEndpoint.secondary] {
            if let sessions = try? await endpoint.listSessions() {
                for session in sessions {
                    try? await endpoint.deleteSession(id: session.id)
                }
            }
        }
        try? FileManager.default.removeItem(atPath: DaemonEndpoint.denyFlagPath)
        app = nil
    }

    /// Leaves the app on the pairing form regardless of what a previous
    /// run persisted: opens the profile sheet and deletes every profile,
    /// which also drops the sheet automatically once the active profile
    /// disappears.
    func bringAppToPairingScreen() {
        let paste = app.textFields["pairing.paste"]
        if paste.waitForExistence(timeout: 3) { return }

        let profilesButton = app.buttons["sessions.profiles"]
        guard profilesButton.waitForExistence(timeout: 8) else { return }
        profilesButton.tap()
        _ = app.buttons["Done"].waitForExistence(timeout: 5)

        while !paste.exists {
            let row = app.descendants(matching: .any)
                .matching(NSPredicate(format: "identifier BEGINSWITH %@", "profile.row."))
                .firstMatch
            guard row.waitForExistence(timeout: 4) else { break }
            row.swipeLeft()
            let swipeDelete = app.buttons["Delete"]
            _ = swipeDelete.waitForExistence(timeout: 4)
            swipeDelete.tap()
            let confirm = app.alerts.firstMatch.buttons["Delete"]
            _ = confirm.waitForExistence(timeout: 4)
            confirm.tap()
            _ = paste.waitForExistence(timeout: 3)
        }

        if app.buttons["Done"].exists { app.buttons["Done"].tap() }
        XCTAssertTrue(paste.waitForExistence(timeout: 8), "app should return to the pairing form")
    }

    /// Pastes a `dream url` line, optionally names the profile, saves it,
    /// and waits until the session browser is up. Returns whether the
    /// browser appeared.
    @discardableResult
    func pairApp(urlString: String, profileName: String? = nil) -> Bool {
        let paste = app.textFields["pairing.paste"]
        guard paste.waitForExistence(timeout: 5) else { return false }
        paste.clearAndType(urlString)
        if let profileName {
            let nameField = app.textFields["pairing.name"]
            if nameField.waitForExistence(timeout: 3) {
                typeText(profileName, into: nameField)
            }
        }
        app.buttons["pairing.save"].tap()
        return app.buttons["sessions.create"].waitForExistence(timeout: 15)
    }

    /// Taps a field, waits until the keyboard is actually up, then
    /// types — typing before focus settles loses the event to a
    /// "no keyboard focus" synthesis failure, mostly on sheets. When
    /// the keyboard is already up from a previous field the wait returns
    /// instantly while the focus handoff is still in flight, so a short
    /// settle delay follows it.
    func typeText(_ text: String, into element: XCUIElement) {
        element.tap()
        _ = app.keyboards.firstMatch.waitForExistence(timeout: 5)
        usleep(300_000)
        element.typeText(text)
    }

    /// The session list row for a known daemon session id.
    func sessionRow(id: String) -> XCUIElement {
        app.descendants(matching: .any)["session.row.\(id)"]
    }

    /// Opens a row's context menu — swipe actions cover the same items,
    /// but the menu is reachable without knowing the row's swipe side.
    func openRowMenu(id: String) {
        let row = sessionRow(id: id)
        XCTAssertTrue(row.waitForExistence(timeout: 10), "session row \(id) should exist")
        row.press(forDuration: 1.2)
    }

    /// Reads a file attribute on the host filesystem — the test runner
    /// shares the macOS filesystem, so daemon replay logs are statable.
    func fileSize(at path: String) -> Int? {
        let attributes = try? FileManager.default.attributesOfItem(atPath: path)
        return attributes?[.size] as? Int
    }
}

extension XCUIElement {
    /// Replaces a field's contents: taps to focus, walks the caret to
    /// the end, backspaces past the old value, then types the new one.
    func clearAndType(_ text: String) {
        let existing = (value as? String) ?? ""
        coordinate(withNormalizedOffset: CGVector(dx: 0.98, dy: 0.5)).tap()
        let backspaces = String(repeating: XCUIKeyboardKey.delete.rawValue, count: existing.count + 4)
        typeText(backspaces + text)
    }
}


