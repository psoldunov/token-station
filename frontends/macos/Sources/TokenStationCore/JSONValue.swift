/// Any JSON document, kept whole.
///
/// The settings page edits a handful of keys, but `SetSettings` replaces the
/// entire config. Holding the document as a generic value and patching single
/// paths means a key this build has never heard of — added by a newer daemon, or
/// hand-written into `config.toml` — survives a round trip instead of being
/// silently reset to its default.
public enum JSONValue: Codable, Hashable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([Self])
    case object([String: Self])

    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([Self].self) {
            self = .array(value)
        } else if let value = try? container.decode([String: Self].self) {
            self = .object(value)
        } else {
            throw DecodingError.dataCorruptedError(
                in: container, debugDescription: "unsupported JSON value")
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}

// MARK: - Reading

extension JSONValue {
    // A JSON document either holds a boolean at this path or holds nothing
    // there, so the absent case is the data, not a third state in an API.
    // swiftlint:disable:next discouraged_optional_boolean
    public var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }

    public var doubleValue: Double? {
        if case .number(let value) = self { return value }
        return nil
    }

    public var intValue: Int? {
        guard case .number(let value) = self, value.isFinite else { return nil }
        return Int(value.rounded())
    }

    public var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    public var objectValue: [String: JSONValue]? {
        if case .object(let value) = self { return value }
        return nil
    }

    /// Reads a dotted path, e.g. `alerts.warning_percent`.
    public subscript(path path: String) -> JSONValue? {
        var current = self
        for key in path.split(separator: ".") {
            guard let object = current.objectValue, let next = object[String(key)] else {
                return nil
            }
            current = next
        }
        return current
    }

    /// A copy with one dotted path replaced; missing intermediate objects are created.
    public func setting(path: String, to value: JSONValue) -> JSONValue {
        let keys = path.split(separator: ".").map(String.init)
        guard !keys.isEmpty else { return value }
        return Self.setting(self, keys: keys[...], to: value)
    }

    private static func setting(
        _ node: JSONValue,
        keys: ArraySlice<String>,
        to value: JSONValue
    ) -> JSONValue {
        guard let key = keys.first else { return value }
        var object = node.objectValue ?? [:]
        let child = object[key] ?? .object([:])
        object[key] = setting(child, keys: keys.dropFirst(), to: value)
        return .object(object)
    }
}
