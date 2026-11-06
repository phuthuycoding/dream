import SwiftUI

/// Create-session sheet — name/cmd/cwd, all optional. Blank fields are
/// sent as absent keys so the daemon applies its defaults (cmd defaults
/// server-side to the user's shell).
struct SessionFormView: View {
    let submit: (_ name: String?, _ cmd: String?, _ cwd: String?) async throws -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var cmd = ""
    @State private var cwd = ""
    @State private var saving = false
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name (optional)", text: $name)
                        .accessibilityIdentifier("sessionform.name")
                    TextField("Command (optional — default shell)", text: $cmd)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("sessionform.cmd")
                    TextField("Working directory (optional)", text: $cwd)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("sessionform.cwd")
                }
                if let errorMessage {
                    Section {
                        Text(errorMessage)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("sessionform.error")
                    }
                }
            }
            .navigationTitle("New session")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create") {
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

    private func create() async {
        saving = true
        defer { saving = false }
        do {
            try await submit(nonEmpty(name), nonEmpty(cmd), nonEmpty(cwd))
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
