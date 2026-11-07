import SwiftUI

/// Profile switcher — pick which daemon the app talks to, rename
/// entries, delete them (Keychain cleanup happens inside
/// `ProfileStore.delete`), or pair another daemon.
struct ProfileListView: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss

    @State private var renaming: DaemonProfile?
    @State private var renameText = ""
    @State private var deleting: DaemonProfile?
    @State private var pairingNew = false
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            List {
                ForEach(appState.profiles) { profile in
                    Button {
                        appState.setActiveProfile(profile.id)
                        dismiss()
                    } label: {
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(profile.name).font(.headline)
                                Text(profile.baseURL.absoluteString)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            if profile.id == appState.activeProfileId {
                                Image(systemName: "checkmark")
                                    .foregroundStyle(.tint)
                            }
                        }
                    }
                    .accessibilityIdentifier("profile.row.\(profile.id.uuidString)")
                    .swipeActions(edge: .trailing) {
                        Button("Delete", role: .destructive) { deleting = profile }
                        Button("Rename") { beginRename(profile) }
                    }
                    .contextMenu {
                        Button("Rename") { beginRename(profile) }
                        Button("Delete", role: .destructive) { deleting = profile }
                    }
                }
            }
            .navigationTitle("Daemons")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button { pairingNew = true } label: {
                        Image(systemName: "plus")
                    }
                    .accessibilityIdentifier("profiles.add")
                }
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .sheet(isPresented: $pairingNew) {
                PairingView()
            }
            .alert("Rename profile", isPresented: renamePresented, presenting: renaming) { profile in
                TextField("Name", text: $renameText)
                Button("Save") { commitRename(profile) }
                Button("Cancel", role: .cancel) {}
            }
            .alert(deleteTitle, isPresented: deletePresented, presenting: deleting) { profile in
                Button("Delete", role: .destructive) {
                    do {
                        try appState.deleteProfile(profile.id)
                    } catch {
                        errorMessage = error.localizedDescription
                    }
                }
                Button("Cancel", role: .cancel) {}
            } message: { _ in
                Text("Its Keychain token is removed too.")
            }
            .alert("Something went wrong", isPresented: failurePresented) {
                Button("OK") {}
            } message: {
                Text(errorMessage ?? "")
            }
        }
    }

    private func beginRename(_ profile: DaemonProfile) {
        renaming = profile
        renameText = profile.name
    }

    private func commitRename(_ profile: DaemonProfile) {
        let trimmed = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != profile.name else { return }
        do {
            try appState.renameProfile(profile.id, to: trimmed)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private var deleteTitle: String {
        "Delete \"\(deleting?.name ?? "profile")\"?"
    }

    private var renamePresented: Binding<Bool> {
        Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })
    }

    private var deletePresented: Binding<Bool> {
        Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } })
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { errorMessage != nil }, set: { if !$0 { errorMessage = nil } })
    }
}
