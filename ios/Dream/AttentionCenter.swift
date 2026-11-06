import Foundation
import Observation
import UserNotifications

/// Local-notification bridge for the daemon's `attention` flag — local
/// banners only, there is no APNs channel. The session poll (and the
/// attached socket's `a` frame via the re-list it triggers) feeds
/// `observe(_:)`; a false→true edge posts a notification unless that
/// session's terminal is on screen — attaching already acks its flag —
/// and a true→false edge retracts the banner since it was acked
/// elsewhere. Taps route through `requestedSessionID`, which the
/// session list drains into a navigation.
@MainActor
@Observable
final class AttentionCenter: NSObject {
    /// Session id a notification tap asked to open — set by the
    /// delegate, drained by the session list via `consumeRequest()`.
    private(set) var requestedSessionID: String?
    /// Session whose terminal is on screen right now. Its own edge is
    /// suppressed: the attach already acknowledged the flag.
    @ObservationIgnored var foregroundedSessionID: String?

    /// Session ids the last observation reported as flagged — the edge
    /// detector's baseline. Not render state.
    @ObservationIgnored private var flagged: Set<String> = []
    /// The first observation only seeds `flagged` — a flag already set
    /// when the app opened is not a fresh edge.
    @ObservationIgnored private var primed = false

    override init() {
        super.init()
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        // Asked at launch so the first flag can already notify. A
        // denial only mutes banners — the in-app badge keeps working.
        center.requestAuthorization(options: [.alert, .sound]) { _, error in
            if let error {
                NSLog("AttentionCenter: notification permission request failed: \(error.localizedDescription)")
            }
        }
    }

    /// Diff one session-list snapshot against the previous one: rising
    /// attention edges notify (except the foregrounded terminal's),
    /// cleared flags retract their pending and delivered banners.
    func observe(_ sessions: [DreamSession]) {
        let now = Set(sessions.filter(\.attention).map(\.id))
        defer { flagged = now }
        guard primed else {
            primed = true
            return
        }
        for session in sessions where session.attention && !flagged.contains(session.id) {
            if session.id != foregroundedSessionID {
                notify(session)
            }
        }
        let cleared = Array(flagged.subtracting(now))
        if !cleared.isEmpty {
            let center = UNUserNotificationCenter.current()
            center.removePendingNotificationRequests(withIdentifiers: cleared)
            center.removeDeliveredNotifications(withIdentifiers: cleared)
        }
    }

    /// Returns and clears the pending deep-link, if any.
    func consumeRequest() -> String? {
        defer { requestedSessionID = nil }
        return requestedSessionID
    }

    private func notify(_ session: DreamSession) {
        let content = UNMutableNotificationContent()
        content.title = "\"\(session.name)\" wants attention"
        content.body = session.cmd
        content.sound = .default
        content.userInfo = [attentionSessionIDKey: session.id]
        // A nil trigger delivers immediately; the request identifier is
        // the session id so a re-flag replaces rather than stacks.
        let request = UNNotificationRequest(identifier: session.id, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request) { error in
            if let error {
                NSLog("AttentionCenter: failed to post attention notification for session \(session.id): \(error.localizedDescription)")
            }
        }
    }
}

/// `userInfo` key carrying the session id in attention notifications —
/// file scope so the nonisolated delegate can read it.
private let attentionSessionIDKey = "sessionID"

extension AttentionCenter: UNUserNotificationCenterDelegate {
    /// Attention banners also present while the app is foreground —
    /// the flag is meant to interrupt whatever screen is up.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    /// A tap records the session id; the list navigates once it (or a
    /// refresh) confirms the session still exists.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let sessionID = response.notification.request.content.userInfo[attentionSessionIDKey] as? String
        Task { @MainActor in
            self.requestedSessionID = sessionID
            completionHandler()
        }
    }
}
