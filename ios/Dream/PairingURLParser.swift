import Foundation

/// Errors raised while reducing a pasted `dream url` line to a base URL
/// and bearer token. Every failure mode is typed so the pairing flow can
/// tell "no URL here" apart from "almost right".
enum PairingURLError: Error, Equatable {
    /// The input contains no `http(s)://…` candidate at all.
    case noURLFound
    /// The candidate is not a well-formed http(s) URL with a host.
    case invalidURL
    /// An explicit port is present but is not a number in 1…65535.
    case invalidPort
    /// The `token` query item is absent or empty.
    case missingToken
}

/// Extracts the daemon's pairing URL — `http(s)://host:port/?token=…` —
/// out of a pasted `dream url` output line, validates it, and hands back
/// the base URL (query stripped) plus the bearer token.
enum PairingURLParser {
    /// The first whitespace-delimited `http(s)://` run in the input. The
    /// daemon prints the URL inside a chatty line ("dream ready — open:
    /// <url>"), so the token must be found inside surrounding text.
    private static let urlPattern = try! NSRegularExpression(
        pattern: #"https?://[^\s<>"']+"#
    )

    /// Delimiters that may legitimately follow a URL inside prose but can
    /// never be part of the pairing URL itself.
    private static let trailingTrim = CharacterSet(charactersIn: ".,;:!?)]}>")

    static func parse(_ input: String) throws -> (baseURL: URL, token: String) {
        guard let candidate = urlCandidate(in: input) else {
            throw PairingURLError.noURLFound
        }
        guard
            let components = URLComponents(string: candidate),
            let scheme = components.scheme?.lowercased(),
            scheme == "http" || scheme == "https",
            let host = components.host, !host.isEmpty
        else {
            throw PairingURLError.invalidURL
        }
        try validateExplicitPort(in: candidate)
        guard
            let token = components.queryItems?
                .first(where: { $0.name == "token" })?.value,
            !token.isEmpty
        else {
            throw PairingURLError.missingToken
        }

        var base = components
        base.query = nil
        base.fragment = nil
        guard let baseURL = base.url else {
            throw PairingURLError.invalidURL
        }
        return (baseURL, token)
    }

    private static func urlCandidate(in input: String) -> String? {
        let range = NSRange(input.startIndex..<input.endIndex, in: input)
        guard let match = urlPattern.firstMatch(in: input, range: range),
              let swiftRange = Range(match.range, in: input)
        else {
            return nil
        }
        let candidate = String(input[swiftRange])
            .trimmingCharacters(in: trailingTrim)
        return candidate.isEmpty ? nil : candidate
    }

    /// `URLComponents` quietly drops an out-of-range or non-numeric port,
    /// which would turn `http://host:abc/` into a silently different
    /// endpoint. Inspect the authority section explicitly so a malformed
    /// port is a hard failure instead.
    private static func validateExplicitPort(in candidate: String) throws {
        guard let schemeEnd = candidate.range(of: "://") else {
            throw PairingURLError.invalidURL
        }
        let afterScheme = candidate[schemeEnd.upperBound...]
        let authority = afterScheme.prefix(while: { $0 != "/" && $0 != "?" && $0 != "#" })
        let hostPort = authority.split(separator: "@").last ?? ""
        guard let colon = hostPort.lastIndex(of: ":") else {
            return // no explicit port — daemon default applies
        }
        let portText = hostPort[hostPort.index(after: colon)...]
        guard let port = Int(portText), (1...65_535).contains(port) else {
            throw PairingURLError.invalidPort
        }
    }
}
