import SwiftUI

/// Session browser for the active daemon — mirrors `web/dashboard.js`
/// semantics: the list is the source of truth, every mutation re-lists,
/// `resurrected` wins over a plain Live badge, and respawn only shows
/// on Dead rows. Polls every ~15s while the scene is active so the
/// attention dots stay live.
struct SessionListView: View {
    let profile: DaemonProfile
    let client: APIClient

    @Environment(AppState.self) private var appState
    @Environment(AttentionCenter.self) private var attentionCenter
    @Environment(\.scenePhase) private var scenePhase

    @State private var sessions: [DreamSession] = []
    @State private var loaded = false
    @State private var banner: String?
    @State private var showCreate = false
    @State private var showProfiles = false
    @State private var renaming: DreamSession?
    @State private var renameText = ""
    @State private var deleting: DreamSession?
    @State private var failure: String?
    /// Notification-tap target — a pending item pushes its terminal.
    @State private var deepLinked: DreamSession?
    @State private var missingSessionNotice = false

    var body: some View {
        NavigationStack {
            decoratedContent
                .alert("Session unavailable", isPresented: $missingSessionNotice) {
                    Button("OK") {}
                } message: {
                    Text("The session no longer exists on \(profile.name).")
                }
                .navigationDestination(item: $deepLinked) { session in
                    terminalScreen(for: session)
                }
                .onChange(of: attentionCenter.requestedSessionID) { _, _ in
                    drainRequest()
                }
                .task(id: scenePhase) {
                    guard scenePhase == .active else { return }
                    await refresh()
                    while !Task.isCancelled {
                        do {
                            try await Task.sleep(for: .seconds(15))
                        } catch {
                            // Cancellation — backgrounding or teardown stops the poll.
                            return
                        }
                        await refresh()
                    }
                }
        }
    }

    /// The list plus its toolbar, sheets, and alerts — a separate
    /// expression so the full modifier chain stays type-checkable.
    private var decoratedContent: some View {
        VStack(spacing: 0) {
            if let banner {
                Label(banner, systemImage: "wifi.exclamationmark")
                    .font(.footnote)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(8)
                    .background(.yellow.opacity(0.2))
                    .accessibilityIdentifier("sessions.banner")
            }
            content
        }
        .navigationTitle(profile.name)
            .toolbar {
                ToolbarItem(placement: .navigation) {
                    Button { showProfiles = true } label: {
                        Image(systemName: "server.rack")
                    }
                    .accessibilityIdentifier("sessions.profiles")
                }
                ToolbarItem(placement: .primaryAction) {
                    Button { showCreate = true } label: {
                        Image(systemName: "plus")
                    }
                    .accessibilityIdentifier("sessions.create")
                }
            }
            .sheet(isPresented: $showCreate) {
                SessionFormView { name, cmd, cwd in
                    do {
                        _ = try await client.createSession(name: name, cmd: cmd, cwd: cwd)
                        await refresh()
                    } catch APIError.unauthorized {
                        appState.handleUnauthorized(profileId: profile.id)
                        throw APIError.unauthorized
                    }
                }
            }
            .sheet(isPresented: $showProfiles) {
                ProfileListView()
            }
            .alert("Rename session", isPresented: renamePresented, presenting: renaming) { session in
                TextField("Name", text: $renameText)
                Button("Save") { rename(session) }
                Button("Cancel", role: .cancel) {}
            }
            .alert(deleteTitle, isPresented: deletePresented, presenting: deleting) { session in
                Button("Delete", role: .destructive) {
                    run { try await client.deleteSession(id: session.id) }
                }
                Button("Cancel", role: .cancel) {}
            } message: { _ in
                Text("Its state and replay log are removed.")
            }
            .alert("Action failed", isPresented: failurePresented) {
                Button("OK") {}
            } message: {
                Text(failure ?? "")
            }
    }

