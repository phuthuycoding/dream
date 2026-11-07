import Foundation

/// WebSocket attach client for one daemon session. Wraps a single
/// `URLSessionWebSocketTask` on `ws(s)://<base>/api/sessions/<id>/ws`
/// and owns the attach-ordering rules of `src/ws/attach.rs`, mirroring
/// `web/terminal.js`:
///
/// - `o` frames before the first `s` are buffered scrollback — handed to
///   `onSnapshot(scrollback:)` ahead of the snapshot payload, or flushed
///   on `e`/close for a dead session (which never sends `s`, so the
///   client must never block waiting for one).
/// - A mid-stream `s` (broadcast lag resync) arrives through the same
///   callback with an empty scrollback; the payload's own RIS reset is
///   what resyncs the emulator.
/// - `e` flushes any buffered replay, then reports the notice text.
///
/// Every callback fires on the main queue. Bad peer frames — unknown
/// opcodes, text messages — are dropped, never fatal.
final class WSClient: NSObject {
    /// One binary `o` payload, live or flushed replay.
    var onOutput: (Data) -> Void = { _ in }
    /// One `s` payload plus the `o` chunks buffered before it. Feed the
    /// scrollback first, then the snapshot, at the snapshot's own
    /// `\x1b[8;<rows>;<cols>t` geometry.
    var onSnapshot: (_ scrollback: [Data], _ payload: Data) -> Void = { _, _ in }
    /// `e` notice text — the session is over; the text names the state
    /// and the respawn route.
    var onEnd: (String) -> Void = { _ in }
    /// `a` — the session wants attention.
    var onAttention: () -> Void = {}
    /// The upgrade answered 401 — the credential was rejected; the
    /// re-pair route applies. Fires before `onClose`.
    var onUnauthorized: () -> Void = {}
    /// The upgrade answered 404 — the session no longer exists on the
    /// daemon; the session list should refresh. Fires before `onClose`.
    var onSessionGone: () -> Void = {}
    /// The socket closed; `error` is nil for a clean close.
    var onClose: (Error?) -> Void = { _ in }

    private let url: URL
    private var session: URLSession?
    private var task: URLSessionWebSocketTask?
    private var pendingOutput: [Data] = []
    private var snapshotApplied = false
    private var finished = false
    /// Keepalive — a dead socket that never reported an error still
    /// answers the ping timeout. Not protocol state.
    private var pingTimer: Timer?

    init(url: URL) {
        self.url = url
    }

    /// The attach URL for one session. `?token=` is the only credential
    /// form the server gate accepts on this route from a client that
    /// cannot set `Authorization` on the upgrade — and it is sent only
    /// when a token exists, never as an empty credential.
    static func attachURL(baseURL: URL, sessionID: String, token: String?) -> URL? {
        // Append like APIClient.sessionURL — a path-prefixed baseURL
        // (daemon behind a reverse proxy at e.g. https://host/dream/)
        // must keep its prefix or every attach misses the route.
        let wsURL = baseURL
            .appendingPathComponent("api")
            .appendingPathComponent("sessions")
            .appendingPathComponent(sessionID)
            .appendingPathComponent("ws")
        guard var components = URLComponents(url: wsURL, resolvingAgainstBaseURL: false) else {
            return nil
        }
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        components.queryItems = nil
        components.fragment = nil
        if let token, !token.isEmpty {
            components.queryItems = [URLQueryItem(name: "token", value: token)]
        }
        return components.url
    }

    func connect() {
        guard task == nil else { return }
        let session = URLSession(configuration: .default, delegate: self, delegateQueue: .main)
        let task = session.webSocketTask(with: url)
        self.session = session
        self.task = task
        task.resume()
        receiveNext()
        // The ~15s keepalive arms in armPingTimer() once the upgrade is
        // proven live — pinging a still-connecting socket would fail
        // and kill a handshake that was about to succeed.
    }

