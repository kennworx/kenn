using System.CommandLine;
using Kenn.Dotnet.Cli;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Console;

// MSBuild registration deliberately does NOT happen here: it would make
// `--version` and `--help` fail on a machine with no .NET SDK. It runs in the
// `index` action instead, still before any MSBuild type loads. See
// MsBuildBootstrap.

// Log level via env var so the factory can be created before parse.
// KENN_DOTNET_LOG = Trace | Debug | Information | Warning | Error.
var level = Enum.TryParse<LogLevel>(
    Environment.GetEnvironmentVariable("KENN_DOTNET_LOG"),
    ignoreCase: true,
    out var lvl) ? lvl : LogLevel.Information;

using var loggerFactory = LoggerFactory.Create(b => b
    .SetMinimumLevel(level)
    .AddConsole(o =>
    {
        // Route ALL log output to stderr; stdout is reserved for JSONL.
        o.LogToStandardErrorThreshold = LogLevel.Trace;
        o.FormatterName = ConsoleFormatterNames.Simple;
    }));

var root = new RootCommand("Streaming C# indexer (Roslyn → JSONL)")
{
    IndexCommand.Build(loggerFactory),
};

return await root.Parse(args).InvokeAsync();
