import Foundation
import OSLog
import TokenStationCore

/// The bundled `token-station` daemon, run as a child of the app.
///
/// Its standard input is a pipe the app holds open and never writes to, so the
/// helper sees end-of-file the moment the app goes away — with
/// `--exit-on-stdin-close` that is what stops a daemon outliving the menu bar
/// icon that started it. Everything it prints goes to the unified log rather
/// than to a file the user would have to find.
final class HelperProcess: @unchecked Sendable {
    /// `EX_TEMPFAIL`: another daemon already holds the lock.
    static let alreadyRunningStatus: Int32 = 75

    private let process = Process()
    private let stdin = Pipe()
    private let stdout = Pipe()
    private let stderr = Pipe()
    private let logger: Logger
    private let lock = NSLock()
    private var lastProblem: String?
    /// Partial lines held back until their newline arrives, one buffer per pipe.
    private var partial: [ObjectIdentifier: String] = [:]

    init(executable: URL, socketPath: String?, logger: Logger) {
        self.logger = logger
        process.executableURL = executable
        process.arguments = ["daemon", "--exit-on-stdin-close"]
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = stderr

        var environment = ProcessInfo.processInfo.environment
        // The daemon's own logs go through os.Logger, which has no use for the
        // escape codes a terminal would want.
        environment["NO_COLOR"] = "1"
        if let socketPath { environment["TOKEN_STATION_SOCKET"] = socketPath }
        process.environment = environment
    }

    func run() throws {
        forward(stdout, stream: .out)
        forward(stderr, stream: .err)
        // A handler left on a pipe whose child is gone spins a core: the read
        // end stays readable at end-of-file for ever.
        process.terminationHandler = { [weak self] _ in self?.stopReading() }
        try process.run()
    }

    /// Which of the child's two streams a line came from.
    private enum Stream: String {
        case out
        case err
    }

    private func forward(_ pipe: Pipe, stream: Stream) {
        let key = ObjectIdentifier(pipe)
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard let self else { return }
            guard !data.isEmpty else {
                // End of file. Nothing more is coming and the handler would
                // otherwise be called again immediately, for ever.
                handle.readabilityHandler = nil
                self.flushPartial(key: key, stream: stream)
                return
            }
            guard let text = String(data: data, encoding: .utf8) else { return }
            self.consume(text, key: key, stream: stream)
        }
    }

    /// Logs whole lines only; anything after the last newline waits for the rest.
    private func consume(_ text: String, key: ObjectIdentifier, stream: Stream) {
        lock.lock()
        let combined = (partial[key] ?? "") + text
        var lines = combined.components(separatedBy: "\n")
        partial[key] = lines.popLast() ?? ""
        lock.unlock()

        for line in lines where !line.isEmpty {
            log(line, stream: stream)
        }
    }

    /// Logs whatever the child left without a trailing newline.
    private func flushPartial(key: ObjectIdentifier, stream: Stream) {
        lock.lock()
        let leftover = partial.removeValue(forKey: key) ?? ""
        lock.unlock()
        guard !leftover.isEmpty else { return }
        log(leftover, stream: stream)
    }

    /// Logs a line at the level the helper itself stamped on it.
    ///
    /// The helper puts `INFO` on standard error along with everything else, so
    /// going by the stream alone would file its whole startup as errors.
    private func log(_ line: String, stream: Stream) {
        switch HelperLogLine.severity(of: line) {
        case .debug:
            logger.debug("[\(stream.rawValue, privacy: .public)] \(line, privacy: .public)")
        case .warning:
            logger.warning("[\(stream.rawValue, privacy: .public)] \(line, privacy: .public)")
        case .error:
            logger.error("[\(stream.rawValue, privacy: .public)] \(line, privacy: .public)")
        case .info:
            // `.notice` is the lowest level the unified log keeps on disk, and a
            // helper's output is worth having after the fact.
            logger.notice("[\(stream.rawValue, privacy: .public)] \(line, privacy: .public)")
        case nil:
            // Nothing stamped a level on it: a panic, a usage error, a dynamic
            // linker failure. On standard error all of those are problems.
            if stream == .err {
                logger.error("[\(stream.rawValue, privacy: .public)] \(line, privacy: .public)")
            } else {
                logger.notice("[\(stream.rawValue, privacy: .public)] \(line, privacy: .public)")
            }
        }
        remember(line, stream: stream)
    }

    private func remember(_ line: String, stream: Stream) {
        guard HelperLogLine.isDiagnostic(line, onStandardError: stream == .err) else { return }
        lock.lock()
        defer { lock.unlock() }
        lastProblem = line
    }

    /// The last line the helper complained with, which is what a failure is
    /// explained with. `nil` when it only ever said routine things, so the
    /// caller can fall back to the exit status rather than quote a line that has
    /// nothing to do with why it stopped.
    var lastProblemLine: String? {
        lock.lock()
        defer { lock.unlock() }
        return lastProblem
    }

    var isRunning: Bool { process.isRunning }

    /// The child's exit status, once it has one.
    var terminationStatus: Int32? { process.isRunning ? nil : process.terminationStatus }

    func terminate() {
        stopReading()
        // Closing standard input is the polite stop; the signal is the fallback.
        try? stdin.fileHandleForWriting.close()
        if process.isRunning { process.terminate() }
    }

    /// Detaches both handlers, after logging whatever was left mid-line.
    private func stopReading() {
        for (pipe, stream) in [(stdout, Stream.out), (stderr, Stream.err)] {
            pipe.fileHandleForReading.readabilityHandler = nil
            flushPartial(key: ObjectIdentifier(pipe), stream: stream)
        }
    }
}
