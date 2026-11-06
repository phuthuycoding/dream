import Foundation

/// A remembered daemon pairing. The bearer token is deliberately not a
/// field — it lives in the Keychain keyed by `id`, so serializing this
/// model can never leak a credential.
struct DaemonProfile: Codable, Equatable, Identifiable, Sendable {
    var id: UUID
    var name: String
    var baseURL: URL
}
