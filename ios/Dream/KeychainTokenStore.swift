import Foundation
import Security

/// Wraps a `Security` framework failure so callers see the OSStatus.
struct KeychainError: Error, Equatable {
    let status: OSStatus
}

/// Bearer tokens stored as generic-password Keychain items, one per
/// profile id. Only this type touches `Security` for credentials — the
/// token never transits UserDefaults or the profile file.
struct KeychainTokenStore: Sendable {
    private let service = "com.talabs.dream.token"

    private func query(for account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    func token(for account: String) -> String? {
        var lookup = query(for: account)
        lookup[kSecReturnData as String] = true
        lookup[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(lookup as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else {
            return nil
        }
        return String(data: data, encoding: .utf8)
    }

    func setToken(_ token: String, for account: String) throws {
        let data = Data(token.utf8)
        var attributes = query(for: account)
        attributes[kSecValueData as String] = data
        let status = SecItemAdd(attributes as CFDictionary, nil)
        switch status {
        case errSecSuccess:
            return
        case errSecDuplicateItem:
            let update = SecItemUpdate(
                query(for: account) as CFDictionary,
                [kSecValueData as String: data] as CFDictionary
            )
            guard update == errSecSuccess else {
                throw KeychainError(status: update)
            }
        default:
            throw KeychainError(status: status)
        }
    }

    func deleteToken(for account: String) throws {
        let status = SecItemDelete(query(for: account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainError(status: status)
        }
    }
}
