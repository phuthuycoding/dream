import Foundation

/// A daemon session exactly as `GET /api/sessions` serializes it: the
/// serde wire names (`created_at`, `log_path`, `setup_script`), the
/// `"Live"/"Dead"/"Resurrected"` status values, and an `attention` flag
/// that older daemons omit entirely — hence the default-false decode.
struct DreamSession: Codable, Equatable, Hashable, Sendable, Identifiable {
    var id: String
    var name: String
    var cmd: String
    var cwd: String
    var setupScript: String?
    var status: Status
    var createdAt: Date
    var logPath: String
    var resurrected: Bool
    var attention: Bool

    enum Status: String, Codable, Sendable {
        case live = "Live"
        case dead = "Dead"
        case resurrected = "Resurrected"
    }

    init(
        id: String,
        name: String,
        cmd: String,
        cwd: String,
        setupScript: String? = nil,
        status: Status,
        createdAt: Date,
        logPath: String,
        resurrected: Bool = false,
        attention: Bool = false
    ) {
        self.id = id
        self.name = name
        self.cmd = cmd
        self.cwd = cwd
        self.setupScript = setupScript
        self.status = status
        self.createdAt = createdAt
        self.logPath = logPath
        self.resurrected = resurrected
        self.attention = attention
    }

    private enum CodingKeys: String, CodingKey {
        case id, name, cmd, cwd, status, resurrected, attention
        case setupScript = "setup_script"
        case createdAt = "created_at"
        case logPath = "log_path"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        name = try container.decode(String.self, forKey: .name)
        cmd = try container.decode(String.self, forKey: .cmd)
        cwd = try container.decode(String.self, forKey: .cwd)
        setupScript = try container.decodeIfPresent(String.self, forKey: .setupScript)
        status = try container.decode(Status.self, forKey: .status)
        logPath = try container.decode(String.self, forKey: .logPath)
        // Serde marks `resurrected` and `attention` `#[serde(default)]` —
        // old payloads omit them and decode as false.
        resurrected = try container.decodeIfPresent(Bool.self, forKey: .resurrected) ?? false
        attention = try container.decodeIfPresent(Bool.self, forKey: .attention) ?? false

        let stamp = try container.decode(String.self, forKey: .createdAt)
        guard let parsed = WireDate.parse(stamp) else {
            throw DecodingError.dataCorruptedError(
                forKey: .createdAt,
                in: container,
                debugDescription: "created_at is not RFC 3339: \(stamp)"
            )
        }
        createdAt = parsed
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(name, forKey: .name)
        try container.encode(cmd, forKey: .cmd)
        try container.encode(cwd, forKey: .cwd)
        try container.encodeIfPresent(setupScript, forKey: .setupScript)
        try container.encode(status, forKey: .status)
        try container.encode(WireDate.format(createdAt), forKey: .createdAt)
        try container.encode(logPath, forKey: .logPath)
        try container.encode(resurrected, forKey: .resurrected)
        try container.encode(attention, forKey: .attention)
    }
}

/// RFC 3339 timestamps with optional fractional seconds — `chrono`
/// emits whole-second stamps (`…T03:04:05Z`) and nanosecond ones
/// (`…T12:00:00.123456Z`) depending on the value.
enum WireDate {
    private static let plain: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter
    }()

    private static let fractional: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    static func parse(_ string: String) -> Date? {
        fractional.date(from: string) ?? plain.date(from: string)
    }

    static func format(_ date: Date) -> String {
        fractional.string(from: date)
    }
}
