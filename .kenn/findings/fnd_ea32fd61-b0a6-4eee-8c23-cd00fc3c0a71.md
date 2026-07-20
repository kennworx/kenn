---
id: fnd_ea32fd61-b0a6-4eee-8c23-cd00fc3c0a71
tags:
- directive
- polarity:dont
parent_ids: []
created_at: 2026-07-10T14:17:26.270787Z
---
kenn-dotnet must NOT register MSBuild at process start. `MSBuildLocator.RegisterDefaults()` throws when no .NET SDK is reachable, so calling it from Program.cs's top-level statements makes `--version` and `--help` die on exactly the machines that need to ask whether C# is indexable. Registration belongs in the `index` action (MsBuildBootstrap.TryRegister), in a frame that references no MSBuild type; `IndexCommand.Run` — the first method that loads one, via Roslyn's MSBuildWorkspace — carries [MethodImpl(MethodImplOptions.NoInlining)] so the JIT cannot hoist those type loads into the registering frame.

Also: the missing-toolchain diagnostic must be written with Console.Error.WriteLine, NEVER through ILogger. Program.cs sets the logger's minimum level from KENN_DOTNET_LOG, so `KENN_DOTNET_LOG=Critical` silences LogError entirely — exit 1 with zero bytes of explanation. It is the only output a user without an SDK ever sees; it must not be suppressible. Emit it exactly once on stderr and once on the wire (meta+error+end), not both plus a raw write.