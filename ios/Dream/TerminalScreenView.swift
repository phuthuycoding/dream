import SwiftUI

/// Full-screen terminal attach for one session. The overlays mirror the
/// web's: a connecting spinner until the first snapshot, the `e`-frame
/// end overlay with a respawn affordance, and a reconnect overlay when
/// the socket drops without one. Disappearing — back navigation or a
/// profile-switch rebuild — closes only the socket; the session stays
/// alive on the daemon.
struct TerminalScreenView: View {
    @Environment(AttentionCenter.self) private var attentionCenter
    @State private var controller: TerminalController

    init(
        session: DreamSession,
        client: APIClient,
        baseURL: URL,
        tokenProvider: @escaping () -> String?,
        onAttention: @escaping () -> Void,
        onUnauthorized: @escaping () -> Void
    ) {
        _controller = State(
            initialValue: TerminalController(
                session: session,
                client: client,
                baseURL: baseURL,
                tokenProvider: tokenProvider,
                onAttention: onAttention,
                onUnauthorized: onUnauthorized
            )
        )
    }

    var body: some View {
        TerminalView(controller: controller)
            .background(.black)
            .navigationTitle(controller.terminalTitle.isEmpty ? controller.session.name : controller.terminalTitle)
            .navigationBarTitleDisplayMode(.inline)
            .overlay { overlay }
            .onAppear {
                // While this terminal is up its own attention edge is
                // suppressed — the attach already acks the flag.
                attentionCenter.foregroundedSessionID = controller.session.id
                controller.attach()
            }
            .onDisappear {
                if attentionCenter.foregroundedSessionID == controller.session.id {
                    attentionCenter.foregroundedSessionID = nil
                }
                controller.detach()
            }
    }

    @ViewBuilder
    private var overlay: some View {
        switch controller.phase {
        case .connecting:
            ProgressView("Attaching…")
                .padding()
                .background(.regularMaterial)
                .clipShape(RoundedRectangle(cornerRadius: 12))
        case .ended(let notice):
            EndOverlay(
                title: "Session ended",
                detail: notice,
                actionTitle: "Respawn",
                actionIdentifier: "terminal.respawn",
                action: controller.respawn
            )
        case .disconnected(let reason):
            EndOverlay(
                title: "Disconnected",
                detail: reason,
                actionTitle: "Reconnect",
                actionIdentifier: "terminal.reconnect",
                action: controller.attach
            )
        case .live:
            EmptyView()
        }
    }
}

/// The end-of-attach overlay: notice text plus one action button —
/// respawn for a dead session, reconnect for a dropped socket.
private struct EndOverlay: View {
    let title: String
    let detail: String
    let actionTitle: String
    let actionIdentifier: String
    let action: () -> Void

    var body: some View {
        ZStack {
            Color.black.opacity(0.55)
                .ignoresSafeArea()
            VStack(spacing: 12) {
                Text(title)
                    .font(.headline)
                Text(detail)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                Button(actionTitle, action: action)
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier(actionIdentifier)
            }
            .padding(24)
            .background(.regularMaterial)
            .clipShape(RoundedRectangle(cornerRadius: 12))
            .padding(32)
        }
        .accessibilityIdentifier("terminal.overlay.end")
    }
}
