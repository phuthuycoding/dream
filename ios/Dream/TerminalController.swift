import Foundation
import Observation
import SwiftTerm
import UIKit

/// Owns one terminal attach: the SwiftTerm view on one side, the
/// `WSClient` on the other, and the protocol state machine between them
/// (web/terminal.js parity). As the view's `terminalDelegate`, it turns
/// keystrokes into `i` frames and geometry changes into `r` frames.
@MainActor
@Observable
final class TerminalController {
    /// Attach lifecycle, surfaced as the screen's overlay state.
    enum Phase: Equatable {
        case connecting
        /// A snapshot has been applied — the terminal is live.
        case live
        /// `e` notice arrived — the session ended; overlay offers respawn.
        case ended(String)
        /// The socket dropped or failed without an end frame.
        case disconnected(String)
    }

    private(set) var phase: Phase = .connecting
    /// Title the shell program set via OSC — empty until it does.
    private(set) var terminalTitle = ""

    let session: DreamSession
    private let client: APIClient
    private let baseURL: URL
    private let tokenProvider: () -> String?
    /// `a` frame forwarded for the list's attention dot (TASK-005 will
    /// consume this more fully).
    private let onAttention: () -> Void
    private let onUnauthorized: () -> Void

    /// Every live access happens on the main actor; `nonisolated(unsafe)`
    /// only lets `deinit` close the socket. By then the client callbacks'
    /// `weak self` is nil, so nothing escapes the dying controller.
    /// `@ObservationIgnored` — a socket is not render state.
    @ObservationIgnored
    private nonisolated(unsafe) var ws: WSClient?
    @ObservationIgnored
    private weak var terminalView: SwiftTerm.TerminalView?

    init(
        session: DreamSession,
        client: APIClient,
        baseURL: URL,
        tokenProvider: @escaping () -> String?,
        onAttention: @escaping () -> Void,
        onUnauthorized: @escaping () -> Void
    ) {
        self.session = session
        self.client = client
        self.baseURL = baseURL
        self.tokenProvider = tokenProvider
        self.onAttention = onAttention
        self.onUnauthorized = onUnauthorized
    }

    deinit {
        // A controller torn down while still attached (profile switch
        // rebuilds the whole screen) must not leave a socket behind.
        ws?.disconnect()
    }

    /// The representable hands over the view once created.
    func bind(_ view: SwiftTerm.TerminalView) {
        terminalView = view
    }

    /// Opens a fresh socket. The token is read at connect time so a
    /// re-pair between attaches takes effect on the next one.
    func attach() {
        detach()
        guard let url = WSClient.attachURL(
            baseURL: baseURL,
            sessionID: session.id,
            token: tokenProvider()
        ) else {
            phase = .disconnected("The daemon's address is not a valid URL.")
            return
        }
        let ws = WSClient(url: url)
        ws.onOutput = { [weak self] bytes in
            self?.feed(bytes)
        }
        ws.onSnapshot = { [weak self] scrollback, payload in
            self?.applySnapshot(scrollback: scrollback, payload: payload)
        }
        ws.onEnd = { [weak self] notice in
            self?.phase = .ended(notice)
        }
        ws.onAttention = { [weak self] in
            self?.onAttention()
        }
        ws.onClose = { [weak self] error in
            self?.didClose(error: error)
        }
        self.ws = ws
        phase = .connecting
        ws.connect()
    }

    /// Detach leaves the session alive — it closes the socket and
    /// nothing else. Reattach gets the scrollback replay from the daemon.
    func detach() {
        guard let old = ws else { return }
        ws = nil
        old.disconnect()
    }

    /// The `e` overlay's affordance: `POST …/respawn`, then re-attach.
    func respawn() {
        phase = .connecting
        Task { [weak self] in
            guard let self else { return }
            do {
                _ = try await client.respawnSession(id: session.id)
                attach()
            } catch APIError.unauthorized {
                onUnauthorized()
            } catch {
                phase = .disconnected(APIError.describe(error))
            }
        }
    }

    private func feed(_ bytes: Data) {
        terminalView?.feed(byteArray: ArraySlice(bytes))
    }

    /// Applies one `s` payload the way the web client does: resize the
    /// emulator to the dump's geometry first (its cells map 1:1 onto the
    /// screen), then replay the buffered scrollback, then the snapshot.
    /// The snapshot's own `\x1bc` is the emulator reset, so a mid-stream
    /// snapshot is also the resync path — nothing extra to clear.
    private func applySnapshot(scrollback: [Data], payload: Data) {
        guard let view = terminalView else { return }
        if let geometry = Self.snapshotGeometry(payload) {
            // resize() reports back through sizeChanged → the `r` frame
            // the protocol asks for after a snapshot goes out there.
            view.resize(cols: geometry.cols, rows: geometry.rows)
        }
        for chunk in scrollback {
            view.feed(byteArray: ArraySlice(chunk))
        }
        view.feed(byteArray: ArraySlice(payload))
        phase = .live
    }

