import Foundation

let toolVersion = "0.1.0"

let usageText =
    "usage: kenn-swift index --workspace <dir> [--projects <Package.swift>]... [--skip-build]"

/// Options for the `index` subcommand, as parsed from argv.
struct IndexArgs: Equatable {
    var workspace: String
    var projects: [String] = []
    var skipBuild = false
    /// Read this index store directly, bypassing provisioning.
    var storeOverride: String?
    /// Xcode build destination override (macos|ios|...); auto when nil.
    var platform: String?
}

enum CliError: Error, Equatable {
    case unknownOption(String)
    case missingValue(String)
}

/// Parse the arguments that follow `index`.
///
/// Unknown *options* are rejected rather than ignored. Silently skipping them
/// meant `kenn-swift index --version` discarded the flag and started a full
/// workspace build — a probe for whether the tool is runnable would instead
/// spend minutes compiling.
///
/// Non-option tokens are still tolerated, as they always were. Rejecting them
/// too would narrow the accepted argv beyond what that bug required.
func parseIndexArgs(_ argv: [String], defaultWorkspace: String) throws -> IndexArgs {
    var args = IndexArgs(workspace: defaultWorkspace)
    var idx = 0
    while idx < argv.count {
        let token = argv[idx]

        // An option's value is the next token — unless that token is itself an
        // option. `--platform --skip-build` would otherwise set the platform to
        // "--skip-build" and silently drop the flag.
        func value() throws -> String {
            idx += 1
            guard idx < argv.count, !argv[idx].hasPrefix("-") else {
                throw CliError.missingValue(token)
            }
            return argv[idx]
        }

        switch token {
        case "--workspace": args.workspace = try value()
        case "--projects": args.projects.append(try value())
        case "--skip-build": args.skipBuild = true
        case "--store": args.storeOverride = try value()
        case "--platform": args.platform = try value()
        default:
            if token.hasPrefix("-") { throw CliError.unknownOption(token) }
        }
        idx += 1
    }
    return args
}

extension CliError {
    /// Unprefixed: `logError` already writes the `kenn-swift: ` prefix.
    var message: String {
        switch self {
        case .unknownOption(let token): return "unknown option '\(token)'"
        case .missingValue(let token): return "'\(token)' requires a value"
        }
    }
}
