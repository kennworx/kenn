import XCTest

@testable import kenn_swift

/// Guards the `posix_spawn` runner that replaced `Foundation.Process` (the one
/// that deadlocks in swift:6.x containers). `/bin/sh` exists on macOS + Linux.
final class ProcessRunnerTests: XCTestCase {
    func testRunReturnsTrueOnZeroExit() {
        XCTAssertTrue(ProcessRunner.run("/bin/sh", ["-c", "exit 0"]))
    }

    func testRunReturnsFalseOnNonZeroExit() {
        XCTAssertFalse(ProcessRunner.run("/bin/sh", ["-c", "exit 7"]))
    }

    func testRunReturnsFalseWhenSpawnFails() {
        XCTAssertFalse(ProcessRunner.run("/nonexistent/kenn-binary-xyz", []))
    }

    func testCaptureReturnsChildStdout() {
        let data = ProcessRunner.capture("/bin/sh", ["-c", "printf 'hello world'"])
        XCTAssertEqual(data.flatMap { String(data: $0, encoding: .utf8) }, "hello world")
    }

    func testCaptureDrainsMoreThanAPipeBuffer() {
        // 200 KiB exceeds the OS pipe buffer, so a runner that waited before
        // draining would deadlock or truncate. Proves read-to-EOF-before-wait.
        let n = 200_000
        let data = ProcessRunner.capture("/bin/sh", ["-c", "yes x | head -c \(n)"])
        XCTAssertEqual(data?.count, n)
    }

    func testCaptureReturnsNilOnNonZeroExit() {
        XCTAssertNil(ProcessRunner.capture("/bin/sh", ["-c", "printf out; exit 1"]))
    }
}
