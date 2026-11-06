import Foundation
import XCTest

/// TC-013 — a daemon attention flag reaches the user: row badge on the
/// list, a local notification banner while the app isn't showing that
/// session's terminal, notification tap deep-links into the terminal,
/// and a flag on the foregrounded terminal never notifies (the attach
/// already acknowledged it).
///
/// Fixture notes:
/// - The flag is raised by a BEL byte (0x07) in pty output. The daemon's
///   REST has no input endpoint — the specs attach a
///   `URLSessionWebSocketTask` to `/api/sessions/{id}/ws?token=` and send
///   the binary `i`nput frame `printf '\a'\n`, exactly the pty write the
///   app's own terminal performs.
/// - Sessions run a ticker shell (`(while sleep 10; do printf .; done) &
///   exec sh`): the daemon also flags attention after 30s of output
///   silence, and UI pairing takes ~1-2 min under XCTest quiescence —
///   without periodic output every session would pre-flag before the app
///   ever lists it, priming the edge detector and muting the very
///   notification edge under test. The ticker keeps the flag false until
///   the spec rings the bell.
/// - XCUITest cannot read delivered notifications; the observable proxy
///   is the SpringBoard banner (`willPresent` returns `.banner`, so the
///   alert shows even while the app is foreground) plus the row badge.
final class AttentionUITests: DreamUITestCase {
    private let daemon = DaemonEndpoint.primary

    override func setUpWithError() throws {
        // AttentionCenter requests notification permission in its init —
        // iOS shows the "Allow" prompt on the app's first launch, mid-way
        // through the shared fixture. Approve it (monitor + explicit
        // check) or every banner assertion below is vacuous.
        addUIInterruptionMonitor(withDescription: "notification permission") { alert in
            let allow = alert.buttons["Allow"]
            guard allow.exists else { return false }
            allow.tap()
            return true
        }
        try super.setUpWithError()
        allowNotificationPermissionIfAsked()
    }