    /// ~15s keepalive — silent stalls (a suspend or a dead route) are
    /// reported through the pong error, not left hanging forever. Armed
    /// by the first completed receive, i.e. after the handshake has
    /// actually finished; idempotent across frames.
    private func armPingTimer() {
        guard !finished, pingTimer == nil else { return }
        pingTimer = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in
            self?.task?.sendPing { [weak self] error in
                if let error {
                    self?.finish(error: error)
                }
            }
        }
    }

    /// `i` frame — keystroke bytes for the pty's stdin.
    func sendInput(_ bytes: Data) {
        send(.input(bytes))
    }

    /// `r` frame — cols, rows as two big-endian u16 fields.
    func sendResize(cols: Int, rows: Int) {
        send(.resize(cols: UInt16(clamping: cols), rows: UInt16(clamping: rows)))
    }

    /// Detach: a clean close and nothing else — the session keeps
    /// running on the daemon, and reattach gets the scrollback replay.
    func disconnect() {
        guard !finished else { return }
        finish(error: nil)
    }

    private func send(_ frame: Frame) {
        guard !finished, let task else { return }
        task.send(.data(frame.encode())) { [weak self] error in
            // A failed send means the socket is broken — report it once
            // through the same path as a dropped connection.
            if let error {
                self?.finish(error: error)
            }
        }
    }

    private func receiveNext() {
        task?.receive { [weak self] result in
            guard let self, !self.finished else { return }
            switch result {
            case .success(let message):
                // A completed receive means the upgrade finished — the
                // keepalive only makes sense on a live socket.
                self.armPingTimer()
                if case .data(let data) = message {
                    self.handle(data)
                }
                // Text frames are not in the protocol — drop, keep reading.
                self.receiveNext()
            case .failure(let error):
                // A rejected upgrade also fails the pending receive, and
                // that error can land before didCompleteWithError —
                // classify it here too so whichever teardown signal
                // arrives first routes 401/404; the later one is
                // `!finished`-guarded.
                self.classifyUpgradeFailure(of: self.task)
                self.finish(error: error)
            }
        }
    }

    private func handle(_ data: Data) {
        guard let frame = Frame.decode(data) else { return }
        switch frame {
        case .output(let payload):
            if snapshotApplied {
                onOutput(payload)
            } else {
                pendingOutput.append(payload)
            }
        case .snapshot(let payload):
            let scrollback = pendingOutput
            pendingOutput.removeAll()
            snapshotApplied = true
            onSnapshot(scrollback, payload)
        case .end(let payload):
            // Dead-session attach: the replayed `o` frames arrive without
            // a snapshot — flush them so the final screen shows.
            flushPending()
            onEnd(String(decoding: payload, as: UTF8.self))
        case .attention:
            onAttention()
        case .input, .resize:
            break // server never sends client opcodes — drop
        }
    }

    private func flushPending() {
        let chunks = pendingOutput
        pendingOutput.removeAll()
        for chunk in chunks {
            onOutput(chunk)
        }
    }

    /// The HTTP status a failed upgrade leaves on `task.response` —
    /// 401 routes to re-pair, 404 names the dead session. Runs before
    /// `finish` on the receive-error and delegate paths alike, so the
    /// first teardown signal to land classifies; a live socket's plain
    /// drop (nil or 101 response) falls through to `default`.
    private func classifyUpgradeFailure(of task: URLSessionTask?) {
        switch (task?.response as? HTTPURLResponse)?.statusCode {
        case 401: onUnauthorized()
        case 404: onSessionGone()
        default: break
        }
    }

    /// Terminal teardown — exactly once, always through here so a close,
    /// a receive error, and a deliberate detach converge.
    private func finish(error: Error?) {
        guard !finished else { return }
        finished = true
        pingTimer?.invalidate()
        pingTimer = nil
        flushPending() // never strand buffered replay bytes
        task?.cancel(with: .normalClosure, reason: nil)
        session?.invalidateAndCancel()
        task = nil
        session = nil
        onClose(error)
    }
}

extension WSClient: URLSessionWebSocketDelegate {
    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
        reason: Data?
    ) {
        finish(error: nil)
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        didCompleteWithError error: Error?
    ) {
        // A superseded socket's late callback must not classify against
        // the new attach — its 401 would report (and clear) a credential
        // it never presented.
        guard !finished else { return }
        // A failed upgrade carries the HTTP status on task.response —
        // classify it before the generic close so 401 routes to re-pair
        // and 404 names the dead session instead of a bare error. The
        // receive-error path classifies too, so this may be the second
        // signal — the first one already ran and set `finished`.
        classifyUpgradeFailure(of: task)
        finish(error: error)
    }
}
