import SwiftUI

/// Create/edit session sheet — name/cmd/cwd/setup_script, all optional.
/// Blank fields are sent as absent keys so the daemon applies its
/// defaults (cmd defaults server-side to the user's shell). With
/// `editing != nil` the same form becomes the edit screen: fields are
/// prefilled and the `cmd` row is hidden — PATCH never sends a command
/// (changing it implies a respawn, dashboard.js parity).
struct SessionFormView: View {
    let submit: (_ name: String?, _ cmd: String?, _ cwd: String?, _ setupScript: String?) async throws -> Void
    /// Non-nil → edit mode: prefilled, no cmd field.
    var editing: DreamSession? = nil

    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var cmd = ""
    @State private var cwd = ""
    @State private var setupScript = ""
    @State private var saving = false
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name (optional)", text: $name)
                        .accessibilityIdentifier("sessionform.name")
                    if editing == nil {
                        TextField("Command (optional — default shell)", text: $cmd)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .accessibilityIdentifier("sessionform.cmd")
                    }
                    TextField("Working directory (optional)", text: $cwd)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("sessionform.cwd")
                    TextField("Setup script (optional)", text: $setupScript, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("sessionform.setup")
                }
                if let errorMessage {
                    Section {
                        Text(errorMessage)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("sessionform.error")
                    }
                }
            }
            .navigationTitle(editing == nil ? "New session" : "Edit session")
            .onAppear(perform: prefill)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(editing == nil ? "Create" : "Save") {
                        Task { await create() }
                    }
                    .disabled(saving)
                    .accessibilityIdentifier("sessionform.submit")
                }
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
    }

    private func prefill() {
        guard let editing else { return }
        name = editing.name
        cwd = editing.cwd
        setupScript = editing.setupScript ?? ""
    }

    private func create() async {
        saving = true
        defer { saving = false }
        do {
            try await submit(nonEmpty(name), nonEmpty(cmd), nonEmpty(cwd), nonEmpty(setupScript))
            dismiss()
        } catch {
            errorMessage = APIError.describe(error)
        }
    }

    private func nonEmpty(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