    /// `\x1b[8;<rows>;<cols>t` — the geometry field the daemon writes
    /// right after the snapshot's RIS (`src/vt/snapshot.rs`). Scans only
    /// the head like the web's `parseGeometry`.
    static func snapshotGeometry(_ payload: Data) -> (cols: Int, rows: Int)? {
        let head = payload.prefix(64)
        let marker: [UInt8] = [0x1B, 0x5B, 0x38, 0x3B] // ESC [ 8 ;
        guard let range = head.firstRange(of: marker) else { return nil }
        var index = range.upperBound

        func readNumber() -> Int? {
            var value = 0
            var found = false
            while index < head.endIndex, head[index] >= 0x30, head[index] <= 0x39 {
                value = value * 10 + Int(head[index] - 0x30)
                found = true
                index = head.index(after: index)
            }
            return found ? value : nil
        }

        func expect(_ byte: UInt8) -> Bool {
            guard index < head.endIndex, head[index] == byte else { return false }
            index = head.index(after: index)
            return true
        }

        // Wire order is rows;cols, like xterm's window-op resize.
        guard let rows = readNumber(), expect(0x3B), // ';'
              let cols = readNumber(), expect(0x74), // 't'
              rows > 0, cols > 0
        else { return nil }
        return (cols, rows)
    }

    private func didClose(error: Error?) {
        // A replaced or detached socket reports through here too — only
        // the live one may change the phase, and an `e` already shown
        // wins over the close that follows it on the wire.
        guard ws != nil else { return }
        ws = nil
        if case .ended = phase { return }
        let reason = error.map { APIError.describe($0) } ?? "The daemon closed the connection."
        phase = .disconnected(reason)
    }
}

extension TerminalController: TerminalViewDelegate {
    /// Any geometry change — layout, rotation, keyboard, or our own
    /// snapshot-driven `resize()` — goes to the daemon as an `r` frame.
    func sizeChanged(source: SwiftTerm.TerminalView, newCols: Int, newRows: Int) {
        ws?.sendResize(cols: newCols, rows: newRows)
    }

    /// Keystrokes and soft-key bytes → `i` frames.
    func send(source: SwiftTerm.TerminalView, data: ArraySlice<UInt8>) {
        ws?.sendInput(Data(data))
    }

    func setTerminalTitle(source: SwiftTerm.TerminalView, title: String) {
        terminalTitle = title
    }

    func hostCurrentDirectoryUpdate(source: SwiftTerm.TerminalView, directory: String?) {}

    func scrolled(source: SwiftTerm.TerminalView, position: Double) {}

    func bell(source: SwiftTerm.TerminalView) {}

    func requestOpenLink(source: SwiftTerm.TerminalView, link: String, params: [String: String]) {
        guard let url = URL(string: link), url.scheme == "http" || url.scheme == "https" else {
            return
        }
        UIApplication.shared.open(url)
    }

    func clipboardCopy(source: SwiftTerm.TerminalView, content: Data) {
        UIPasteboard.general.string = String(decoding: content, as: UTF8.self)
    }

    /// Reads are denied — handing terminal apps the pasteboard needs a
    /// user-consent story this task does not have.
    func clipboardRead(source: SwiftTerm.TerminalView) -> Data? {
        nil
    }

    /// No explicit Kitty clipboard services — the terminal falls back to
    /// the platform pasteboard paths above.
    func kittyClipboardCapabilities(source: SwiftTerm.TerminalView) -> KittyClipboardCapabilities {
        []
    }

    func kittyClipboardAvailableMimeTypes(
        source: SwiftTerm.TerminalView,
        location: KittyClipboardLocation
    ) -> [String]? {
        nil
    }

    func kittyClipboardRead(
        source: SwiftTerm.TerminalView,
        location: KittyClipboardLocation,
        mimeType: String
    ) -> KittyClipboardReadResult? {
        nil
    }

    func kittyClipboardWrite(
        source: SwiftTerm.TerminalView,
        location: KittyClipboardLocation,
        content: KittyClipboardWriteContent
    ) -> KittyClipboardWriteResult {
        .unsupported
    }

    func kittyClipboardRequestPermission(
        source: SwiftTerm.TerminalView,
        request: KittyClipboardPermissionRequest
    ) -> KittyClipboardPermissionResult {
        .deny
    }

    func iTermContent(source: SwiftTerm.TerminalView, content: ArraySlice<UInt8>) {}

    func rangeChanged(source: SwiftTerm.TerminalView, startY: Int, endY: Int) {}
}