    /// TC-013 step 1 — the flag flips on while the app sits on the session
    /// list → the row's attention badge appears on the next list refresh
    /// (the ~15s foreground poll; no `a` frame reaches an unattached app).
    func testAttentionFlagOnListShowsBadgeOnSessionRow() async throws {
        let session = try await daemon.createSession(name: "bell-list", cmd: Self.tickerShell)
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "attn-prof"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))
        XCTAssertFalse(
            badge(for: session).exists,
            "a session with no flag must render no badge before the bell"
        )

        try await ringBell(sessionID: session.id)
        try await waitForDaemonAttention(session.id, want: true)

        XCTAssertTrue(
            badge(for: session).waitForExistence(timeout: 30),
            "the next list poll should surface the flag as the row's attention badge"
        )
    }

    /// TC-013 steps 2+3 — the flag flips while the app is not on that
    /// session's terminal → a local notification is posted (SpringBoard
    /// banner carrying the session name) and tapping it deep-links into
    /// that session's terminal.
    ///
    /// NOT covered if the banner is unreachable: SpringBoard banner
    /// visibility in the simulator's a11y tree is best-effort. A missing
    /// banner is reported as XCTSkip rather than a failure — the spec
    /// cannot distinguish "not posted" from "posted but unobservable",
    /// and the badge spec + daemon REST state still prove the flag and
    /// its delivery to the UI. The bg-grace variant of step 2 is not
    /// covered at all: backgrounding suspends the foreground poll, and a
    /// suspended app cannot be queried by XCUITest anyway.
    func testAttentionBannerAppearsAndTapOpensSessionTerminal() async throws {
        let session = try await daemon.createSession(name: "bell-notify", cmd: Self.tickerShell)
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "attn-prof"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))

        try await ringBell(sessionID: session.id)
        try await waitForDaemonAttention(session.id, want: true)

        // The same poll edge renders the badge and posts the banner —
        // watch for the banner first since it auto-dismisses in ~5s
        // while the badge persists.
        guard let banner = waitForSpringboardBanner(matching: session.name, timeout: 30) else {
            throw XCTSkip(
                "attention banner not reachable via SpringBoard accessibility " +
                "in this environment — cannot assert posting or tap-to-open"
            )
        }
        banner.tap()

        let terminal = app.descendants(matching: .any)["terminal.view"]
        XCTAssertTrue(
            terminal.waitForExistence(timeout: 15),
            "tapping the attention notification should open that session's terminal"
        )
    }

    /// TC-013 step 4 — the flag flips on the session whose terminal the
    /// app is showing → NO notification posts (the attach is already the
    /// acknowledgement). The flag is still real: REST reports it and the
    /// row badge appears once back on the list.
    ///
    /// The "no banner" half is only as strong as banner detectability —
    /// if testAttentionBannerAppearsAndTapOpensSessionTerminal saw one,
    /// SpringBoard a11y is proven to surface banners in this environment
    /// and this absence assertion is meaningful.
    func testAttentionOnForegroundedTerminalPostsNoNotification() async throws {
        let session = try await daemon.createSession(name: "bell-foreground", cmd: Self.tickerShell)
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "attn-prof"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))
        sessionRow(id: session.id).tap()
        waitForTerminalLive()

        try await ringBell(sessionID: session.id)
        // The REST flag proves the edge actually happened — without it a
        // missing banner could mean the flag never raised, not that it
        // was suppressed.
        try await waitForDaemonAttention(session.id, want: true)

        XCTAssertNil(
            waitForSpringboardBanner(matching: session.name, timeout: 10),
            "a flag on the foregrounded terminal must be suppressed — no banner"
        )

        // Back on the list the flag still shows as the row badge — only
        // an attach acknowledges it, and viewing the terminal is not an
        // acknowledgement of a flag raised after that attach.
        app.navigationBars.buttons.firstMatch.tap()
        XCTAssertTrue(app.buttons["sessions.create"].waitForExistence(timeout: 10))
        XCTAssertTrue(
            badge(for: session).waitForExistence(timeout: 30),
            "the flagged session should still carry its badge on the list"
        )
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    /// Ticks a dot every 10s so the daemon's 30s idle-attention never
    /// fires before the spec rings the bell; `exec sh` answers the
    /// `printf '\a'` written to its stdin.
    private static let tickerShell = "(while sleep 10; do printf .; done) & exec sh"

    /// The row's attention badge element.
    private func badge(for session: DaemonSession) -> XCUIElement {
        app.descendants(matching: .any)["session.attention.\(session.id)"]
    }

    /// Waits for the terminal to exist and the attaching overlay to
    /// clear — the attach is what acknowledges (clears) any prior flag.
    private func waitForTerminalLive() {
        let terminal = app.descendants(matching: .any)["terminal.view"]
        XCTAssertTrue(terminal.waitForExistence(timeout: 15), "terminal.view should appear")
        let attaching = app.staticTexts["Attaching…"]
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: attaching, handler: nil)
        waitForExpectations(timeout: 15)
    }

    /// Writes `printf '\a'\n` into the session's pty stdin over the
    /// daemon's WS attach protocol (`i` + payload), the same path the
    /// app's terminal uses — there is no REST input route.
    private func ringBell(sessionID: String) async throws {
        var components = URLComponents(
            url: daemon.baseURL.appendingPathComponent("api/sessions/\(sessionID)/ws"),
            resolvingAgainstBaseURL: false
        )
        components?.scheme = "ws"
        components?.queryItems = [URLQueryItem(name: "token", value: daemon.token)]
        let url = try XCTUnwrap(components?.url, "ws attach URL should build")

        let task = URLSession.shared.webSocketTask(with: url)
        task.resume()
        defer { task.cancel(with: .normalClosure, reason: nil) }

        // The attach contract always answers scrollback + one snapshot
        // frame immediately — the first receive also proves the upgrade
        // was accepted before the input frame goes out.
        _ = try await task.receive()
        var input = Data("i".utf8)
        input.append(Data("printf '\\a'\n".utf8))
        try await task.send(.data(input))
        try await Task.sleep(for: .milliseconds(300))
    }

    /// Polls `GET /api/sessions/{id}` until `attention` reads `want` —
    /// the monitor sets it asynchronously off the pty broadcast.
    private func waitForDaemonAttention(
        _ id: String,
        want: Bool,
        timeout: TimeInterval = 15
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if let session = try? await daemon.session(id: id), session.attention == want {
                return
            }
            try await Task.sleep(for: .milliseconds(250))
        }
        let last = try await daemon.session(id: id)
        XCTAssertEqual(last.attention, want, "daemon attention flag should be \(want)")
    }

    /// Polls SpringBoard for a notification banner whose label or
    /// identifier mentions `text` (the session name, which the banner
    /// title embeds as `"name" wants attention`). Returns nil if none
    /// surfaces inside `timeout` — the caller decides skip vs fail.
    private func waitForSpringboardBanner(
        matching text: String,
        timeout: TimeInterval
    ) -> XCUIElement? {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let predicate = NSPredicate(
            format: "label CONTAINS[c] %@ OR identifier CONTAINS[c] %@ OR value CONTAINS[c] %@",
            text, text, text
        )
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            for query in [
                springboard.otherElements.matching(predicate),
                springboard.cells.matching(predicate),
                springboard.staticTexts.matching(predicate),
            ] {
                let element = query.firstMatch
                if element.exists { return element }
            }
            usleep(300_000)
        }
        return nil
    }

    /// The permission prompt survives fixture navigation when no
    /// interaction interrupted it — dismiss it before pairing types.
    private func allowNotificationPermissionIfAsked() {
        let prompt = app.alerts
            .matching(NSPredicate(format: "label CONTAINS[c] 'Notifications'"))
            .firstMatch
        guard prompt.waitForExistence(timeout: 3) else { return }
        let allow = prompt.buttons["Allow"]
        if allow.waitForExistence(timeout: 3) { allow.tap() }
    }
}
