import XCTest

/// Terminal attach, typing, soft keys, touch scroll, and detach/reattach —
/// each spec drives the real app against a real daemon.
///
/// The SwiftTerm view renders its grid with Metal/CoreGraphics and exposes
/// no text to the accessibility tree (verified: `terminal.view` arrives as
/// a TextView element with an empty label/value). Every spec therefore
/// asserts on what IS user-observable to XCUITest:
///  - the attach overlay appearing/disappearing (snapshot applied = live),
///  - the native scroll indicators' presence and frame response to drags,
///  - the daemon's replay log (`session.logPath`) — the pty's output byte
///    stream, which is the closest checkable proxy for "the terminal showed
///    it". Note the pty echoes typed input one character at a time, so a
///    typed command never appears contiguously in the log — but a command's
///    *output* (one write per line) does, and soft keys leave recognizable
///    bytes (`^C`-style caret echoes, `^[[D`, literal tab under `cat`), plus
///    `"r"` resize records for geometry changes.
final class TerminalUITests: DreamUITestCase {
    private let daemon = DaemonEndpoint.primary

    /// TC-009 — tapping a row attaches the terminal, typing round-trips
    /// through the daemon, and a rotation renegotiates geometry (`r` frame
    /// → pty resize, verified via `stty size`).
    func testAttachTypeAndRotateRoundTrip() async throws {
        let session = try await daemon.createSession(name: "terminal-basic", cmd: "sh")
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "term-prof"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))
        sessionRow(id: session.id).tap()
        waitForTerminalLive()

        // Type `echo hi` — its output comes back over the websocket and
        // lands in the replay log (the typed echo is per-character, so the
        // output line is the single contiguous occurrence of the marker).
        let marker = "hi_e2e_attach"
        typeInTerminal("echo \(marker)\n")
        XCTAssertTrue(
            waitForLog(session.logPath, where: { $0.contains(marker) }),
            "echo \(marker) should round-trip — its output must reach the pty stream"
        )

        // Rotation changes the view size → sizeChanged → `r` frame → pty
        // SIGWINCH. The new geometry is read back with `stty size` (and the
        // daemon logs the `r` record itself).
        let geomCmd = "stty size | sed 's/^/GEOM:/'\n"
        typeInTerminal(geomCmd)
        XCTAssertTrue(waitForLog(session.logPath, where: { self.lastGeometry($0) != nil }))
        let portrait = lastGeometry(logText(session.logPath))
        XCTAssertNotNil(portrait)

        XCUIDevice.shared.orientation = .landscapeLeft
        defer { XCUIDevice.shared.orientation = .portrait }
        usleep(1_500_000) // let the rotation + resize frame settle

        typeInTerminal(geomCmd)
        XCTAssertTrue(
            waitForLog(session.logPath, where: {
                guard let g = self.lastGeometry($0), let p = portrait else { return false }
                return g != p
            }),
            "geometry after rotation should differ — resize frame should renegotiate"
        )
    }

    /// TC-010 — accessory soft keys emit the right bytes: ctrl latches a
    /// modifier so the next key is a control byte (Ctrl-C interrupts the
    /// foreground sleep → the prompt returns), the Up arrow drives readline
    /// (recalls history — the redrawn command lands in the log), and under
    /// `cat` each soft key's raw byte is kernel-echoed into the pty stream
    /// where the exact byte is assertable (`^[`/0x1b for Esc, a tab byte
    /// for Tab, `^[[D` for Left).
    func testAccessorySoftKeysSendBytesThroughWs() async throws {
        let session = try await daemon.createSession(name: "terminal-keys", cmd: "sh")
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "term-prof"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))
        sessionRow(id: session.id).tap()
        waitForTerminalLive()

        // Ctrl-C: start a foreground sleep that cannot finish on its own,
        // latch ctrl, type `c` → 0x03 → SIGINT → the shell prints a fresh
        // prompt. (sh-3.2 doesn't echo a literal "^C" — the returning
        // prompt is the observable interruption.)
        let promptCount = countOccurrences("sh-3.2$ ", in: logText(session.logPath))
        typeInTerminal("sleep 600\n")
        usleep(500_000) // let the sleep take foreground before interrupting
        app.buttons["ctrl"].tap()
        terminal().typeText("c")
        XCTAssertTrue(
            waitForLog(session.logPath, where: {
                self.countOccurrences("sh-3.2$ ", in: $0) > promptCount
            }),
            "ctrl + c should send a control byte that interrupts the sleep"
        )

        // Up arrow recalls the previous command: readline redraws the whole
        // line in one write — the contiguous command string only ever comes
        // from the recall, not the per-character typed echo.
        typeInTerminal("echo arrow_recall\n")
        XCTAssertTrue(waitForLog(session.logPath, where: { $0.contains("arrow_recall") }))
        app.buttons["Up"].tap()
        XCTAssertTrue(
            waitForLog(session.logPath, where: { $0.contains("echo arrow_recall") }),
            "Up should emit the cursor-up sequence and recall the last command"
        )
        // Dismiss the recalled line without running it.
        app.buttons["ctrl"].tap()
        terminal().typeText("c")

        // Under `cat` the tty echoes input bytes literally — the raw soft-key
        // bytes become greppable markers in the replay log.
        typeInTerminal("cat\n")
        usleep(1_000_000) // cat gives no output of its own; let it take over the tty

        var before = logText(session.logPath).count
        app.buttons["esc"].tap()
        XCTAssertTrue(
            waitForLogGrowth(session.logPath, beyond: before, where: {
                $0.contains("^[") || $0.contains("\\u001b")
            }),
            "esc should emit 0x1b — echoed under cat as ^[ or the raw byte"
        )

        before = logText(session.logPath).count
        app.buttons["tab"].tap()
        XCTAssertTrue(
            waitForLogGrowth(session.logPath, beyond: before, where: {
                $0.contains("\\t") || $0.contains("^I")
            }),
            "tab should emit 0x09 — echoed as a literal tab under cat"
        )

        before = logText(session.logPath).count
        app.buttons["Left"].tap()
        XCTAssertTrue(
            waitForLogGrowth(session.logPath, beyond: before, where: {
                $0.contains("^[[D") || $0.contains("\\u001b[D")
            }),
            "Left should emit ESC [ D — echoed under cat"
        )

        // Exit cat for teardown cleanliness.
        app.buttons["ctrl"].tap()
        terminal().typeText("c")
    }

    /// TC-011 (partial coverage) — covers only the observable third of the
    /// test case: drag gestures are delivered to the terminal's scroll view
    /// and input/output keep working afterwards.
    ///
    /// NOT covered here (unobservable via XCUITest — requires manual
    /// verification): "new output while scrolled up leaves the view in
    /// place" (freeze-while-scrolled) and "scrolling to the bottom resumes
    /// auto-follow". The viewport position has no accessibility
    /// representation: SwiftTerm's scroll indicator reports a stale
    /// "1 page"/0% regardless of scrollback size, and its thumb's a11y
    /// frame flips once on the first gesture then never tracks position
    /// again (verified empirically with swipes, real drags, and idle waits).
    /// Inertia is likewise unassertable.
    func testTouchScrollReachesViewportAndTerminalStaysFunctional() async throws {
        // The session command pre-fills scrollback, then drops to a shell.
        let session = try await daemon.createSession(
            name: "terminal-scroll",
            cmd: "seq 1 150; exec sh"
        )
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "term-prof"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))
        sessionRow(id: session.id).tap()
        waitForTerminalLive()

        XCTAssertTrue(
            verticalScrollBar().waitForExistence(timeout: 10),
            "terminal should expose its native scroll indicator"
        )

        // A drag over the terminal is delivered to the scroll view — the
        // scroll region responds (its accessibility frame changes once the
        // scrollable range materializes).
        let terminal = terminal()
        let before = descendantFrames()
        terminal.swipeDown()
        usleep(600_000)
        var moved = descendantFrames() != before
        if !moved {
            terminal.swipeUp()
            usleep(600_000)
            moved = descendantFrames() != before
        }
        XCTAssertTrue(moved, "a drag gesture should reach the terminal's scroll view")

        // New output still arrives and input still works after scrolling.
        typeInTerminal("echo after_scroll_marker\n")
        XCTAssertTrue(
            waitForLog(session.logPath, where: { $0.contains("after_scroll_marker") }),
            "terminal should still round-trip output after touch scrolling"
        )
    }

    /// TC-012 — leaving the screen must not kill the session, and
    /// reattaching must restore it: after navigate-back the daemon still
    /// reports the session Live (verified via REST — the `dream ps`
    /// equivalent), re-attach reaches `.live` again, the replay source is
    /// untouched, and fresh input round-trips on the same session.
    ///
    /// NOT covered here (unobservable today): that `onDisappear` actually
    /// closed the client socket — the daemon broadcasts to all attached
    /// clients and the session JSON exposes no ws-client count, so a broken
    /// `detach()` would pass this spec. Manual verification or a daemon-side
    /// attach-count signal would be needed.
    func testDetachKeepsSessionAliveAndReattachRestoresScrollback() async throws {
        let session = try await daemon.createSession(
            name: "terminal-reattach",
            cmd: "seq 1 150; exec sh"
        )
        XCTAssertTrue(pairApp(urlString: daemon.pairingURL, profileName: "term-prof"))
        XCTAssertTrue(sessionRow(id: session.id).waitForExistence(timeout: 10))
        sessionRow(id: session.id).tap()
        waitForTerminalLive()

        typeInTerminal("echo reattach_marker\n")
        XCTAssertTrue(waitForLog(session.logPath, where: { $0.contains("reattach_marker") }))
        let logBeforeDetach = logText(session.logPath)

        // Navigate back — onDisappear detaches the socket.
        app.navigationBars.buttons.firstMatch.tap()
        XCTAssertTrue(app.buttons["sessions.create"].waitForExistence(timeout: 10))

        // The daemon-side session must still be alive — the `dream ps`
        // equivalent over REST.
        let observed = try await daemon.session(id: session.id)
        XCTAssertEqual(observed.status, "Live", "detach should leave the session alive")

        // Reattach: attach completes (snapshot applied → live), the daemon
        // still holds the scrollback (replay source intact), and fresh input
        // round-trips on the same session. Whether the *screen* shows the
        // replayed scrollback isn't assertable — the terminal grid isn't in
        // the a11y tree — so the intact-replay evidence is: attach succeeds
        // again and the log that feeds the replay is untouched.
        sessionRow(id: session.id).tap()
        waitForTerminalLive()
        XCTAssertTrue(
            logText(session.logPath).hasPrefix(logBeforeDetach),
            "scrollback source should be intact after reattach — nothing may be rewritten"
        )

        typeInTerminal("echo after_reattach\n")
        XCTAssertTrue(
            waitForLog(session.logPath, where: { $0.contains("after_reattach") }),
            "reattached terminal should accept input on the same live session"
        )
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    /// The SwiftTerm view, whatever element type the a11y tree gives it.
    private func terminal() -> XCUIElement {
        app.descendants(matching: .any)["terminal.view"]
    }

    /// Waits for the view to exist and the connecting overlay to clear —
    /// `.live` is only reached after the snapshot has been applied, so a
    /// clear overlay means the terminal rendered the snapshot.
    private func waitForTerminalLive() {
        let terminal = terminal()
        XCTAssertTrue(terminal.waitForExistence(timeout: 15), "terminal.view should appear")
        let attaching = app.staticTexts["Attaching…"]
        let gone = NSPredicate(format: "exists == false")
        expectation(for: gone, evaluatedWith: attaching, handler: nil)
        waitForExpectations(timeout: 15)
    }

    private func typeInTerminal(_ text: String) {
        terminal().typeText(text)
    }

    /// The replay log on the host filesystem — the test runner shares it.
    private func logText(_ path: String) -> String {
        (try? String(contentsOfFile: path, encoding: .utf8)) ?? ""
    }

    /// Polls the replay log until the predicate holds — filesystem polling,
    /// since `waitForExistence` only covers the accessibility tree.
    private func waitForLog(
        _ path: String,
        timeout: TimeInterval = 10,
        where predicate: (String) -> Bool
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if predicate(logText(path)) { return true }
            usleep(250_000)
        }
        return predicate(logText(path))
    }

    /// Same polling, but only over bytes appended after `offset` — isolates
    /// the echo of a single soft key from earlier output.
    private func waitForLogGrowth(
        _ path: String,
        beyond offset: Int,
        timeout: TimeInterval = 10,
        where predicate: (String) -> Bool
    ) -> Bool {
        waitForLog(path, timeout: timeout) { text in
            guard text.count > offset else { return false }
            let tail = String(text.dropFirst(offset))
            return predicate(tail)
        }
    }

    private func countOccurrences(_ needle: String, in haystack: String) -> Int {
        haystack.components(separatedBy: needle).count - 1
    }

    /// Last `GEOM:<rows> <cols>` marker emitted by `stty size | sed`.
    private func lastGeometry(_ log: String) -> (rows: Int, cols: Int)? {
        guard let regex = try? NSRegularExpression(pattern: "GEOM:(\\d+) (\\d+)") else { return nil }
        let range = NSRange(log.startIndex..<log.endIndex, in: log)
        guard let match = regex.matches(in: log, range: range).last,
              let rowsRange = Range(match.range(at: 1), in: log),
              let colsRange = Range(match.range(at: 2), in: log)
        else { return nil }
        return (Int(log[rowsRange]) ?? 0, Int(log[colsRange]) ?? 0)
    }

    /// The native vertical scroll indicator SwiftTerm gets for free — its
    /// label carries "N pages" of scrollback and its value the position %.
    private func verticalScrollBar() -> XCUIElement {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH 'Vertical scroll bar'"))
            .firstMatch
    }

    /// Frames of everything inside the terminal — a drag that reaches the
    /// scroll view perturbs at least one of them (verified empirically: the
    /// scroll-region child's frame changes once on the first gesture).
    private func descendantFrames() -> [CGRect] {
        let kids = terminal().descendants(matching: .any)
        return (0..<kids.count).map { kids.element(boundBy: $0).frame }
    }
}
