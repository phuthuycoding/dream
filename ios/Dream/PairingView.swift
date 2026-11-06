import SwiftUI

/// Pairing screen — paste the `dream url` output line (URL and token
/// are extracted automatically) or fill in host/port/token manually.
/// Nothing is persisted until `GET /api/sessions` answers with the
/// entered credentials. With `existing != nil` the same form re-pairs
/// that profile: fields are prefilled and saving writes a fresh token
/// onto the existing profile id.
struct PairingView: View {
    let existing: DaemonProfile?

    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss

    @State private var pasted = ""
    @State private var host = ""
    @State private var port = ""
    @State private var token = ""
    @State private var name = ""
    @State private var errorMessage: String?
    @State private var saving = false
    @State private var showProfiles = false

    init(existing: DaemonProfile? = nil) {
        self.existing = existing
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Paste `dream url` output", text: $pasted, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("pairing.paste")
                } header: {
                    Text("Pairing URL")
                } footer: {
                    Text("Paste the line `dream url` prints on the daemon host.")
                }

                Section("Or enter manually") {
                    TextField("Host (e.g. 192.168.1.5)", text: $host)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .accessibilityIdentifier("pairing.host")
                    TextField("Port", text: $port)
                        .keyboardType(.numberPad)
                        .accessibilityIdentifier("pairing.port")
                    SecureField("Token", text: $token)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("pairing.token")
                    TextField("Profile name", text: $name)
                        .accessibilityIdentifier("pairing.name")
                }

                if let errorMessage {
                    Section {
                        Text(errorMessage)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("pairing.error")
                    }
                }

                Section {
                    Button {
                        Task { await save() }
                    } label: {
                        if saving {
                            ProgressView()
                        } else {
                            Text(existing == nil ? "Pair and save" : "Save new credentials")
                        }
                    }
                    .disabled(saving)
                    .accessibilityIdentifier("pairing.save")
                }

                // A daemon that keeps rejecting tokens must not trap the
                // user — the switcher lets them pick another profile or
                // delete this one.
                if existing != nil {
                    Section {
                        Button("Use a different daemon") { showProfiles = true }
                            .accessibilityIdentifier("pairing.switch")
                    }
                }
            }
            .navigationTitle(existing.map { "Re-pair \($0.name)" } ?? "Pair a daemon")
            .onAppear(perform: prefill)
            .sheet(isPresented: $showProfiles) {
                ProfileListView()
            }
        }
    }

    private func prefill() {
        guard let existing, host.isEmpty else { return }
        host = existing.baseURL.host ?? ""
        port = existing.baseURL.port.map(String.init) ?? ""
        name = existing.name
    }

    private func save() async {
        errorMessage = nil
        let credentials: (baseURL: URL, token: String)
        do {
            credentials = try resolveCredentials()
        } catch let error as PairingURLError {
            errorMessage = Self.describe(error)
            return
        } catch {
            errorMessage = error.localizedDescription
            return
        }

        saving = true
        defer { saving = false }

        // Validation per the brief: the token must answer GET
        // /api/sessions before the profile is persisted.
        let probe = APIClient(baseURL: credentials.baseURL) { credentials.token }
        do {
            _ = try await probe.listSessions()
        } catch APIError.unreachable(let error) {
            errorMessage = Self.transportHint(baseURL: credentials.baseURL)
                ?? APIError.describe(error)
            return
        } catch {
            errorMessage = APIError.describe(error)
            return
        }

        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let profile = DaemonProfile(
            id: existing?.id ?? UUID(),
            name: trimmedName.isEmpty ? Self.defaultName(for: credentials.baseURL) : trimmedName,
            baseURL: credentials.baseURL
        )
        do {
            try appState.completePairing(profile: profile, token: credentials.token)
            dismiss()
        } catch {
            errorMessage = "Couldn't save the profile: \(error.localizedDescription)"
        }
    }

    /// A pasted line wins over the manual fields — it carries both the
    /// URL and the token in one string.
    private func resolveCredentials() throws -> (baseURL: URL, token: String) {
        let pastedText = pasted.trimmingCharacters(in: .whitespacesAndNewlines)
        if !pastedText.isEmpty {
            return try PairingURLParser.parse(pastedText)
        }

        let hostText = host.trimmingCharacters(in: .whitespaces)
        let tokenText = token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !hostText.isEmpty else { throw PairingURLError.noURLFound }
        guard !hostText.contains(where: { $0.isWhitespace }) else {
            throw PairingURLError.invalidURL
        }

        // On re-pair, seed components from the stored baseURL so its
        // scheme and path survive — rebuilding as http://host:port would
        // silently downgrade https pairings and drop path prefixes.
        var components = existing.flatMap {
            URLComponents(url: $0.baseURL, resolvingAgainstBaseURL: false)
        } ?? URLComponents()
        if components.scheme == nil {
            components.scheme = "http"
        }
        components.host = hostText
        let portText = port.trimmingCharacters(in: .whitespaces)
        if portText.isEmpty {
            components.port = nil
        } else {
            guard let value = Int(portText), (1...65_535).contains(value) else {
                throw PairingURLError.invalidPort
            }
            components.port = value
        }
        guard let url = components.url else {
            throw PairingURLError.invalidURL
        }
        guard !tokenText.isEmpty else { throw PairingURLError.missingToken }
        return (url, tokenText)
    }

    /// ATS exempts IP literals but not hostnames: a plain-http pairing
    /// to `dreambox.home` or a tailnet name fails at the transport layer
    /// with an opaque "cannot load resources" — name the real cause.
    private static func transportHint(baseURL: URL) -> String? {
        guard baseURL.scheme == "http",
              let host = baseURL.host,
              !isIPLiteral(host)
        else { return nil }
        return """
        iOS requires https for named hosts — use the daemon's IP address \
        or serve it over https (e.g. `tailscale serve`).
        """
    }

    /// IPv4 dotted-quad or anything containing ':' (IPv6 literal).
    private static func isIPLiteral(_ host: String) -> Bool {
        if host.contains(":") { return true }
        let octets = host.split(separator: ".")
        return octets.count == 4
            && octets.allSatisfy { $0.count <= 3 && UInt8($0) != nil }
    }

    private static func describe(_ error: PairingURLError) -> String {
        switch error {
        case .noURLFound:
            return "Paste the `dream url` output above or enter host, port, and token below."
        case .invalidURL:
            return "That isn't a valid http(s) URL — check the host."
        case .invalidPort:
            return "The port must be a number between 1 and 65535."
        case .missingToken:
            return "The token is missing — the pairing URL carries it as `?token=`."
        }
    }

    private static func defaultName(for url: URL) -> String {
        if let port = url.port {
            return "\(url.host ?? "daemon"):\(port)"
        }
        return url.host ?? "daemon"
    }
}
