import Darwin
import Dispatch
import Foundation

/// A connected Unix-domain stream socket, read and written through `DispatchIO`.
///
/// Every mutable member is touched only on `queue`, which is why the class can
/// be `@unchecked Sendable`: the queue, not the compiler, is the lock.
final class UnixSocketChannel: @unchecked Sendable {
    private let queue = DispatchQueue(label: "dev.soldunov.TokenStation.socket")
    private let dispatchIO: DispatchIO
    private var onBytes: (@Sendable (Data) -> Void)?
    private var onClose: (@Sendable (String?) -> Void)?
    private var closed = false

    /// Opens and connects the socket, blocking for as long as a local connect takes.
    init(path: String) throws {
        let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else {
            throw DaemonTransportError.cannotOpenSocket(path, errno: errno)
        }

        do {
            try Self.connect(descriptor: descriptor, path: path)
        } catch {
            Darwin.close(descriptor)
            throw error
        }

        dispatchIO = DispatchIO(
            type: .stream,
            fileDescriptor: descriptor,
            queue: queue,
            cleanupHandler: { _ in Darwin.close(descriptor) }
        )
        // Hand every byte over as it arrives: the daemon's lines are small and a
        // snapshot that waits for a buffer to fill is a panel that lags.
        dispatchIO.setLimit(lowWater: 1)
    }

    private static func connect(descriptor: Int32, path: String) throws {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)

        let bytes = Array(path.utf8)
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard bytes.count < capacity else {
            throw DaemonTransportError.socketPathTooLong(path)
        }
        withUnsafeMutableBytes(of: &address.sun_path) { destination in
            destination.copyBytes(from: bytes)
            destination[bytes.count] = 0
        }

        let status = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { generic in
                Darwin.connect(descriptor, generic, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard status == 0 else {
            throw DaemonTransportError.cannotOpenSocket(path, errno: errno)
        }
    }

    /// Starts the read loop. Call once.
    func start(
        onBytes: @escaping @Sendable (Data) -> Void,
        onClose: @escaping @Sendable (String?) -> Void
    ) {
        queue.async {
            self.onBytes = onBytes
            self.onClose = onClose
            self.readMore()
        }
    }

    private func readMore() {
        dispatchIO.read(offset: 0, length: Int.max, queue: queue) { [weak self] done, data, error in
            guard let self else { return }
            if let data, !data.isEmpty {
                self.onBytes?(Data(data))
            }
            guard done else { return }
            if error != 0 {
                self.finish(reason: String(cString: strerror(error)))
            } else if data?.isEmpty ?? true {
                self.finish(reason: nil)
            } else {
                self.readMore()
            }
        }
    }

    /// Writes one already framed line, newline included.
    func write(_ data: Data) {
        let payload = data.withUnsafeBytes { DispatchData(bytes: $0) }
        queue.async { [self] in
            guard !closed else { return }
            dispatchIO.write(offset: 0, data: payload, queue: queue) { [self] done, _, error in
                guard done, error != 0 else { return }
                finish(reason: String(cString: strerror(error)))
            }
        }
    }

    /// Closes the socket; the close handler fires once, whoever got there first.
    func close() {
        queue.async { self.finish(reason: nil) }
    }

    private func finish(reason: String?) {
        guard !closed else { return }
        closed = true
        dispatchIO.close(flags: .stop)
        let handler = onClose
        onBytes = nil
        onClose = nil
        handler?(reason)
    }
}
