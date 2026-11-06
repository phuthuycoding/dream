import XCTest

/// Pairing flow against a live daemon: pasting the `dream url` output,
/// rejected credentials, and what happens across relaunches.
final class PairingUITests: DreamUITestCase {

    /// Paste the daemon's printed URL → the form parses it, save
    /// validates against the daemon, and the session browser appears
    /// under the chosen profile name.
    func testPastingDaemonURLPairsAndShowsSessionBrowser() throws {
        XCTAssertTrue(
            pairApp(urlString: DaemonEndpoint.primary.pairingURL, profileName: "Home LAN"),
            "session browser should appear after saving a valid pairing URL"
        )
        XCTAssertTrue(app.navigationBars["Home LAN"].waitForExistence(timeout: 5))

        // A fresh daemon has no sessions — the empty state is the
        // user-visible proof the list actually loaded.
        XCTAssertTrue(app.staticTexts["No sessions"].waitForExistence(timeout: 5))
    }

    /// Credentials the daemon rejects must surface the error and keep
    /// the user on the pairing form — nothing is persisted, and pasting
    /// a good `dream url` line afterwards succeeds.
    func testRejectedTokenShowsErrorAndAllowsCorrectPairing() throws {
        // Manual entry keeps the paste field empty, so the later paste
        // still wins credential resolution.
        let host = app.textFields["pairing.host"]
        XCTAssertTrue(host.waitForExistence(timeout: 5))
        typeText("127.0.0.1", into: host)
        typeText("4161", into: app.textFields["pairing.port"])
        typeText("not-the-real-token",
                 into: app.descendants(matching: .any)["pairing.token"])

        app.buttons["pairing.save"].tap()

        let error = app.staticTexts["pairing.error"]
        XCTAssertTrue(error.waitForExistence(timeout: 10), "rejected token should show an error")
        XCTAssertTrue(app.textFields["pairing.paste"].exists,
                      "app should stay on the pairing form after a rejection")
        XCTAssertFalse(app.buttons["sessions.create"].exists)

        // Same form, the real pairing URL → pairing completes.
        XCTAssertTrue(
            pairApp(urlString: DaemonEndpoint.primary.pairingURL, profileName: "Home LAN"),
            "correcting the credentials should finish pairing"
        )
    }

    /// Force-quit and relaunch: the persisted profile reconnects without
    /// the pairing form appearing at all.
    func testRelaunchReconnectsWithoutAskingToPairAgain() throws {
        XCTAssertTrue(pairApp(urlString: DaemonEndpoint.primary.pairingURL, profileName: "Home LAN"))

        app.terminate()
        app.launch()

        XCTAssertTrue(
            app.buttons["sessions.create"].waitForExistence(timeout: 15),
            "relaunch should land on the session browser"
        )
        XCTAssertFalse(app.textFields["pairing.paste"].exists, "relaunch must not re-prompt pairing")
        XCTAssertTrue(app.navigationBars["Home LAN"].waitForExistence(timeout: 5))
    }

    /// When the daemon starts rejecting the stored token, the app drops
    /// the credential and routes back to a re-pair form for the same
    /// profile; entering valid credentials restores the session list.
    func testDaemonRejectingStoredTokenPromptsRepair() throws {
        // The gated endpoint is a transparent proxy in front of the
        // primary daemon; the flag file makes it answer 401.
        let flagPath = DaemonEndpoint.denyFlagPath
        try? FileManager.default.removeItem(atPath: flagPath)

        XCTAssertTrue(
            pairApp(urlString: DaemonEndpoint.gated.pairingURL, profileName: "Gated"),
            "pairing through the gate should succeed while it forwards"
        )

        FileManager.default.createFile(atPath: flagPath, contents: Data("1".utf8))
        app.terminate()
        app.launch()

        let pairingNav = app.navigationBars["Re-pair Gated"]
        XCTAssertTrue(
            pairingNav.waitForExistence(timeout: 15),
            "a rejected stored token should route back to the re-pair form"
        )
        XCTAssertTrue(app.textFields["pairing.paste"].exists)
        XCTAssertFalse(app.buttons["sessions.create"].exists)

        // Daemon accepts again → re-entering credentials on the same
        // (fresh) form restores the session browser.
        try? FileManager.default.removeItem(atPath: flagPath)
        let paste = app.textFields["pairing.paste"]
        typeText(DaemonEndpoint.gated.pairingURL, into: paste)
        app.buttons["pairing.save"].tap()
        XCTAssertTrue(app.buttons["sessions.create"].waitForExistence(timeout: 15))
    }
}
