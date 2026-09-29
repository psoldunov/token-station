import Foundation

/// The JSON-RPC 2.0 messages the daemon socket carries.
enum JSONRPC {
    struct Request: Encodable {
        // Written to the wire and never read back, which is what the protocol
        // asks for: every request carries the version it speaks.
        // swiftlint:disable:next unused_declaration
        let jsonrpc = "2.0"
        let id: Int?
        let method: String
        let params: JSONValue?
    }

    /// One decoded line: a response, a notification, or a parse error report.
    struct Message: Decodable {
        var id: Int?
        var method: String?
        var params: JSONValue?
        var result: JSONValue?
        var error: DaemonError?
    }
}

/// A JSON-RPC error object as the daemon sends it.
public struct DaemonError: Error, Decodable, Hashable, Sendable, LocalizedError {
    public var code: Int
    public var message: String
    public var data: JSONValue?

    public var errorDescription: String? { message }

    /// `error.data.problems` for a `SetSettings` the daemon refused.
    public var problems: [String] {
        guard case .array(let rows)? = data?[path: "problems"] else { return [] }
        return rows.compactMap(\.stringValue)
    }

    /// Invalid params: the settings document was out of bounds or unwritable.
    public var isInvalidParams: Bool { code == -32602 }
}

/// Something went wrong with the connection itself rather than with a request.
public enum DaemonTransportError: Error, LocalizedError, Sendable {
    case cannotOpenSocket(String, errno: Int32)
    case socketPathTooLong(String)
    case connectionClosed
    case malformedMessage(String)

    public var errorDescription: String? {
        switch self {
        case .cannotOpenSocket(let path, let code):
            "Cannot reach the Token Station service at \(path): \(String(cString: strerror(code)))."
        case .socketPathTooLong(let path):
            "The socket path is too long for a Unix domain socket: \(path)."
        case .connectionClosed:
            "The Token Station service closed the connection."
        case .malformedMessage(let detail):
            "The Token Station service sent something unreadable: \(detail)."
        }
    }
}

/// A push from the daemon after `Subscribe`.
public enum DaemonNotification: Sendable {
    case snapshotChanged(revision: UInt64, snapshot: Snapshot)
    case alert(Alert)
    case closed(reason: String?)
}

/// One `Alert` notification.
public struct Alert: Codable, Hashable, Sendable {
    public var provider: ProviderID
    public var providerName: String
    public var windowId: String
    public var windowLabel: String
    public var kind: AlertKind
    public var percent: Double
    public var resetsAt: Int64?
    /// The daemon's own English text, for clients that do not word it themselves.
    public var summary: String
    public var body: String

    public init(
        provider: ProviderID,
        providerName: String,
        windowId: String,
        windowLabel: String,
        kind: AlertKind,
        percent: Double,
        resetsAt: Int64? = nil,
        summary: String = "",
        body: String = ""
    ) {
        self.provider = provider
        self.providerName = providerName
        self.windowId = windowId
        self.windowLabel = windowLabel
        self.kind = kind
        self.percent = percent
        self.resetsAt = resetsAt
        self.summary = summary
        self.body = body
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        provider = try container.decode(ProviderID.self, forKey: .provider)
        providerName = try container.decodeIfPresent(String.self, forKey: .providerName) ?? ""
        windowId = try container.decodeIfPresent(String.self, forKey: .windowId) ?? ""
        windowLabel = try container.decodeIfPresent(String.self, forKey: .windowLabel) ?? ""
        kind = try container.decode(AlertKind.self, forKey: .kind)
        percent = try container.decodeIfPresent(Double.self, forKey: .percent) ?? 0
        resetsAt = try container.decodeIfPresent(Int64.self, forKey: .resetsAt)
        summary = try container.decodeIfPresent(String.self, forKey: .summary) ?? ""
        body = try container.decodeIfPresent(String.self, forKey: .body) ?? ""
    }
}

/// `GetVersion`.
public struct DaemonVersion: Decodable, Hashable, Sendable {
    public var version: String
    public var protocolVersion: Int

    private enum CodingKeys: String, CodingKey {
        case version
        case protocolVersion = "protocol"
    }
}

/// `GetSnapshot` and `Subscribe`.
public struct SnapshotEnvelope: Decodable, Sendable {
    public var revision: UInt64
    public var snapshot: Snapshot
}

/// One point of `GetHistory`.
public struct HistoryPoint: Hashable, Sendable {
    /// Unix seconds.
    public var timestamp: Int64
    /// 0–100.
    public var percent: Double

    public init(timestamp: Int64, percent: Double) {
        self.timestamp = timestamp
        self.percent = percent
    }

    public var date: Date { Date(timeIntervalSince1970: TimeInterval(timestamp)) }
}
