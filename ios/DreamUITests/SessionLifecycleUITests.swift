import XCTest

/// Session lifecycle from the browser: creating, renaming, killing,
/// respawning, deleting, and cleaning — each change must show up in the
/// app and on the daemon itself.
final class SessionLifecycleUITests: DreamUITestCase {
    private let daemon = DaemonEndpoint.primary

    /// Creating a session from the toolbar puts its row in the list and
    /// registers it on the daemon.
    func testCreatedSessionAppearsInListAndOnDaemon() async throws {
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "workbench"))

        app.buttons["sessions.create"].tap()
        let nameField = app.textFields["sessionform.name"]
        XCTAssertTrue(nameField.waitForExistence(timeout: 5))
        typeText("fresh-shell", into: nameField)
        app.buttons["sessionform.submit"].tap()

        let row = app.staticTexts["fresh-shell"]
        XCTAssertTrue(row.waitForExistence(timeout: 10), "new session should be listed")

        let sessions = try await daemon.listSessions()
        XCTAssertEqual(sessions.map(\.name), ["fresh-shell"])
        XCTAssertEqual(sessions.first?.status, "Live")
    }

    /// Renaming from the row's menu updates the row and the daemon's
    /// record.
    func testRenamedSessionUpdatesRowAndDaemon() async throws {
        let session = try await daemon.createSession(name: "old-name", cmd: "sleep 600")
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "workbench"))

        openRowMenu(id: session.id)
        XCTAssertTrue(app.buttons["Rename"].waitForExistence(timeout: 5))
        app.buttons["Rename"].tap()

        let alert = app.alerts["Rename session"]
        XCTAssertTrue(alert.waitForExistence(timeout: 5))
        let field = alert.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.clearAndType("renamed-shell")
        alert.buttons["Save"].tap()

        XCTAssertTrue(app.staticTexts["renamed-shell"].waitForExistence(timeout: 10))
        let updated = try await daemon.session(id: session.id)
        XCTAssertEqual(updated.name, "renamed-shell")
    }

    /// Kill turns the row Dead — and only then does Respawn appear;
    /// respawning brings it back Live. Both transitions are verified
    /// against the daemon.
    func testKillingAndRespawningTracksDaemonStatus() async throws {
        let session = try await daemon.createSession(name: "killable", cmd: "sleep 600")
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "workbench"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))

        openRowMenu(id: session.id)
        XCTAssertTrue(app.buttons["Kill"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Respawn"].exists, "live sessions never offer respawn")
        app.buttons["Kill"].tap()

        XCTAssertTrue(app.staticTexts["dead"].waitForExistence(timeout: 10))
        var observed = try await daemon.session(id: session.id)
        XCTAssertEqual(observed.status, "Dead")

        openRowMenu(id: session.id)
        XCTAssertTrue(app.buttons["Respawn"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Kill"].exists, "dead sessions offer respawn, not kill")
        app.buttons["Respawn"].tap()

        XCTAssertTrue(app.staticTexts["live"].waitForExistence(timeout: 10))
        observed = try await daemon.session(id: session.id)
        XCTAssertEqual(observed.status, "Live")
    }

    /// Deleting asks for confirmation, removes the row, and the session
    /// is gone from the daemon.
    func testDeletingSessionRemovesRowAndDaemonRecord() async throws {
        let session = try await daemon.createSession(name: "doomed", cmd: "sleep 600")
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "workbench"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))

        openRowMenu(id: session.id)
        XCTAssertTrue(app.buttons["Delete"].waitForExistence(timeout: 5))
        app.buttons["Delete"].tap()

        let confirm = app.alerts.firstMatch.buttons["Delete"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5))
        confirm.tap()

        let row = sessionRow(id: session.id)
        XCTAssertTrue(row.waitForNonExistence(timeout: 10), "deleted row should disappear")
        let statusCode = try await daemon.sessionStatusCode(id: session.id)
        XCTAssertEqual(statusCode, 404)
        XCTAssertTrue(app.staticTexts["No sessions"].waitForExistence(timeout: 5))
    }

    /// "Clean replay log" resets the session's scrollback file — the
    /// closest observable stand-in for "reattach shows a cleared
    /// scrollback" until the real terminal view lands.
    func testCleanResetsReplayLog() async throws {
        let session = try await daemon.createSession(
            name: "scrollback",
            cmd: "echo e2e-scrollback-marker; sleep 600"
        )
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "workbench"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))

        // Wait until the shell's output has actually been persisted.
        var populated = false
        for _ in 0..<20 where !populated {
            populated = (fileSize(at: session.logPath) ?? 0) > 0
            if !populated { usleep(250_000) }
        }
        XCTAssertTrue(populated, "replay log should hold shell output before cleaning")
        let before = fileSize(at: session.logPath) ?? 0

        openRowMenu(id: session.id)
        XCTAssertTrue(app.buttons["Clean replay log"].waitForExistence(timeout: 5))
        app.buttons["Clean replay log"].tap()

        var cleared = false
        for _ in 0..<20 where !cleared {
            let size = fileSize(at: session.logPath)
            cleared = size != nil && size! < before
            if !cleared { usleep(250_000) }
        }
        XCTAssertTrue(cleared, "clean should reset the session's replay log")
        let after = try await daemon.session(id: session.id)
        XCTAssertEqual(after.status, "Live")
    }
}
