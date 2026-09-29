import Darwin
import Foundation

/// A Unix-socket server that speaks just enough of the protocol to test the client.
///
/// It is started inside the test, on a path short enough for `sockaddr_un`, and
/// it answers whatever the test's handler says — including out of order, which
/// the protocol allows and the client has to cope with.
final class FakeDaemon: @unchecked Sendable {
    /// One request, as the handler sees it.
    struct Request: Sendable {
        var id: Int?
        var method: String

        init?(line: Data) {
            guard let object = try? JSONSerialization.jsonObject(with: line),
                  let dictionary = object as? [String: Any],
                  let method = dictionary["method"] as? String
            else { return nil }
            self.id = dictionary["id"] as? Int
            self.method = method
        }
    }

    let path: String
    private let listener: Int32
    private var client: Int32 = -1
    private let lock = NSLock()
    private var stopped = false
    private let handler: @Sendable (Request, FakeDaemon) -> Void

    init(handler: @escaping @Sendable (Request, FakeDaemon) -> Void) throws {
        self.handler = handler
        path = "/tmp/ts-\(UUID().uuidString.prefix(8)).sock"
        unlink(path)

        listener = socket(AF_UNIX, SOCK_STREAM, 0)
        guard listener >= 0 else { throw Failure.cannotListen(errno) }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &address.sun_path) { destination in
            destination.copyBytes(from: bytes)
            destination[bytes.count] = 0
        }
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { generic in
                bind(listener, generic, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, listen(listener, 4) == 0 else {
            close(listener)
            throw Failure.cannotListen(errno)
        }

        Thread.detachNewThread { [weak self] in self?.serve() }
    }

    enum Failure: Error {
        case cannotListen(Int32)
    }

    private func serve() {
        let accepted = accept(listener, nil, nil)
        guard accepted >= 0 else { return }
        lock.lock()
        client = accepted
        lock.unlock()

        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 4096)
        while true {
            let read = Darwin.read(accepted, &chunk, chunk.count)
            guard read > 0 else { break }
            buffer.append(contentsOf: chunk[0..<read])
            while let newline = buffer.firstIndex(of: UInt8(ascii: "\n")) {
                let line = Data(buffer[buffer.startIndex..<newline])
                buffer.removeSubrange(buffer.startIndex...newline)
                if let request = Request(line: line) {
                    handler(request, self)
                }
            }
        }
        stop()
    }

    /// Writes one raw JSON line to the connected client.
    func send(_ json: String) {
        lock.lock()
        let descriptor = client
        lock.unlock()
        guard descriptor >= 0 else { return }
        let line = Data((json + "\n").utf8)
        line.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            _ = Darwin.write(descriptor, base, raw.count)
        }
    }

    func reply(to id: Int, result: String) {
        send(#"{"jsonrpc":"2.0","id":\#(id),"result":\#(result)}"#)
    }

    func fail(_ id: Int, code: Int, message: String, data: String? = nil) {
        let dataPart = data.map { #","data":\#($0)"# } ?? ""
        send(#"{"jsonrpc":"2.0","id":\#(id),"error":{"code":\#(code),"message":"\#(message)"\#(dataPart)}}"#)
    }

    func stop() {
        lock.lock()
        defer { lock.unlock() }
        guard !stopped else { return }
        stopped = true
        if client >= 0 { close(client) }
        close(listener)
        unlink(path)
    }

    deinit {
        stop()
    }
}
