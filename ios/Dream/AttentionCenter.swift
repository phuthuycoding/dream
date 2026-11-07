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
    /// Profile the tapped banner was posted for — a tap that lands
    /// after a profile switch must not be consumed by the wrong
    /// daemon's list (its session id would mean nothing there).
    private(set) var requestedProfileID: UUID?
    /// Session whose terminal is on screen right now. Its own edge is
    /// suppressed: the attach already acknowledged the flag.
    @ObservationIgnored var foregroundedSessionID: String?

    /// The profile whose daemon the edge detector listens to — the
    /// session list sets it when it becomes the active browser.
    /// Switching it re-primes the baseline so the new daemon's
    /// pre-existing flags do not fire as stale edges, and a refresh
    /// that resolves after the switch is ignored rather than diffed
    /// against the other daemon's baseline.
    @ObservationIgnored var activeProfileID: UUID? {
        didSet {
            guard activeProfileID != oldValue else { return }
            flagged.removeAll()
            primed = false
        }
    }

    /// Session ids the last observation reported as flagged — the edge
    /// detector's baseline. Not render state.
    @ObservationIgnored private var flagged: Set<String> = []
    /// The first observation of a profile only seeds `flagged` — a flag
    /// already set when observation started is not a fresh edge.
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
    /// Snapshots belonging to another profile — a poll issued before a
    /// profile switch that resolved after it — are dropped.
    func observe(_ sessions: [DreamSession], for profileID: UUID) {
        guard profileID == activeProfileID else { return }
        let now = Set(sessions.filter(\.attention).map(\.id))
        defer { flagged = now }
        guard primed else {
            primed = true
            return
        }
        for session in sessions where session.attention && !flagged.contains(session.id) {
            if session.id != foregroundedSessionID {
                notify(session, for: profileID)
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
        defer {
            requestedSessionID = nil
            requestedProfileID = nil
        }
        return requestedSessionID
    }

    private func notify(_ session: DreamSession, for profileID: UUID) {
        let content = UNMutableNotificationContent()
        content.title = "\"\(session.name)\" wants attention"
        content.body = session.cmd
        content.sound = .default
        content.userInfo = [
            attentionSessionIDKey: session.id,
            attentionProfileIDKey: profileID.uuidString,
        ]
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

/// `userInfo` keys carrying the session id and owning profile id in
/// attention notifications — file scope so the nonisolated delegate
/// can read them.
private let attentionSessionIDKey = "sessionID"
private let attentionProfileIDKey = "profileID"

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
        let userInfo = response.notification.request.content.userInfo
        let sessionID = userInfo[attentionSessionIDKey] as? String
        let profileID = (userInfo[attentionProfileIDKey] as? String).flatMap(UUID.init(uuidString:))
        Task { @MainActor in
            self.requestedSessionID = sessionID
            self.requestedProfileID = profileID
            completionHandler()
        }
    }
}
