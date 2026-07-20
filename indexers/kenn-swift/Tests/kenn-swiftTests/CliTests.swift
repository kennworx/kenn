import XCTest

@testable import kenn_swift

/// `kenn init` probes each indexer with `--version` to decide whether the
/// language is indexable. Silently ignoring an unrecognised option meant
/// `kenn-swift index --version` discarded the flag and started a full workspace
/// build instead of answering.
final class CliTests: XCTestCase {
    func testKnownOptionsParse() throws {
        let args = try parseIndexArgs(
            ["--workspace", "/w", "--projects", "a/Package.swift", "--projects", "b/Package.swift",
             "--skip-build", "--platform", "ios", "--store", "/s"],
            defaultWorkspace: "/default")

        XCTAssertEqual(args.workspace, "/w")
        XCTAssertEqual(args.projects, ["a/Package.swift", "b/Package.swift"])
        XCTAssertTrue(args.skipBuild)
        XCTAssertEqual(args.platform, "ios")
        XCTAssertEqual(args.storeOverride, "/s")
    }

    func testEmptyArgvUsesTheDefaultWorkspace() throws {
        let args = try parseIndexArgs([], defaultWorkspace: "/default")
        XCTAssertEqual(args, IndexArgs(workspace: "/default"))
    }

    /// The regression: `--version` after `index` must not fall through to a build.
    func testUnknownOptionIsRejectedRatherThanIgnored() {
        XCTAssertThrowsError(try parseIndexArgs(["--version"], defaultWorkspace: "/default")) { err in
            XCTAssertEqual(err as? CliError, .unknownOption("--version"))
        }
        XCTAssertThrowsError(
            try parseIndexArgs(["--workspace", "/w", "--nope"], defaultWorkspace: "/default")
        ) { err in
            XCTAssertEqual(err as? CliError, .unknownOption("--nope"))
        }
    }

    func testOptionMissingItsValueIsAnError() {
        XCTAssertThrowsError(try parseIndexArgs(["--workspace"], defaultWorkspace: "/default")) { err in
            XCTAssertEqual(err as? CliError, .missingValue("--workspace"))
        }
    }

    /// Only options are rejected. A stray positional was always tolerated, and
    /// the `--version` bug did not require narrowing that.
    func testPositionalArgumentsAreStillTolerated() throws {
        let args = try parseIndexArgs(["/some/path", "--workspace", "/w"], defaultWorkspace: "/default")
        XCTAssertEqual(args.workspace, "/w")
    }

    /// An option's value may itself look like a path, and must not be mistaken
    /// for a stray token or an unknown option.
    func testOptionValuesAreNotReparsedAsTokens() throws {
        let args = try parseIndexArgs(
            ["--platform", "macos", "--projects", "/p/Package.swift"], defaultWorkspace: "/d")
        XCTAssertEqual(args.platform, "macos")
        XCTAssertEqual(args.projects, ["/p/Package.swift"])
    }

    /// An option cannot swallow the next option as its value. Silently setting
    /// `platform = "--skip-build"` and dropping the flag is the same class of
    /// misparse as ignoring `--version`.
    func testAnOptionCannotConsumeAnotherOptionAsItsValue() {
        XCTAssertThrowsError(
            try parseIndexArgs(["--platform", "--skip-build"], defaultWorkspace: "/d")
        ) { err in
            XCTAssertEqual(err as? CliError, .missingValue("--platform"))
        }
        XCTAssertThrowsError(
            try parseIndexArgs(["--workspace", "--projects", "/p"], defaultWorkspace: "/d")
        ) { err in
            XCTAssertEqual(err as? CliError, .missingValue("--workspace"))
        }
    }

    /// Bare, no name prefix — matching `kenn-dotnet --version` and `kenn-ts --version`.
    func testToolVersionIsABareSemver() {
        XCTAssertNotNil(toolVersion.range(of: #"^\d+\.\d+\.\d+$"#, options: .regularExpression))
    }

    /// `logError` supplies the `kenn-swift: ` prefix; duplicating it here
    /// produced `kenn-swift: kenn-swift: unknown option '--version'`.
    func testErrorMessagesAreNotPrefixed() {
        XCTAssertFalse(CliError.unknownOption("--x").message.hasPrefix("kenn-swift:"))
        XCTAssertFalse(CliError.missingValue("--x").message.hasPrefix("kenn-swift:"))
    }
}
