import Foundation

/// A blocking TCP connection carrying one JSON document per line. The
/// agent protocol is strictly request/reply, so synchronous reads on a
/// dedicated background thread keep things simple.
final class LineConnection {
    private static let maximumLineBytes = 16 * 1024 * 1024

    private let descriptor: Int32
    private var buffer = Data()

    init(host: String, port: UInt16) throws {
        descriptor = socket(AF_INET, SOCK_STREAM, 0)
        guard descriptor >= 0 else {
            throw AgentError.connectionError("socket() failed: \(errno)")
        }

        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        guard inet_pton(AF_INET, host, &address.sin_addr) == 1 else {
            close(descriptor)
            throw AgentError.connectionError("invalid host: \(host)")
        }

        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard result == 0 else {
            close(descriptor)
            throw AgentError.connectionError(
                "could not connect to \(host):\(port): \(errno)")
        }

        var flag: Int32 = 1
        guard
            setsockopt(
                descriptor, SOL_SOCKET, SO_NOSIGPIPE, &flag,
                socklen_t(MemoryLayout<Int32>.size)) == 0
        else {
            close(descriptor)
            throw AgentError.connectionError(
                "could not disable SIGPIPE: \(errno)")
        }
        guard
            setsockopt(
                descriptor, Int32(IPPROTO_TCP), TCP_NODELAY, &flag,
                socklen_t(MemoryLayout<Int32>.size)) == 0
        else {
            close(descriptor)
            throw AgentError.connectionError(
                "could not enable TCP_NODELAY: \(errno)")
        }
    }

    deinit {
        close(descriptor)
    }

    func send<Message: Encodable>(_ message: Message) throws {
        var data = try JSONEncoder().encode(message)
        guard data.count <= Self.maximumLineBytes else {
            throw AgentError.protocolError(
                "message exceeds \(Self.maximumLineBytes) bytes")
        }
        data.append(0x0A)
        try data.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) in
            var remaining = bytes
            while !remaining.isEmpty {
                let written = write(
                    descriptor, remaining.baseAddress, remaining.count)
                if written < 0, errno == EINTR {
                    continue
                }
                guard written > 0 else {
                    throw AgentError.connectionError("write failed: \(errno)")
                }
                remaining = UnsafeRawBufferPointer(
                    rebasing: remaining[written...])
            }
        }
    }

    /// Read the next line; `nil` when the driver has closed the
    /// connection (i.e. the test is over).
    func receiveLine() throws -> Data? {
        while true {
            if let newline = buffer.firstIndex(of: 0x0A) {
                let line = buffer.prefix(upTo: newline)
                guard line.count <= Self.maximumLineBytes else {
                    throw AgentError.protocolError(
                        "message exceeds \(Self.maximumLineBytes) bytes")
                }
                buffer.removeSubrange(...newline)
                return Data(line)
            }
            guard buffer.count <= Self.maximumLineBytes else {
                throw AgentError.protocolError(
                    "message exceeds \(Self.maximumLineBytes) bytes")
            }
            var chunk = [UInt8](repeating: 0, count: 4096)
            let count = read(descriptor, &chunk, chunk.count)
            if count < 0, errno == EINTR {
                continue
            }
            if count < 0 {
                throw AgentError.connectionError("read failed: \(errno)")
            }
            if count == 0 {
                guard buffer.isEmpty else {
                    throw AgentError.protocolError(
                        "connection closed during a message")
                }
                return nil
            }
            buffer.append(contentsOf: chunk.prefix(count))
        }
    }
}
