/// A string enum that survives a newer daemon.
///
/// The daemon and the app ship together, but a user can update one without the
/// other. A snapshot carrying a state, level or window kind this build has never
/// heard of decodes into `unknown(raw)` instead of failing the whole snapshot,
/// so one new provider state cannot blank the panel.
protocol ForwardCompatibleEnum: RawRepresentable, Codable, Hashable, Sendable
where RawValue == String {
    /// The case that carries a value this build does not know.
    static func unknown(_ raw: String) -> Self
}

extension ForwardCompatibleEnum {
    // Public because the enums that adopt this are public, and a witness cannot
    // be less visible than the conformance it satisfies.
    public init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? Self.unknown(raw)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}
