import Foundation

/// Splits a byte stream into the newline-terminated JSON lines the socket carries.
///
/// The protocol caps a line at 1 MiB; a longer one is a protocol error and the
/// daemon closes the connection, so the framer refuses to buffer past the cap
/// rather than growing without bound on a peer that never sends a newline.
struct LineFramer: Sendable {
    /// Largest line the protocol allows, in bytes, newline excluded.
    static let maximumLineLength = 1 << 20

    struct LineTooLongError: Error, CustomStringConvertible, Sendable {
        let length: Int
        var description: String {
            "the daemon sent a line of \(length) bytes, over the \(LineFramer.maximumLineLength) byte limit"
        }
    }

    private var buffer = Data()

    init() {}

    /// Appends bytes and returns every complete line they finished.
    mutating func append(_ bytes: Data) throws -> [Data] {
        buffer.append(bytes)
        var lines: [Data] = []
        while let newline = buffer.firstIndex(of: UInt8(ascii: "\n")) {
            let line = buffer[buffer.startIndex..<newline]
            if line.count > Self.maximumLineLength {
                buffer.removeAll(keepingCapacity: false)
                throw LineTooLongError(length: line.count)
            }
            lines.append(Data(line))
            buffer.removeSubrange(buffer.startIndex...newline)
        }
        if buffer.count > Self.maximumLineLength {
            let length = buffer.count
            buffer.removeAll(keepingCapacity: false)
            throw LineTooLongError(length: length)
        }
        return lines
    }

    /// Bytes held back because no newline has arrived yet.
    var pendingByteCount: Int { buffer.count }
}
