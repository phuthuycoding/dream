import XCTest

/// Multiple daemon profiles: adding a second pairing, switching which
/// one is active, and deleting entries.
final class ProfilesUITests: DreamUITestCase {

    /// Two profiles pointing at two different daemons (a "LAN" daemon
    /// and a "Tailscale" one — both loopback here, distinguished by the
    /// sessions each serves). Switching to the second must connect and
    /// show that daemon's sessions; switching back shows the first.
    func testAddingAndSwitchingProfilesConnectsEachDaemon() async throws {
        // Only the secondary daemon knows this session — its row is the
        // proof which daemon the app is talking to.
        let marker = try await DaemonEndpoint.secondary.createSession(
            name: "tailscale-only-shell",
            cmd: "sleep 600"
        )

        XCTAssertTrue(pairApp(urlString: DaemonEndpoint.primary.pairingURL, profileName: "LAN"))

        // Add the second profile from the switcher. No custom name —
        // the profile falls back to its `host:port` label, which is
        // what the nav bar assertions below check.
        app.buttons["sessions.profiles"].tap()
        XCTAssertTrue(app.buttons["profiles.add"].waitForExistence(timeout: 5))
        app.buttons["profiles.add"].tap()

        XCTAssertTrue(
            pairApp(urlString: DaemonEndpoint.secondary.pairingURL),
            "pairing the second daemon should land on its session browser"
        )
        XCTAssertTrue(app.navigationBars["127.0.0.1:4162"].waitForExistence(timeout: 5))
        XCTAssertTrue(
            sessionRow(id: marker.id).waitForExistence(timeout: 10),
            "switched profile should list the secondary daemon's session"
        )

        // Switch back: the LAN daemon shows its own (empty) list.
        app.buttons["sessions.profiles"].tap()
        let lanRow = app.buttons.matching(
            NSPredicate(format: "identifier BEGINSWITH %@ AND label CONTAINS %@",
                        "profile.row.", "LAN")
        ).firstMatch
        XCTAssertTrue(lanRow.waitForExistence(timeout: 5))
        lanRow.tap()

        XCTAssertTrue(app.navigationBars["LAN"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["No sessions"].waitForExistence(timeout: 5))
        XCTAssertFalse(sessionRow(id: marker.id).exists)
    }

    /// Removing a profile drops it from the switcher, and removing the
    /// active one takes the app back to pairing since nothing is left
    /// to connect through.
    func testDeletingProfilesRemovesThemFromTheSwitcher() throws {
        XCTAssertTrue(pairApp(urlString: DaemonEndpoint.primary.pairingURL, profileName: "LAN"))
        XCTAssertTrue(app.navigationBars["LAN"].waitForExistence(timeout: 5))

        // Delete the only profile while it is active.
        app.buttons["sessions.profiles"].tap()
        let lanRow = app.buttons.matching(
            NSPredicate(format: "identifier BEGINSWITH %@ AND label CONTAINS %@",
                        "profile.row.", "LAN")
        ).firstMatch
        XCTAssertTrue(lanRow.waitForExistence(timeout: 5))
        lanRow.swipeLeft()
        XCTAssertTrue(app.buttons["Delete"].waitForExistence(timeout: 4))
        app.buttons["Delete"].tap()
        let confirm = app.alerts.firstMatch.buttons["Delete"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 4))
        confirm.tap()

        XCTAssertTrue(
            app.textFields["pairing.paste"].waitForExistence(timeout: 8),
            "deleting the active profile should return to the pairing form"
        )
    }
}