    @ViewBuilder
    private var content: some View {
        if !loaded {
            ProgressView("Loading sessions…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if sessions.isEmpty {
            ContentUnavailableView {
                Label("No sessions", systemImage: "terminal")
            } description: {
                Text(banner ?? "Create a session on \(profile.name) to get started.")
            } actions: {
                Button("Create session") { showCreate = true }
                    .accessibilityIdentifier("sessions.empty.create")
            }
        } else {
            List(sessions, id: \.id) { session in
                NavigationLink {
                    terminalScreen(for: session)
                } label: {
                    SessionRow(session: session)
                }
                .accessibilityIdentifier("session.row.\(session.id)")
                .swipeActions(edge: .trailing) {
                    Button("Delete", role: .destructive) { deleting = session }
                    lifecycleButton(session)
                }
                .contextMenu {
                    Button("Rename") {
                        renaming = session
                        renameText = session.name
                    }
                    lifecycleButton(session)
                    Button("Clean replay log") {
                        run { try await client.cleanSession(id: session.id) }
                    }
                    Button("Delete", role: .destructive) { deleting = session }
                }
            }
            .refreshable { await refresh() }
        }
    }

    /// Kill for live shells, respawn for dead ones — dashboard.js shows
    /// exactly one of the two, never both.
    @ViewBuilder
    private func lifecycleButton(_ session: DreamSession) -> some View {
        if session.status == .dead {
            Button("Respawn") {
                run { _ = try await client.respawnSession(id: session.id) }
            }
        } else {
            Button("Kill") {
                run { _ = try await client.killSession(id: session.id) }
            }
        }
    }

    private func refresh() async {
        do {
            sessions = try await client.listSessions()
            // Feed the attention edge detector before anything drains a
            // deep-link so the row snapshot it routes against is fresh.
            attentionCenter.observe(sessions)
            banner = nil
        } catch APIError.unauthorized {
            appState.handleUnauthorized(profileId: profile.id)
            return
        } catch APIError.unreachable(let error) {
            banner = "Daemon unreachable — \(error.localizedDescription)"
        } catch {
            banner = APIError.describe(error)
        }
        loaded = true
        drainRequest()
    }

    /// The attach both row taps and notification taps build.
    private func terminalScreen(for session: DreamSession) -> some View {
        TerminalScreenView(
            session: session,
            client: client,
            baseURL: profile.baseURL,
            tokenProvider: { appState.token(for: profile.id) },
            // The `a` frame means the shell wants attention —
            // re-list so the row's dot is fresh on pop-back.
            onAttention: { Task { await refresh() } },
            onUnauthorized: { appState.handleUnauthorized(profileId: profile.id) }
        )
    }

    /// A notification tap queued a session id: push its terminal when
    /// the row still exists. A tap that raced the app's launch arrives
    /// before the first list, so a miss re-lists once before the
    /// dead-session notice.
    private func drainRequest() {
        guard let id = attentionCenter.consumeRequest() else { return }
        Task {
            if sessions.first(where: { $0.id == id }) == nil {
                await refresh()
            }
            if let session = sessions.first(where: { $0.id == id }) {
                deepLinked = session
            } else {
                missingSessionNotice = true
            }
        }
    }

    /// Mutations re-list afterwards so the rows always reflect the
    /// daemon's state, not an optimistic guess.
    private func run(_ action: @escaping () async throws -> Void) {
        Task {
            do {
                try await action()
                await refresh()
            } catch APIError.unauthorized {
                appState.handleUnauthorized(profileId: profile.id)
            } catch {
                failure = APIError.describe(error)
            }
        }
    }

    private func rename(_ session: DreamSession) {
        let newName = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !newName.isEmpty, newName != session.name else { return }
        run { _ = try await client.updateSession(id: session.id, name: newName) }
    }

    private var deleteTitle: String {
        "Delete \"\(deleting?.name ?? "session")\"?"
    }

    private var renamePresented: Binding<Bool> {
        Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })
    }

    private var deletePresented: Binding<Bool> {
        Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } })
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })
    }
}

/// One list row: status dot, name, attention badge, cmd, relative
/// created time, and the status capsule.
private struct SessionRow: View {
    let session: DreamSession

    var body: some View {
        HStack(spacing: 10) {
            Circle()
                .fill(statusColor)
                .frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(session.name)
                        .font(.headline)
                        .lineLimit(1)
                    if session.attention {
                        Circle()
                            .fill(.orange)
                            .frame(width: 8, height: 8)
                            .accessibilityLabel("Needs attention")
                            .accessibilityIdentifier("session.attention.\(session.id)")
                    }
                }
                Text(session.cmd)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(session.createdAt, style: .relative)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            Spacer()
            Text(statusLabel)
                .font(.caption2.weight(.semibold))
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(statusColor.opacity(0.15))
                .clipShape(Capsule())
        }
        .padding(.vertical, 2)
    }

    /// `resurrected` wins over a plain `Live` — dashboard.js parity.
    private var statusLabel: String {
        session.resurrected ? "resurrected" : session.status.rawValue.lowercased()
    }

    private var statusColor: Color {
        if session.resurrected { return .purple }
        switch session.status {
        case .live: return .green
        case .dead: return .gray
        case .resurrected: return .purple
        }
    }
}
