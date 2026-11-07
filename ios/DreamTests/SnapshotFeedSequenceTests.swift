import XCTest
@testable import Dream

/// `TerminalController.snapshotFeedSequence` — the ordered chunks
/// `applySnapshot` paints for one `s` payload: the payload's own RIS
/// first (it clears modes a previous attach left behind), then the
/// scrollback replay, then the payload minus that leading reset —
/// re-feeding the RIS after the replay would wipe the scrollback just
/// restored. (The pending-feed drain and the geometry resize run in
/// `applySnapshot` against the live view, ahead of these chunks; the
/// pure sequence pins the feed order that follows them.)
@MainActor
final class SnapshotFeedSequenceTests: XCTestCase {
    /// `ESC c` — the full reset the daemon writes as a snapshot's
    /// first bytes.
    private let ris = Data([0x1B, 0x63])

    func testLeadingRISFeedsAheadOfScrollbackThenTheStrippedPayload() {
        let scrollback = [Data("older\n".utf8), Data("newer\n".utf8)]
        let remainder = Data("\u{1B}[8;24;80tdump".utf8)
        let payload = ris + remainder

        let chunks = TerminalController.snapshotFeedSequence(
            scrollback: scrollback,
            payload: payload
        )

        XCTAssertEqual(
            chunks,
            [ris, scrollback[0], scrollback[1], remainder],
            "reset first, replay in order, then the dump without its second reset"
        )
    }

    func testEmptyScrollbackYieldsResetThenPayloadRemainder() {
        let remainder = Data("\u{1B}[8;24;80tdump".utf8)

        XCTAssertEqual(
            TerminalController.snapshotFeedSequence(
                scrollback: [],
                payload: ris + remainder
            ),
            [ris, remainder]
        )
    }

    func testRISLessPayloadPassesThroughAfterTheScrollback() {
        // A snapshot whose dump does not open with RIS gets no
        // synthetic reset prepended — the payload feeds intact.
        let scrollback = Data("replay\n".utf8)
        let payload = Data("plain dump".utf8)

        XCTAssertEqual(
            TerminalController.snapshotFeedSequence(
                scrollback: [scrollback],
                payload: payload
            ),
            [scrollback, payload]
        )
    }

    func testSoftResetIsNotStrippedOrReFedAsRIS() {
        // `ESC [ ! p` is a DEC soft reset, not `ESC c` — it belongs to
        // the payload and must survive the sequence untouched.
        let payload = Data("\u{1B}[!pdump".utf8)

        XCTAssertEqual(
            TerminalController.snapshotFeedSequence(
                scrollback: [],
                payload: payload
            ),
            [payload],
            "a leading soft reset is not the snapshot's RIS"
        )
    }

    func testPayloadThatIsOnlyARISStillEndsWithTheStrippedRemainder() {
        XCTAssertEqual(
            TerminalController.snapshotFeedSequence(
                scrollback: [],
                payload: ris
            ),
            [ris, Data()],
            "the sequence always ends with the payload-minus-RIS chunk"
        )
    }

    func testScrollbackFeedsBetweenTheResetAndThePayloadNotBeforeIt() throws {
        // Regression shape for the ordering hazard the seam exists to
        // pin: fed after the replay, the RIS would wipe the scrollback
        // the replay just restored.
        let scrollback = Data("replayed".utf8)
        let payload = ris + Data("dump".utf8)

        let chunks = TerminalController.snapshotFeedSequence(
            scrollback: [scrollback],
            payload: payload
        )

        XCTAssertEqual(chunks.first, ris, "the reset must paint before any replay")
        let risIndex = try XCTUnwrap(chunks.firstIndex(of: ris))
        let replayIndex = try XCTUnwrap(chunks.firstIndex(of: scrollback))
        XCTAssertLessThan(
            risIndex,
            replayIndex,
            "the RIS precedes the scrollback it prepares the emulator for"
        )
        XCTAssertEqual(chunks.last, Data("dump".utf8))
    }
}
