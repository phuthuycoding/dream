import XCTest
@testable import Dream

/// `AttachSequencer.action(for:snapshotApplied:)` — the receive-path
/// decision that gives `o` frames a split personality: scrollback
/// replay until the first `s` lands, live pty bytes after it. `e`
/// flushes whatever replay is still held, which is what lets a dead
/// session (one that never sends `s`) paint its final screen instead
/// of blocking on a snapshot that is not coming.
final class AttachSequencerTests: XCTestCase {

    // MARK: - Output routing

    func testOutputBeforeTheFirstSnapshotIsHeldAsScrollback() {
        let payload = Data("replay".utf8)

        XCTAssertEqual(
            AttachSequencer.action(for: .output(payload), snapshotApplied: false),
            .bufferScrollback(payload),
            "pre-snapshot output is scrollback — it must wait for the snapshot's geometry"
        )
    }

    func testOutputAfterTheFirstSnapshotIsDeliveredLive() {
        let payload = Data("live".utf8)

        XCTAssertEqual(
            AttachSequencer.action(for: .output(payload), snapshotApplied: true),
            .deliverOutput(payload),
            "post-snapshot output is live pty bytes — feed the emulator now"
        )
    }

    // MARK: - Snapshot

    func testFirstSnapshotAppliesItsPayload() {
        let payload = Data("\u{1B}cdump".utf8)

        XCTAssertEqual(
            AttachSequencer.action(for: .snapshot(payload), snapshotApplied: false),
            .applySnapshot(payload)
        )
    }

    func testMidStreamSnapshotAppliesTheSameAsTheFirst() {
        // Broadcast-lag resync: an `s` after the attach is already live
        // repaints through the same path — the payload's own RIS is
        // what resets the emulator.
        let payload = Data("\u{1B}cdump".utf8)

        XCTAssertEqual(
            AttachSequencer.action(for: .snapshot(payload), snapshotApplied: true),
            .applySnapshot(payload)
        )
    }

    // MARK: - End / attention / client opcodes

    func testEndFlushesHeldReplayThenReportsTheNotice() {
        let notice = Data("session exited — respawn from the list".utf8)

        XCTAssertEqual(
            AttachSequencer.action(for: .end(notice), snapshotApplied: false),
            .flushEnd(notice),
            "a dead session's replayed output must flush even though no snapshot ever arrived"
        )
        XCTAssertEqual(
            AttachSequencer.action(for: .end(notice), snapshotApplied: true),
            .flushEnd(notice)
        )
    }

    func testAttentionMapsToTheAttentionAction() {
        XCTAssertEqual(
            AttachSequencer.action(for: .attention, snapshotApplied: false),
            .attention
        )
        XCTAssertEqual(
            AttachSequencer.action(for: .attention, snapshotApplied: true),
            .attention
        )
    }

    func testClientOpcodesAreDroppedRegardlessOfSnapshotState() {
        for applied in [false, true] {
            XCTAssertEqual(
                AttachSequencer.action(for: .input(Data("x".utf8)), snapshotApplied: applied),
                .drop,
                "the server never sends `i` — a reflected client opcode is dropped"
            )
            XCTAssertEqual(
                AttachSequencer.action(
                    for: .resize(cols: 80, rows: 24),
                    snapshotApplied: applied
                ),
                .drop
            )
        }
    }

    // MARK: - Stream walks

    /// Replays a frame stream through the decision the way `handle`
    /// does — `snapshotApplied` flips once an applySnapshot action
    /// comes back — and pins the whole ordering contract.
    private func actions(for frames: [Frame]) -> [AttachSequencer.Action] {
        var snapshotApplied = false
        return frames.map { frame in
            let action = AttachSequencer.action(for: frame, snapshotApplied: snapshotApplied)
            if case .applySnapshot = action { snapshotApplied = true }
            return action
        }
    }

    func testLiveAttachBuffersReplayUntilTheSnapshotThenGoesLive() {
        let replayA = Data("line a\n".utf8)
        let replayB = Data("line b\n".utf8)
        let dump = Data("\u{1B}cdump".utf8)
        let live = Data("prompt$ ".utf8)

        XCTAssertEqual(
            actions(for: [
                .output(replayA), .output(replayB),
                .snapshot(dump),
                .output(live),
            ]),
            [
                .bufferScrollback(replayA), .bufferScrollback(replayB),
                .applySnapshot(dump),
                .deliverOutput(live),
            ],
            "the snapshot is the seam between buffered replay and live output"
        )
    }

    func testDeadSessionReplayBuffersEverythingThenFlushesOnEnd() {
        let replayA = Data("last command\n".utf8)
        let replayB = Data("final screen\n".utf8)
        let notice = Data("exited".utf8)

        XCTAssertEqual(
            actions(for: [.output(replayA), .output(replayB), .end(notice)]),
            [.bufferScrollback(replayA), .bufferScrollback(replayB), .flushEnd(notice)],
            "a session that ended before the attach sends `o`s and `e` but never `s`"
        )
    }

    func testSecondSnapshotKeepsDeliveringLiveOutput() {
        let dump = Data("\u{1B}cdump".utf8)
        let resync = Data("\u{1B}cresync".utf8)
        let live = Data("x".utf8)

        XCTAssertEqual(
            actions(for: [
                .snapshot(dump), .output(live),
                .snapshot(resync), .output(live),
            ]),
            [
                .applySnapshot(dump), .deliverOutput(live),
                .applySnapshot(resync), .deliverOutput(live),
            ],
            "a mid-stream resync must not send output back to the scrollback buffer"
        )
    }
}
