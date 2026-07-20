import Foundation

#if canImport(Darwin)
    import Darwin
#elseif canImport(Glibc)
    import Glibc
#endif

/// Buffered JSONL writer to stdout plus the run-local id allocator and edge
/// counters. Mirrors the kenn-dotnet `JsonlSink` role: every line is one JSON
/// object; `Ref` ids are assigned monotonically from 1 (0 means "no ref").
final class Sink {
    private var buffer = Data()
    private let flushBytes: Int
    private let output: FileHandle
    private var nextId: UInt32 = 1

    private(set) var fileCount: Int = 0
    private(set) var symbolCount: Int = 0
    private(set) var edgeCount: Int = 0
    private(set) var errorCount: Int = 0

    init(output: FileHandle = .standardOutput, flushBytes: Int = 1 << 20) {
        self.output = output
        self.flushBytes = flushBytes
    }

    /// Allocate the next run-local wire id.
    func allocId() -> UInt32 {
        defer { nextId += 1 }
        return nextId
    }

    func countFile() { fileCount += 1 }
    func countSymbol() { symbolCount += 1 }
    func countEdge() { edgeCount += 1 }
    func countError() { errorCount += 1 }

    /// Serialize one frame object as a single JSONL line. Keys are sorted for
    /// deterministic output; the consumer is order-independent.
    func write(_ frame: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: frame, options: [.sortedKeys]) else {
            return
        }
        buffer.append(data)
        buffer.append(0x0A) // "\n"
        if buffer.count >= flushBytes {
            flush()
        }
    }

    func flush() {
        guard !buffer.isEmpty else { return }
        output.write(buffer)
        buffer.removeAll(keepingCapacity: true)
    }
}

/// stderr logging helper (the wire is stdout-only).
func logError(_ message: String) {
    FileHandle.standardError.write(Data("kenn-swift: \(message)\n".utf8))
}

/// ISO-8601 UTC timestamp with millisecond precision, matching the wire's
/// `MetaFrame.ts` / `EndFrame.ts` contract (`YYYY-MM-DDTHH:mm:ss.sssZ`).
func iso8601Now() -> String {
    let fmt = ISO8601DateFormatter()
    fmt.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    fmt.timeZone = TimeZone(identifier: "UTC")
    return fmt.string(from: Date())
}

/// POSIX `realpath` of a workspace path — the form the compiler records in
/// index-store units (`/private/tmp/...`, `/private/var/...`). Foundation's
/// `resolvingSymlinksInPath()` is NOT this: it STRIPS a `/private` prefix
/// (documented NSString behavior), so a workspace under `/tmp` or `/var`
/// would never prefix-match its units' `mainFile` paths and the reader
/// would silently emit nothing. Falls back to the input when the path
/// cannot be resolved (e.g. does not exist). Windows has no `/private`
/// alias hazard, so Foundation's resolution is sufficient there.
func canonicalWorkspaceRoot(_ path: String) -> String {
    #if canImport(Darwin) || canImport(Glibc)
        guard let rp = realpath(path, nil) else { return path }
        defer { free(rp) }
        return String(cString: rp)
    #else
        return URL(fileURLWithPath: path).resolvingSymlinksInPath().path
    #endif
}

/// FNV-1a 64-bit hex digest — a stable content hash for `FileFrame.content_hash`
/// (the wire field is a string).
func fnv1a64Hex(_ data: Data) -> String {
    var hash: UInt64 = 0xcbf2_9ce4_8422_2325
    let prime: UInt64 = 0x0000_0100_0000_01b3
    for byte in data {
        hash ^= UInt64(byte)
        hash = hash &* prime
    }
    return String(format: "%016llx", hash)
}
