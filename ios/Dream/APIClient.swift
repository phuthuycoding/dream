import Foundation

/// Failures surfaced by `APIClient`. The daemon's contract maps HTTP
/// statuses onto the app's pairing states: 401 means re-pair, 404 means
/// the session is gone, and a transport error means the daemon is
/// unreachable.
enum APIError: Error {
    case unauthorized
    case notFound
    case unreachable(URLError)
    case unexpectedStatus(Int)
    /// The response was not an HTTP response at all.
    case invalidResponse
}

/// REST client for the daemon's session API. Every request carries
/// `Authorization: Bearer` from the injected token provider; base URL
/// and `URLSession` are injectable so tests can stub the transport.
final class APIClient {
    private let baseURL: URL
    private let tokenProvider: () -> String?
    private let urlSession: URLSession

    init(
        baseURL: URL,
        tokenProvider: @escaping () -> String?,
        urlSession: URLSession = .shared
    ) {
        self.baseURL = baseURL
        self.tokenProvider = tokenProvider
        self.urlSession = urlSession
    }

    // MARK: - Sessions

    func listSessions() async throws -> [DreamSession] {
        try await send("GET", url: sessionsURL())
    }

    func getSession(id: String) async throws -> DreamSession {
        try await send("GET", url: sessionURL(id))
    }

    /// `POST /api/sessions` — an empty body spawns a default shell.
    func createSession(
        name: String? = nil,
        cmd: String? = nil,
        cwd: String? = nil,
        setupScript: String? = nil
    ) async throws -> DreamSession {
        let body = CreateSessionRequest(name: name, cmd: cmd, cwd: cwd, setupScript: setupScript)
        return try await send("POST", url: sessionsURL(), body: body)
    }

    /// `PATCH /api/sessions/{id}` — only the provided fields change.
    func updateSession(
        id: String,
        name: String? = nil,
        cwd: String? = nil,
        setupScript: String? = nil
    ) async throws -> DreamSession {
        let body = PatchSessionRequest(name: name, cwd: cwd, setupScript: setupScript)
        return try await send("PATCH", url: sessionURL(id), body: body)
    }

    func deleteSession(id: String) async throws {
        try await send("DELETE", url: sessionURL(id)) as Void
    }

    /// `POST …/kill` — terminates the shell, session persists as `Dead`.
    func killSession(id: String) async throws -> DreamSession {
        try await send("POST", url: sessionURL(id, action: "kill"))
    }

    /// `POST …/respawn` — fresh shell, setup script re-runs.
    func respawnSession(id: String) async throws -> DreamSession {
        try await send("POST", url: sessionURL(id, action: "respawn"))
    }

    /// `POST …/clean` — resets this session's replay log.
    func cleanSession(id: String) async throws {
        try await send("POST", url: sessionURL(id, action: "clean")) as Void
    }

    // MARK: - Plumbing

    private func sessionsURL() -> URL {
        baseURL.appendingPathComponent("api").appendingPathComponent("sessions")
    }

    private func sessionURL(_ id: String, action: String? = nil) -> URL {
        var url = sessionsURL().appendingPathComponent(id)
        if let action {
            url.appendPathComponent(action)
        }
        return url
    }

    private func send(
        _ method: String,
        url: URL
    ) async throws -> Void {
        _ = try await perform(request(method: method, url: url))
    }

    private func send<T: Decodable>(
        _ method: String,
        url: URL
    ) async throws -> T {
        let data = try await perform(request(method: method, url: url))
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func send<T: Decodable, B: Encodable>(
        _ method: String,
        url: URL,
        body: B
    ) async throws -> T {
        var request = request(method: method, url: url)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(body)
        let data = try await perform(request)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func request(method: String, url: URL) -> URLRequest {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let token = tokenProvider() {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return request
    }

    private func perform(_ request: URLRequest) async throws -> Data {
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await urlSession.data(for: request)
        } catch let error as URLError {
            throw APIError.unreachable(error)
        }
        guard let http = response as? HTTPURLResponse else {
            throw APIError.invalidResponse
        }
        switch http.statusCode {
        case 200...299:
            return data
        case 401:
            throw APIError.unauthorized
        case 404:
            throw APIError.notFound
        default:
            throw APIError.unexpectedStatus(http.statusCode)
        }
    }
}

/// `POST /api/sessions` body — every field optional; an all-nil body
/// means "default session".
private struct CreateSessionRequest: Encodable {
    var name: String?
    var cmd: String?
    var cwd: String?
    var setupScript: String?

    private enum CodingKeys: String, CodingKey {
        case name, cmd, cwd
        case setupScript = "setup_script"
    }
}

/// `PATCH /api/sessions/{id}` body — absent keys leave fields untouched.
private struct PatchSessionRequest: Encodable {
    var name: String?
    var cwd: String?
    var setupScript: String?

    private enum CodingKeys: String, CodingKey {
        case name, cwd
        case setupScript = "setup_script"
    }
}
