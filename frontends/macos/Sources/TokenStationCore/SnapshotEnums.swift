/// A usage provider (one CLI).
public enum ProviderID: ForwardCompatibleEnum {
    case claude
    case codex
    case unknown(String)

    public init?(rawValue: String) {
        switch rawValue {
        case "claude": self = .claude
        case "codex": self = .codex
        default: return nil
        }
    }

    public var rawValue: String {
        switch self {
        case .claude: "claude"
        case .codex: "codex"
        case .unknown(let raw): raw
        }
    }
}

/// Severity derived from the configured warning/critical thresholds.
public enum Level: ForwardCompatibleEnum {
    case normal
    case warning
    case critical
    case unknown(String)

    public init?(rawValue: String) {
        switch rawValue {
        case "normal": self = .normal
        case "warning": self = .warning
        case "critical": self = .critical
        default: return nil
        }
    }

    public var rawValue: String {
        switch self {
        case .normal: "normal"
        case .warning: "warning"
        case .critical: "critical"
        case .unknown(let raw): raw
        }
    }
}

/// Health of one provider's data.
public enum ProviderState: ForwardCompatibleEnum {
    case loading
    case ok
    case stale
    case unauthenticated
    case notInstalled
    case disabled
    case error
    case unknown(String)

    public init?(rawValue: String) {
        switch rawValue {
        case "loading": self = .loading
        case "ok": self = .ok
        case "stale": self = .stale
        case "unauthenticated": self = .unauthenticated
        case "not_installed": self = .notInstalled
        case "disabled": self = .disabled
        case "error": self = .error
        default: return nil
        }
    }

    public var rawValue: String {
        switch self {
        case .loading: "loading"
        case .ok: "ok"
        case .stale: "stale"
        case .unauthenticated: "unauthenticated"
        case .notInstalled: "not_installed"
        case .disabled: "disabled"
        case .error: "error"
        case .unknown(let raw): raw
        }
    }
}

/// What kind of limit a window represents.
public enum WindowKind: ForwardCompatibleEnum {
    case session
    case weekly
    case model
    case other
    case unknown(String)

    public init?(rawValue: String) {
        switch rawValue {
        case "session": self = .session
        case "weekly": self = .weekly
        case "model": self = .model
        case "other": self = .other
        default: return nil
        }
    }

    public var rawValue: String {
        switch self {
        case .session: "session"
        case .weekly: "weekly"
        case .model: "model"
        case .other: "other"
        case .unknown(let raw): raw
        }
    }
}

/// Why an alert was raised.
public enum AlertKind: ForwardCompatibleEnum {
    case warning
    case critical
    case reset
    case unknown(String)

    public init?(rawValue: String) {
        switch rawValue {
        case "warning": self = .warning
        case "critical": self = .critical
        case "reset": self = .reset
        default: return nil
        }
    }

    public var rawValue: String {
        switch self {
        case .warning: "warning"
        case .critical: "critical"
        case .reset: "reset"
        case .unknown(let raw): raw
        }
    }
}
