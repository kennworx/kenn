using System.Collections.Concurrent;
using System.Collections.Immutable;
using System.Diagnostics;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Xml.Linq;
using Kenn.Dotnet.Cli;
using Kenn.Dotnet.Wire;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.MSBuild;
using Microsoft.Extensions.FileSystemGlobbing;
using Microsoft.Extensions.Logging;

namespace Kenn.Dotnet.Indexing;

internal sealed class IndexerCore
{
    private static readonly SymbolDisplayFormat DisplayFormat = new(
        globalNamespaceStyle: SymbolDisplayGlobalNamespaceStyle.OmittedAsContaining,
        typeQualificationStyle: SymbolDisplayTypeQualificationStyle.NameOnly,
        genericsOptions: SymbolDisplayGenericsOptions.IncludeTypeParameters,
        memberOptions: SymbolDisplayMemberOptions.IncludeAccessibility
                       | SymbolDisplayMemberOptions.IncludeModifiers
                       | SymbolDisplayMemberOptions.IncludeParameters
                       | SymbolDisplayMemberOptions.IncludeType
                       | SymbolDisplayMemberOptions.IncludeContainingType,
        kindOptions: SymbolDisplayKindOptions.IncludeMemberKeyword
                     | SymbolDisplayKindOptions.IncludeTypeKeyword,
        miscellaneousOptions: SymbolDisplayMiscellaneousOptions.UseSpecialTypes
                              | SymbolDisplayMiscellaneousOptions.EscapeKeywordIdentifiers);

    private readonly IndexOptions _opts;
    private readonly JsonlSink _sink;
    private readonly ILogger _log;
    private readonly Matcher _pathMatcher;
    private readonly IdRegistry _ids = new();
    /// <summary>
    /// Per-thread scratch buffer for <see cref="PubId"/> /
    /// <see cref="IdRegistry.Key"/>. Each worker thread gets its own
    /// StringBuilder; reusing one across calls amortizes allocation across
    /// tens of thousands of symbols per worker. PubId.For* clears the
    /// buffer at entry so it never grows unbounded.
    /// </summary>
    private readonly ThreadLocal<StringBuilder> _keyBuf = new(() => new StringBuilder());
    private readonly FileTracker _files;
    // Compiled `testAssemblyRegex` patterns (config). A project whose assembly
    // name matches any is test code — fits a repo whose test assemblies all end
    // in `Test` (e.g. `FrontOffice.Test`, bare `Test`).
    private readonly IReadOnlyList<Regex> _testAssemblyRegexes;
    // Dedup keys for emitted edges. Split into two structures:
    //   - structural edges (no range, no field_op) live for the whole run
    //     because the same edge can legitimately be emitted from multiple
    //     trees (e.g. the same `using System.Collections;` in many files
    //     produces one logical `imports` edge). ConcurrentDictionary used
    //     as a thread-safe set under parallel project walks.
    //   - body-walk edges (with range or field_op) only need per-body dedup;
    //     a method body lives in exactly one tree, so cross-tree clashes
    //     are impossible. The set is now per-`BodyWalker`-instance so each
    //     worker has its own without cross-thread synchronization.
    internal readonly record struct BodyEdgeKey(EdgeKind Kind, uint Source, uint Target, Range Range, FieldOp? FieldOp);
    private readonly ConcurrentDictionary<(EdgeKind Kind, uint Source, uint Target), byte> _emittedStructuralEdges = new();
    private readonly IReadOnlySet<EdgeKind>? _edgeAllow;
    private long _symbolFullCount;
    private long _edges;
    private long _errors;

    // KENN_BENCH=1 emits per-stage timings on stderr. Mirrors the rust side's
    // KENN_BENCH gate. Eager-evaluated once at type init — no per-call lookup.
    private static readonly bool BenchEnabled =
        Environment.GetEnvironmentVariable("KENN_BENCH") == "1";

    // Per-project timing samples collected under parallel walks. Compile and
    // walk are measured separately so we can see whether GetCompilationAsync
    // or our walker is the long pole. Note: `proj_compile_ms` is biased
    // *low* because Roslyn defers metadata binding lazily — some compile
    // cost lands in the walk timer when the walker first touches symbols.
    private readonly ConcurrentBag<(long compileMs, long walkMs)> _projStats = new();

    public IndexerCore(IndexOptions opts, JsonlSink sink, ILogger log)
    {
        _opts = opts;
        _sink = sink;
        _log = log;
        _pathMatcher = new Matcher();
        _pathMatcher.AddIncludePatterns(opts.Include.Count == 0 ? new[] { "**" } : opts.Include);
        _pathMatcher.AddExcludePatterns(opts.Exclude);
        // Roslyn 5.3 + MSBuild 18.4 enables source generators (Razor, etc.) by
        // default; their output lives under `obj/` and changes every build.
        // Always exclude build-output directories — never useful in an index.
        _pathMatcher.AddExcludePatterns(new[] { "**/obj/**", "**/bin/**" });
        _files = new FileTracker(sink, opts.Workspace.FullName, _ids, new TestPathMatcher(opts.TestGlobs));
        _testAssemblyRegexes = CompileAssemblyRegexes(opts.TestAssemblyRegexes, log);
        _edgeAllow = opts.EdgeKindAllowlist;
    }

    /// <summary>True when <paramref name="path"/> passes the include/exclude globs.</summary>
    private bool IsPathInScope(string? path) =>
        !string.IsNullOrEmpty(path)
        && _pathMatcher.Match(_opts.Workspace.FullName, path).HasMatches;

    /// <summary>True when at least one of <paramref name="sym"/>'s source locations is in scope.</summary>
    private bool IsSymbolPathInScope(ISymbol sym)
    {
        foreach (var loc in sym.Locations)
        {
            if (loc.IsInSource && IsPathInScope(loc.SourceTree?.FilePath)) return true;
        }
        return false;
    }

    public async Task<EndStats> RunAsync(CancellationToken ct)
    {
        try
        {
            return await RunCoreAsync(ct);
        }
        finally
        {
            // Releases the per-thread StringBuilder slots; the underlying
            // ThreadPool threads outlive RunAsync, so the SBs themselves
            // become unrooted and GC-eligible after this. Disposing here
            // matches IDisposable discipline even if the indexer process
            // exits immediately after.
            _keyBuf.Dispose();
        }
    }

    private async Task<EndStats> RunCoreAsync(CancellationToken ct)
    {
        var entries = SolutionLoader.DiscoverProjectFiles(_opts);
        if (entries.Count == 0)
        {
            EmitError("indexer", $"No .sln or .csproj found under {_opts.Workspace.FullName}");
            return Stats();
        }

        // Shared workspace across all entries. Loading multiple .slns into
        // one MSBuildWorkspace lets Roslyn dedupe by project file path:
        // a project referenced by multiple .slns is loaded once, and its
        // metadata cache (referenced assemblies, source trees) is shared.
        // On a real multi-.sln workspace this collapses ~2x duplicated
        // Project objects down to the unique set and avoids re-doing
        // OpenSolution work for shared projects.
        using var ws = MSBuildWorkspace.Create();
        ws.LoadMetadataForReferencedProjects = true;
        var loadedPaths = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

        foreach (var entry in entries)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                await SolutionLoader.RunRestoreAsync(entry, _opts, _log, ct);
                var swOpen = Stopwatch.StartNew();
                await SolutionLoader.LoadEntryIntoSharedWorkspaceAsync(ws, entry, loadedPaths, _log, ct);
                swOpen.Stop();
                EmitEntryOpenBench(entry.Name, swOpen.ElapsedMilliseconds);
            }
            catch (Exception ex)
            {
                EmitError("indexer", FlattenMessage(ex.Message), path: entry.FullName);
                _log.LogDebug(ex, "stack trace for failed entry {Path}", entry.FullName);
            }
        }

        // Project list is now the union across all .slns, deduped by path.
        // DedupeTargetFrameworks still needed: a multi-targeted .csproj
        // shows up as multiple Project objects (one per TFM) sharing FilePath.
        var allProjects = ws.CurrentSolution.Projects.ToList();
        var deduped = SolutionLoader.DedupeTargetFrameworks(allProjects).ToList();
        foreach (var p in deduped.Where(p => p.Language != LanguageNames.CSharp))
        {
            EmitWarning("indexer",
                $"Skipping non-C# project {p.Name} ({p.Language})",
                path: p.FilePath);
        }
        var csProjects = deduped.Where(p => p.Language == LanguageNames.CSharp).ToList();
        // Register workspace packages up-front so cross-project refs emitted
        // from project A's body walk don't mark project B's package as
        // external before B itself gets walked.
        PreRegisterWorkspacePackages(csProjects);
        // Identify test projects from their MSBuild references and mark every
        // file in them as test *before* the walk — the set is fully populated
        // before any parallel IndexProject runs, so it stays race-free.
        MarkTestProjects(csProjects);

        var swWalk = Stopwatch.StartNew();
        await Parallel.ForEachAsync(
            csProjects,
            new ParallelOptions
            {
                MaxDegreeOfParallelism = _opts.MaxParallelism,
                CancellationToken = ct,
            },
            async (p, tok) => await IndexProject(p, tok));
        swWalk.Stop();
        EmitWalkBench(swWalk.ElapsedMilliseconds, csProjects.Count);

        foreach (var d in ws.Diagnostics)
        {
            if (d.Kind == WorkspaceDiagnosticKind.Failure)
                EmitError("msbuild", FlattenMessage(d.Message));
            else
                EmitWarning("msbuild", FlattenMessage(d.Message));
        }

        EmitBenchSummary();
        return Stats();
    }

    private EndStats Stats() => new()
    {
        Files = _files.Count,
        Symbols = _symbolFullCount,
        Edges = _edges,
        Errors = _errors,
    };

    /// <summary>
    /// Runtime assembly names that mark a C# project as test code: the test
    /// frameworks (xunit / nunit / mstest) and the VS Test Platform. A project
    /// referencing any of these is test code — it's part of what makes
    /// <c>dotnet test</c> discover it, and catches a test host/support project
    /// that uses a framework without the SDK's <c>IsTestProject</c> marker.
    /// </summary>
    internal static bool IsTestFrameworkAssembly(string assemblyName)
    {
        var n = assemblyName.ToLowerInvariant();
        return n.StartsWith("xunit", StringComparison.Ordinal)
            || n.StartsWith("nunit", StringComparison.Ordinal)
            || n.StartsWith("mstest", StringComparison.Ordinal)
            || n.StartsWith("microsoft.visualstudio.testplatform", StringComparison.Ordinal)
            || n.StartsWith("microsoft.testplatform", StringComparison.Ordinal);
    }

    /// <summary>
    /// NuGet package ids that mark their consumer as a test project: a test
    /// framework (xunit / nunit / mstest) or the <c>Microsoft.NET.Test.Sdk</c>
    /// meta-package that makes <c>dotnet test</c> discover the project (and
    /// implies <c>IsTestProject</c>). Matches package ids, not runtime assembly
    /// names — the two differ (the Test.Sdk package ships no
    /// <c>Microsoft.NET.Test.Sdk.dll</c>).
    /// </summary>
    internal static bool IsTestFrameworkPackage(string? packageId)
    {
        if (string.IsNullOrEmpty(packageId)) return false;
        var n = packageId.ToLowerInvariant();
        return n.StartsWith("xunit", StringComparison.Ordinal)
            || n.StartsWith("nunit", StringComparison.Ordinal)
            || n.StartsWith("mstest", StringComparison.Ordinal)
            || n == "microsoft.net.test.sdk";
    }

    /// <summary>
    /// Own-csproj test markers read straight from the XML: the SDK's
    /// <c>&lt;IsTestProject&gt;true&lt;/IsTestProject&gt;</c> property, or a
    /// <c>&lt;PackageReference&gt;</c> to a test framework / the Test SDK.
    /// Reads the source (not resolved <see cref="Project.MetadataReferences"/>),
    /// which is what makes detection fire under central package management +
    /// <c>--skip-restore</c>, where NuGet assemblies never resolve into the
    /// compilation. Element namespaces are ignored (matches both SDK-style and
    /// legacy csproj). Pure — no IO — so it is unit-testable.
    /// </summary>
    internal static bool XmlDeclaresTest(XDocument doc)
    {
        foreach (var el in doc.Descendants())
        {
            switch (el.Name.LocalName)
            {
                case "IsTestProject" when bool.TryParse(el.Value.Trim(), out var b) && b:
                    return true;
                case "PackageReference" when IsTestFrameworkPackage(el.Attribute("Include")?.Value):
                    return true;
            }
        }
        return false;
    }

    /// <summary>
    /// True when the csproj declares a test-framework <c>&lt;PackageReference&gt;</c>.
    /// This is the signal that is <em>contagious</em> across a project reference:
    /// a shared test-base (e.g. <c>Test.Util</c> → <c>nunit</c>) carries a
    /// framework without setting <c>&lt;IsTestProject&gt;</c>, and any project
    /// referencing it is itself test code. <c>&lt;IsTestProject&gt;</c> is
    /// deliberately excluded here — it is a self-marker, not inherited by
    /// referrers. Pure — no IO.
    /// </summary>
    internal static bool XmlReferencesTestFramework(XDocument doc) =>
        doc.Descendants().Any(el =>
            el.Name.LocalName == "PackageReference"
            && IsTestFrameworkPackage(el.Attribute("Include")?.Value));

    /// <summary>
    /// Workspace-relative <c>&lt;ProjectReference&gt;</c> paths declared by the
    /// csproj, resolved to absolute paths against its directory. Follows MSBuild
    /// item semantics for the <c>Include</c> attribute: a <c>;</c>-separated list
    /// of entries, each optionally whitespace-padded and authored with Windows
    /// <c>\</c> separators (e.g. <c>..\Lib\Test.Util\Test.Util.csproj</c>), which
    /// are normalized so the paths resolve on macOS/Linux too.
    ///
    /// Not resolved: MSBuild property / item / glob expansion
    /// (<c>$(SolutionDir)…</c>, <c>@(…)</c>, <c>**</c>) — that needs a full
    /// evaluation, the cost this whole path avoids. Such an entry yields a path
    /// that simply won't exist on disk and is skipped by the caller (never a
    /// crash or a false positive). Pure — no IO.
    /// </summary>
    internal static IEnumerable<string> ProjectReferencePaths(XDocument doc, string csprojDir)
    {
        foreach (var el in doc.Descendants())
        {
            if (el.Name.LocalName != "ProjectReference") continue;
            var inc = el.Attribute("Include")?.Value;
            if (string.IsNullOrEmpty(inc)) continue;
            foreach (var entry in inc.Split(
                         ';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
            {
                yield return Path.GetFullPath(Path.Combine(csprojDir, entry.Replace('\\', '/')));
            }
        }
    }

    /// <summary>
    /// Cached per-csproj test signals — each file is parsed and traversed once
    /// per <see cref="MarkTestProjects"/> pass, however many projects reference
    /// it. <see cref="SelfDeclares"/> answers "is this project test code" (used
    /// when it is the project under test); <see cref="ReferencesFramework"/>
    /// answers "does it carry a test framework" (the contagious signal, used
    /// when it is a referenced test-base); <see cref="ProjectRefs"/> are its
    /// resolved <c>&lt;ProjectReference&gt;</c> targets, followed one level.
    /// </summary>
    internal readonly record struct CsprojTestInfo(
        bool SelfDeclares, bool ReferencesFramework, IReadOnlyList<string> ProjectRefs);

    /// <summary>
    /// Memoized <see cref="CsprojTestInfo"/> for <paramref name="path"/>: parse
    /// and evaluate the csproj on first sight, then serve every later query
    /// (own-project or referenced test-base) from the cache with no re-parse or
    /// re-traversal. A missing or malformed file caches an all-false entry so it
    /// is never retried.
    /// </summary>
    private static CsprojTestInfo CsprojInfo(string path, Dictionary<string, CsprojTestInfo> cache)
    {
        if (cache.TryGetValue(path, out var info)) return info;
        info = ComputeCsprojInfo(path);
        cache[path] = info;
        return info;
    }

    private static CsprojTestInfo ComputeCsprojInfo(string path)
    {
        if (!File.Exists(path)) return new CsprojTestInfo(false, false, Array.Empty<string>());
        XDocument doc;
        try { doc = XDocument.Load(path); }
        catch (Exception) { return new CsprojTestInfo(false, false, Array.Empty<string>()); }
        var dir = Path.GetDirectoryName(path);
        var refs = string.IsNullOrEmpty(dir)
            ? Array.Empty<string>()
            : ProjectReferencePaths(doc, dir).ToArray();
        return new CsprojTestInfo(XmlDeclaresTest(doc), XmlReferencesTestFramework(doc), refs);
    }

    /// <summary>
    /// A C# project's csproj declares it test code when its own XML says so
    /// (<see cref="XmlDeclaresTest"/>), OR it has a <c>&lt;ProjectReference&gt;</c>
    /// to a project that carries a test framework (<see cref="XmlReferencesTestFramework"/>)
    /// — a shared test-base whose framework MSBuild resolves neither as a NuGet
    /// assembly nor as the referenced project's output under <c>--skip-restore</c>.
    /// The project-reference hop is followed one level (direct references only)
    /// and reads the target csproj straight from disk, so it works even when
    /// that project failed to load into the workspace. Results are memoized in
    /// <paramref name="cache"/> so each csproj is parsed and traversed once.
    /// Missing/malformed csproj ⇒ not a test project.
    /// </summary>
    internal static bool CsprojDeclaresTest(string? csprojPath, Dictionary<string, CsprojTestInfo> cache)
    {
        if (string.IsNullOrEmpty(csprojPath)) return false;
        var info = CsprojInfo(csprojPath, cache);
        if (info.SelfDeclares) return true;
        foreach (var refPath in info.ProjectRefs)
        {
            if (CsprojInfo(refPath, cache).ReferencesFramework) return true;
        }
        return false;
    }

    /// <summary>Convenience overload with a private one-shot cache — for unit
    /// tests and any single, non-batched call.</summary>
    internal static bool CsprojDeclaresTest(string? csprojPath) =>
        CsprojDeclaresTest(csprojPath, new Dictionary<string, CsprojTestInfo>());

    private void MarkTestProjects(IEnumerable<Project> projects)
    {
        // One csproj-info cache for the whole pass: a shared test-base (e.g.
        // Test.Util) referenced by many test projects is parsed and evaluated
        // once, and each project's own csproj once.
        var cache = new Dictionary<string, CsprojTestInfo>();
        foreach (var project in projects)
        {
            if (IsTestProject(project, cache))
            {
                _files.MarkTestProject(project.Documents.Select(d => d.FilePath));
            }
        }
    }

    /// <summary>
    /// A C# project is test code when any of these hold: its assembly name
    /// matches a configured <c>testAssemblyRegex</c> (e.g. every test assembly
    /// ends in <c>Test</c>); its csproj declares <c>&lt;IsTestProject&gt;true</c>
    /// or references a test framework / the Test SDK (read from source, so it
    /// works under central package management + <c>--skip-restore</c>); or the
    /// resolved compilation references a test-framework assembly (covers cases
    /// where the framework arrives transitively but only when restored). Every
    /// symbol in a test project emits test=true.
    /// </summary>
    private bool IsTestProject(Project project, Dictionary<string, CsprojTestInfo> cache)
    {
        var asm = project.AssemblyName;
        if (!string.IsNullOrEmpty(asm))
        {
            foreach (var rx in _testAssemblyRegexes)
            {
                if (rx.IsMatch(asm)) return true;
            }
        }
        if (CsprojDeclaresTest(project.FilePath, cache)) return true;
        foreach (var reference in project.MetadataReferences)
        {
            if (reference is PortableExecutableReference { FilePath: { } path }
                && IsTestFrameworkAssembly(Path.GetFileNameWithoutExtension(path)))
            {
                return true;
            }
        }
        return false;
    }

    private static IReadOnlyList<Regex> CompileAssemblyRegexes(
        IReadOnlyList<string> patterns, ILogger log)
    {
        var compiled = new List<Regex>(patterns.Count);
        foreach (var pattern in patterns)
        {
            try
            {
                compiled.Add(new Regex(pattern, RegexOptions.CultureInvariant));
            }
            catch (ArgumentException ex)
            {
                log.LogWarning(ex, "Ignoring invalid testAssemblyRegex `{Pattern}`", pattern);
            }
        }
        return compiled;
    }

    private async Task IndexProject(Project project, CancellationToken ct)
    {
        var swCompile = BenchEnabled ? Stopwatch.StartNew() : null;
        var compilation = await project.GetCompilationAsync(ct);
        swCompile?.Stop();
        if (compilation is null)
        {
            EmitWarning("indexer", $"No compilation for project {project.Name}", path: project.FilePath);
            return;
        }

        var swWalk = BenchEnabled ? Stopwatch.StartNew() : null;
        var asmName = compilation.AssemblyName ?? project.AssemblyName ?? project.Name;
        var version = project.Version.ToString() ?? string.Empty;
        await IndexCompilationAsync(compilation, asmName, version, ct);
        swWalk?.Stop();
        if (BenchEnabled)
        {
            _projStats.Add((swCompile!.ElapsedMilliseconds, swWalk!.ElapsedMilliseconds));
        }
    }

    /// <summary>
    /// Walk a prebuilt compilation: emit its package, symbol tree, contains
    /// edges, and per-tree body edges/imports. Shared by
    /// <see cref="IndexProject"/> and by unit tests that construct an in-memory
    /// <see cref="Compilation"/> directly instead of loading an MSBuild project.
    /// </summary>
    internal async Task IndexCompilationAsync(
        Compilation compilation, string asmName, string version, CancellationToken ct)
    {
        var packageId = EnsurePackage(asmName, version, external: false);

        WalkNamespace(compilation.Assembly.GlobalNamespace, parentId: 0, packageId);
        EmitContainsEdges(compilation, packageId);

        foreach (var tree in compilation.SyntaxTrees)
        {
            ct.ThrowIfCancellationRequested();
            var path = tree.FilePath;
            if (string.IsNullOrEmpty(path)) continue;
            if (!_pathMatcher.Match(_opts.Workspace.FullName, path).HasMatches) continue;

            var model = compilation.GetSemanticModel(tree);
            var root = await tree.GetRootAsync(ct);
            // Each BodyWalker owns its own per-tree edge-dedup set; safe under
            // parallel project walks because the walker is local to this
            // worker and each tree lives in exactly one project.
            new BodyWalker(this, model, packageId).Visit(root);
            EmitImports(root, model, packageId);
        }
    }

    private uint EnsurePackage(string asmName, string version, bool external)
    {
        var (id, isNew) = _ids.RegisterPackage(asmName, version);
        if (!isNew) return id;
        _sink.Write(new PackageFrame
        {
            Id = id,
            Name = asmName,
            Version = string.IsNullOrEmpty(version) ? null : version,
            Manager = external ? "nuget" : null,
            External = external,
        });
        return id;
    }

    /// <summary>
    /// Resolve the package id for an arbitrary symbol. Workspace symbols
    /// share <paramref name="workspacePkgId"/>; symbols from referenced
    /// assemblies (BCL, NuGet) get their own external PackageFrame interned
    /// by `(name, version)`.
    /// </summary>
    private uint EnsurePackageForSymbol(ISymbol sym, uint workspacePkgId)
    {
        if (sym.ContainingAssembly is not { } asm) return workspacePkgId;
        var name = asm.Name;
        var version = asm.Identity.Version.ToString();
        var (id, isNew) = _ids.RegisterPackage(name, version);
        if (isNew && id != workspacePkgId)
        {
            _sink.Write(new PackageFrame
            {
                Id = id,
                Name = name,
                Version = version,
                Manager = "nuget",
                External = true,
            });
        }
        return id;
    }

    private void WalkNamespace(INamespaceSymbol ns, uint parentId, uint packageId)
    {
        var thisId = parentId;
        if (!ns.IsGlobalNamespace && SymbolFilter.IsInSource(ns) && IsSymbolPathInScope(ns))
        {
            thisId = EnsureFullSymbolForDeclared(ns, parentId, packageId);
            if (parentId != 0)
            {
                EmitEdge(EdgeKind.DefinedIn, thisId, parentId);
            }
        }

        foreach (var member in ns.GetMembers())
        {
            if (member is INamespaceSymbol child)
            {
                WalkNamespace(child, thisId, packageId);
            }
            else if (member is INamedTypeSymbol type)
            {
                WalkType(type, thisId, packageId);
            }
        }
    }

    private void WalkType(INamedTypeSymbol type, uint parentId, uint packageId)
    {
        if (!SymbolFilter.IsInSource(type)) return;
        if (SymbolFilter.IsLocalSymbol(type)) return;
        if (!IsSymbolPathInScope(type)) return;

        var typeId = EnsureFullSymbolForDeclared(type, parentId, packageId);
        if (parentId != 0) EmitEdge(EdgeKind.DefinedIn, typeId, parentId);

        // Partial declarations: emit one extra SymbolFrame per additional
        // declaration site with `partial: true` and a fresh wire id sharing
        // the same key+pkg. The consumer's dedup logic appends defs.
        if (type.DeclaringSyntaxReferences.Length > 1)
        {
            EmitPartialAdditionalDefs(type, packageId, parentId);
        }

        EmitGenericConstraints(type.TypeParameters, typeId, packageId);
        EmitTypeRelationships(type, typeId, packageId);

        foreach (var member in type.GetMembers())
        {
            if (member is INamedTypeSymbol nested)
            {
                WalkType(nested, typeId, packageId);
                continue;
            }
            if (SymbolFilter.IsLocalSymbol(member)) continue;
            if (!SymbolFilter.IsInSource(member)) continue;
            if (!IsSymbolPathInScope(member)) continue;
            if (member.IsImplicitlyDeclared && member is IMethodSymbol mImp
                && mImp.MethodKind != MethodKind.Constructor) continue;

            var memId = EnsureFullSymbolForDeclared(member, typeId, packageId);
            EmitEdge(EdgeKind.DefinedIn, memId, typeId);

            if (member is IMethodSymbol method)
            {
                EmitMethodRelationships(method, memId, packageId);
                EmitGenericConstraints(method.TypeParameters, memId, packageId);
            }
        }
    }

    private void EmitPartialAdditionalDefs(INamedTypeSymbol type, uint packageId, uint parentId)
    {
        var key = IdRegistry.Key(_keyBuf.Value!, type);
        var name = type.Name.Length > 0 ? type.Name : type.MetadataName;
        var kind = KindMap.For(type);
        foreach (var refExtra in type.DeclaringSyntaxReferences.Skip(1))
        {
            if (!IsPathInScope(refExtra.SyntaxTree.FilePath)) continue;
            var loc = refExtra.SyntaxTree.GetLocation(refExtra.Span);
            var range = RangeUtil.FromLocation(loc);
            var fileId = _files.RegisterIfNew(refExtra.SyntaxTree.FilePath, refExtra.SyntaxTree);
            if (fileId == 0 || range is not { } r) continue;
            // Fresh wire id (NOT registered in the symbol intern table) so
            // the consumer sees a cross-wire-id collision on (key, pkg) and
            // appends an additional def row instead of overwriting.
            var extraId = _ids.Allocate();
            _sink.Write(new SymbolFrame
            {
                Id = extraId,
                Package = packageId,
                Key = key,
                Kind = kind,
                Name = name,
                Parent = parentId,
                File = fileId,
                Range = r,
                Body = RangeUtil.FromSyntaxNode(refExtra.GetSyntax()),
                Partial = true,
                Nargs = 0,
                Targs = type.Arity,
                Test = _files.IsTest(fileId),
            });
        }
    }

    /// <summary>
    /// Allocate-or-reuse the id for a source-declared symbol and emit a
    /// full SymbolFrame. If a stub for this key was previously emitted via
    /// <see cref="EnsureRefStub"/>, the same id is returned and the
    /// consumer treats this SymbolFrame as the upgrade.
    /// </summary>
    private uint EnsureFullSymbolForDeclared(ISymbol sym, uint parentId, uint packageId)
    {
        var key = IdRegistry.KeyForRegister(_keyBuf.Value!, sym, packageId);
        var id = _ids.RegisterSymbol(key);

        // Prefer an in-scope source location so range/file aren't pinned
        // to a generated obj/ part of a partial type.
        var loc = sym.Locations.FirstOrDefault(l => l.IsInSource && IsPathInScope(l.SourceTree?.FilePath))
                  ?? sym.Locations.FirstOrDefault(l => l.IsInSource);
        var fileId = loc?.SourceTree is { FilePath: var fp } srcTree
            ? _files.RegisterIfNew(fp, srcTree)
            : 0u;
        var range = RangeUtil.FromLocation(loc) ?? new Range(0, 0, 0, 0);

        // Full declaration span (attribute lists + member body) from the
        // declaring syntax node that matches the in-scope name location.
        // Null (field omitted) for metadata-only symbols with no source syntax.
        var bodyRef = sym.DeclaringSyntaxReferences.FirstOrDefault(r => r.SyntaxTree.FilePath == loc?.SourceTree?.FilePath)
                      ?? sym.DeclaringSyntaxReferences.FirstOrDefault();
        Range? body = bodyRef is null ? null : RangeUtil.FromSyntaxNode(bodyRef.GetSyntax());

        EmitFullSymbol(new SymbolFrame
        {
            Id = id,
            Package = sym is INamespaceSymbol ? 0u : packageId,
            Key = IdRegistry.Key(_keyBuf.Value!, sym),
            Kind = KindMap.For(sym),
            Name = sym.Name.Length > 0 ? sym.Name : sym.MetadataName,
            Parent = parentId,
            File = fileId,
            Range = range,
            Body = body,
            Nargs = sym is IMethodSymbol mm ? mm.Parameters.Length : 0,
            Targs = sym switch
            {
                INamedTypeSymbol t => t.Arity,
                IMethodSymbol m => m.Arity,
                _ => 0,
            },
            Partial = sym.DeclaringSyntaxReferences.Length > 1,
            Test = _files.IsTest(fileId),
            Sig = BuildSignatureDoc(sym),
            Doc = DocXml.Normalize(sym.GetDocumentationCommentXml()),
        });
        return id;
    }

    /// <summary>
    /// Allocate-or-reuse the id for a referenced symbol. Returns 0 when the
    /// symbol is not addressable (anonymous-type members, locals).
    ///
    /// Normalizations applied before keying:
    ///   - Extension methods via reduced syntax (`obj.Foo()` where Foo is
    ///     defined as `static Foo(this T)`) use `ReducedFrom` so the key
    ///     matches the static-class declaration site.
    ///   - All symbols are normalized to OriginalDefinition.
    ///
    /// On first sight, emits a minimal stub SymbolFrame so the consumer has
    /// something under this id for the upcoming edge. Later, if the symbol
    /// is walked as a full declaration, the full record overwrites.
    /// </summary>
    private uint EnsureRefStub(ISymbol sym, uint workspacePkgId)
    {
        // Reduced extension methods point at the receiver's type. Walk back
        // to the original static-method declaration.
        if (sym is IMethodSymbol { ReducedFrom: { } original })
        {
            sym = original;
        }
        // Compiler-synthesized accessors (`add_X`, `remove_X`, `get_X`,
        // `set_X`) and the implicit `Invoke` of a delegate type aren't
        // independently emitted by WalkType. Redirect references to the
        // associated symbol (the event / property / delegate type itself).
        if (sym is IMethodSymbol { IsImplicitlyDeclared: true, AssociatedSymbol: { } assoc })
        {
            sym = assoc;
        }
        else if (sym is IMethodSymbol { MethodKind: MethodKind.DelegateInvoke, ContainingType: { } delType })
        {
            sym = delType;
        }
        else if (sym is IMethodSymbol { IsImplicitlyDeclared: true, AssociatedSymbol: null, ContainingType: { } synthOwner })
        {
            // Records synthesize `Equals`, `ToString`, `<Clone>$`, etc.
            // No useful destination of their own; attribute calls to the
            // containing type.
            sym = synthOwner;
        }
        sym = sym.OriginalDefinition;

        // Locals and members of synthesized types aren't independently
        // addressable — anonymous types and tuple types are conjured per
        // use site, so their members never have a stable declaration.
        if (SymbolFilter.IsLocalSymbol(sym)) return 0;
        if (sym.ContainingType is { IsAnonymousType: true } or { IsTupleType: true }) return 0;
        if (sym.ContainingSymbol is INamedTypeSymbol parentType && SymbolFilter.IsLocalSymbol(parentType)) return 0;
        if (sym is INamedTypeSymbol nt && (nt.IsAnonymousType || nt.IsTupleType)) return 0;

        // Resolve owning package from the symbol's *containing assembly*,
        // never from the project we happen to be walking. A cross-project
        // ref to project B's `Foo` from project A's body MUST land under
        // B's package — otherwise the stub-from-A and the full-record-
        // from-B intern under different (key, pkg) pairs and get split
        // into two rows, halving the resolved edge count.
        var pkgId = EnsurePackageForSymbol(sym, workspacePkgId);
        var key = IdRegistry.KeyForRegister(_keyBuf.Value!, sym, pkgId);
        var (id, isNew) = _ids.RegisterSymbolIfNew(key);
        if (!isNew) return id;
        _sink.Write(new StubFrame
        {
            Id = id,
            Kind = KindMap.For(sym),
            Name = sym.Name.Length > 0 ? sym.Name : sym.MetadataName,
            Key = IdRegistry.Key(_keyBuf.Value!, sym),
            Package = sym is INamespaceSymbol ? 0u : pkgId,
        });
        return id;
    }

    /// <summary>
    /// Pre-register every workspace project's package so cross-project
    /// references emitted before the owning project is walked don't
    /// erroneously mark its package as external. Run once per kenn-dotnet
    /// invocation, before any indexing begins.
    /// </summary>
    private void PreRegisterWorkspacePackages(IEnumerable<Project> projects)
    {
        foreach (var p in projects)
        {
            var asmName = p.AssemblyName ?? p.Name;
            var version = p.Version.ToString() ?? string.Empty;
            EnsurePackage(asmName, version, external: false);
        }
    }

    private void EmitFullSymbol(SymbolFrame frame)
    {
        // Atomic test-and-set: under parallel project walks, two workers
        // can independently call EmitFullSymbol for the same shared
        // namespace id. Without this, both would emit a SymbolFrame and
        // both would increment the counter.
        if (_ids.MarkFullyEmittedIfFirst(frame.Id))
        {
            _sink.Write(frame);
            Interlocked.Increment(ref _symbolFullCount);
        }
    }

    private void EmitTypeRelationships(INamedTypeSymbol type, uint typeId, uint packageId)
    {
        var baseType = type.BaseType;
        while (baseType is not null && !IsObjectLike(baseType))
        {
            EmitEdge(EdgeKind.Implements, typeId, EnsureRefStub(baseType.OriginalDefinition, packageId));
            baseType = baseType.BaseType;
        }
        foreach (var iface in type.AllInterfaces)
        {
            EmitEdge(EdgeKind.Implements, typeId, EnsureRefStub(iface.OriginalDefinition, packageId));
        }
    }

    private void EmitMethodRelationships(IMethodSymbol method, uint methodId, uint packageId)
    {
        // Extension method → the type it extends. The method keeps its
        // `defined_in` to the holder static class (extension methods belong to
        // the holder in C#); this edge makes the method discoverable *from* the
        // receiver type, which `defined_in` alone never surfaces. The receiver
        // is the first (`this`) parameter; normalize to OriginalDefinition so
        // `this IEnumerable<T>` targets the open generic type.
        if (method.IsExtensionMethod && method.Parameters.Length > 0)
        {
            var receiver = method.Parameters[0].Type.OriginalDefinition;
            EmitEdge(EdgeKind.ExtendsType, methodId, EnsureRefStub(receiver, packageId));
        }

        var overridden = method.OverriddenMethod;
        while (overridden is not null)
        {
            EmitEdge(EdgeKind.Overrides, methodId, EnsureRefStub(overridden.OriginalDefinition, packageId));
            overridden = overridden.OverriddenMethod;
        }
        if (method.ContainingType is null) return;
        foreach (var iface in method.ContainingType.AllInterfaces)
        {
            foreach (var ifaceMember in iface.GetMembers())
            {
                var impl = method.ContainingType.FindImplementationForInterfaceMember(ifaceMember);
                if (impl is null) continue;
                if (!SymbolEqualityComparer.Default.Equals(impl, method)) continue;
                EmitEdge(EdgeKind.Overrides, methodId, EnsureRefStub(ifaceMember.OriginalDefinition, packageId));
            }
        }
    }

    private void EmitGenericConstraints(ImmutableArray<ITypeParameterSymbol> typeParams, uint ownerId, uint packageId)
    {
        if (typeParams.IsDefaultOrEmpty) return;
        foreach (var tp in typeParams)
        {
            foreach (var constraint in tp.ConstraintTypes)
            {
                if (constraint is INamedTypeSymbol named)
                {
                    EmitEdge(EdgeKind.GenericConstraint, ownerId, EnsureRefStub(named.OriginalDefinition, packageId));
                }
            }
        }
    }

    private void EmitContainsEdges(Compilation compilation, uint packageId)
    {
        var nsToFiles = new Dictionary<uint, HashSet<uint>>();
        var packageFiles = new HashSet<uint>();

        foreach (var tree in compilation.SyntaxTrees)
        {
            if (string.IsNullOrEmpty(tree.FilePath)) continue;
            if (!_pathMatcher.Match(_opts.Workspace.FullName, tree.FilePath).HasMatches) continue;
            var fileId = _files.RegisterIfNew(tree.FilePath, tree);
            if (fileId == 0) continue;
            packageFiles.Add(fileId);

            var model = compilation.GetSemanticModel(tree);
            foreach (var node in tree.GetRoot().DescendantNodesAndSelf()
                .Where(n => n is BaseNamespaceDeclarationSyntax))
            {
                if (model.GetDeclaredSymbol(node) is not INamespaceSymbol ns) continue;
                if (ns.IsGlobalNamespace) continue;
                var nsId = _ids.RegisterSymbol(IdRegistry.KeyForRegister(_keyBuf.Value!, ns, 0));
                if (!nsToFiles.TryGetValue(nsId, out var set))
                {
                    set = new HashSet<uint>();
                    nsToFiles[nsId] = set;
                }
                set.Add(fileId);
            }
        }

        foreach (var fileId in packageFiles)
        {
            EmitEdge(EdgeKind.Contains, packageId, fileId);
        }
        foreach (var (nsId, files) in nsToFiles)
        {
            foreach (var fileId in files) EmitEdge(EdgeKind.Contains, nsId, fileId);
        }
    }

    private void EmitImports(SyntaxNode root, SemanticModel model, uint packageId)
    {
        foreach (var u in root.DescendantNodes().OfType<UsingDirectiveSyntax>())
        {
            if (u.Name is null) continue;
            var info = model.GetSymbolInfo(u.Name);
            if (info.Symbol is not INamespaceSymbol ns) continue;
            var targetId = EnsureRefStub(ns, packageId);

            uint sourceId = packageId;
            for (var n = u.Parent; n is not null; n = n.Parent)
            {
                if (n is BaseNamespaceDeclarationSyntax nsDecl
                    && model.GetDeclaredSymbol(nsDecl) is INamespaceSymbol owner)
                {
                    sourceId = _ids.RegisterSymbol(IdRegistry.KeyForRegister(_keyBuf.Value!, owner, 0));
                    break;
                }
            }
            EmitEdge(EdgeKind.Imports, sourceId, targetId);
        }
    }

    private static bool IsObjectLike(INamedTypeSymbol t) =>
        t.SpecialType is SpecialType.System_Object
                       or SpecialType.System_Enum
                       or SpecialType.System_ValueType
                       or SpecialType.System_Delegate
                       or SpecialType.System_MulticastDelegate;

    private static string SafeDisplay(ISymbol sym)
    {
        try { return sym.ToDisplayString(DisplayFormat); }
        catch { return sym.Name; }
    }

    /// <summary>Bare signature line — no code fence, no language hint.</summary>
    private static string? BuildSignatureDoc(ISymbol sym)
    {
        try
        {
            var sig = sym.ToDisplayString(DisplayFormat);
            return string.IsNullOrEmpty(sig) ? null : sig;
        }
        catch { return null; }
    }

    public void EmitError(string source, string message, string? path = null, Range? range = null, string? code = null)
    {
        _sink.Write(new ErrorFrame { Severity = "error", Source = source, Message = message, Path = path, Range = range, Code = code });
        Interlocked.Increment(ref _errors);
    }

    public void EmitWarning(string source, string message, string? path = null, Range? range = null, string? code = null)
    {
        _sink.Write(new ErrorFrame { Severity = "warning", Source = source, Message = message, Path = path, Range = range, Code = code });
    }

    private static string FlattenMessage(string s) =>
        s.Replace("\r\n", " ").Replace('\n', ' ').Replace('\r', ' ');

    private static void EmitEntryOpenBench(string entryName, long openMs)
    {
        if (!BenchEnabled) return;
        Console.Error.WriteLine($"[bench] entry={entryName} open_ms={openMs}");
    }

    private static void EmitWalkBench(long walkMs, int projects)
    {
        if (!BenchEnabled) return;
        Console.Error.WriteLine($"[bench] walk_ms={walkMs} projects={projects}");
    }

    private void EmitBenchSummary()
    {
        if (!BenchEnabled || _projStats.IsEmpty) return;
        var compiles = _projStats.Select(s => s.compileMs).ToList();
        var walks = _projStats.Select(s => s.walkMs).ToList();
        var (cTotal, cMax, cP50, cP95) = Quantiles(compiles);
        var (wTotal, wMax, wP50, wP95) = Quantiles(walks);
        Console.Error.WriteLine(
            $"[bench] proj_compile total_ms={cTotal} max_ms={cMax} p50_ms={cP50} p95_ms={cP95} n={compiles.Count}");
        Console.Error.WriteLine(
            $"[bench] proj_walk    total_ms={wTotal} max_ms={wMax} p50_ms={wP50} p95_ms={wP95} n={walks.Count}");
    }

    private static (long total, long max, long p50, long p95) Quantiles(List<long> values)
    {
        if (values.Count == 0) return (0, 0, 0, 0);
        values.Sort();
        var total = 0L;
        foreach (var v in values) total += v;
        var max = values[^1];
        var p50 = values[values.Count / 2];
        var p95Idx = (int)Math.Floor(values.Count * 0.95);
        if (p95Idx >= values.Count) p95Idx = values.Count - 1;
        var p95 = values[p95Idx];
        return (total, max, p50, p95);
    }

    /// <summary>
    /// Emit an edge. Structural edges (no <paramref name="range"/>) dedup
    /// across the whole run via a thread-safe set. Body edges (with
    /// <paramref name="range"/>) dedup per-tree via the caller-provided
    /// <paramref name="bodyDedup"/> set, which is owned by the
    /// <see cref="BodyWalker"/> and never crosses worker boundaries.
    /// </summary>
    public void EmitEdge(
        EdgeKind kind,
        uint source,
        uint target,
        Range? range = null,
        FieldOp? fieldOp = null,
        HashSet<BodyEdgeKey>? bodyDedup = null)
    {
        if (_edgeAllow is not null && !_edgeAllow.Contains(kind)) return;
        if (source == 0 || target == 0 || source == target) return;
        if (range is { } r)
        {
            // Body edges: caller (BodyWalker) supplies a per-tree HashSet.
            if (bodyDedup is null) return;
            if (!bodyDedup.Add(new BodyEdgeKey(kind, source, target, r, fieldOp))) return;
        }
        else
        {
            // Structural edge — persists across the whole run, may be hit
            // concurrently by multiple worker tasks.
            if (!_emittedStructuralEdges.TryAdd((kind, source, target), 0)) return;
        }

        _sink.Write(new EdgeFrame
        {
            EdgeKind = kind,
            Source = source,
            Target = target,
            Range = range,
            FieldOp = fieldOp,
        });
        Interlocked.Increment(ref _edges);
    }

    /// <summary>
    /// Body walker — emits calls / type_use / field_access / instantiates
    /// edges. Sources are int ids of the enclosing fn/method/class; targets
    /// are int ids of the referenced symbols (via <see cref="EnsureRefStub"/>
    /// when we don't already have them).
    /// </summary>
    private sealed class BodyWalker : CSharpSyntaxWalker
    {
        private readonly IndexerCore _owner;
        private readonly SemanticModel _model;
        private readonly uint _packageId;
        // Per-walker dedup for body edges. A method body lives in exactly
        // one tree, so cross-walker collisions are impossible — this set
        // can stay a plain HashSet without synchronization.
        private readonly HashSet<BodyEdgeKey> _bodyEdges = new();

        public BodyWalker(IndexerCore owner, SemanticModel model, uint packageId)
        {
            _owner = owner;
            _model = model;
            _packageId = packageId;
        }

        private void EmitBodyEdge(EdgeKind kind, uint source, uint target, Range? range, FieldOp? fieldOp = null)
            => _owner.EmitEdge(kind, source, target, range, fieldOp, _bodyEdges);

        public override void VisitInvocationExpression(InvocationExpressionSyntax node)
        {
            base.VisitInvocationExpression(node);
            var info = _model.GetSymbolInfo(node);
            var target = info.Symbol ?? info.CandidateSymbols.FirstOrDefault();
            if (target is null) return;
            if (target is IMethodSymbol m && SymbolFilter.IsLocalSymbol(m)) return;
            if (target.ContainingType is null && target.ContainingNamespace is null) return;

            var sourceId = ResolveSource(node);
            if (sourceId == 0) return;
            var targetId = _owner.EnsureRefStub(target.OriginalDefinition, _packageId);
            EmitBodyEdge(EdgeKind.Calls, sourceId, targetId, RangeUtil.FromSyntaxNode(node.Expression));
        }

        public override void VisitObjectCreationExpression(ObjectCreationExpressionSyntax node)
        {
            base.VisitObjectCreationExpression(node);
            if (_model.GetSymbolInfo(node).Symbol is not IMethodSymbol ctor) return;
            var sourceId = ResolveSource(node);
            if (sourceId == 0) return;
            if (ctor.ContainingType is { } ct)
            {
                var range = RangeUtil.FromSyntaxNode(node.Type);
                EmitBodyEdge(EdgeKind.TypeUse, sourceId, _owner.EnsureRefStub(ct.OriginalDefinition, _packageId), range);
                EmitInstantiates(sourceId, ct, range);
            }
        }

        public override void VisitMemberAccessExpression(MemberAccessExpressionSyntax node)
        {
            base.VisitMemberAccessExpression(node);
            if (node.Parent is InvocationExpressionSyntax inv && inv.Expression == node) return;

            var sym = _model.GetSymbolInfo(node).Symbol;
            if (sym is not (IFieldSymbol or IPropertySymbol)) return;

            var sourceId = ResolveSource(node);
            if (sourceId == 0) return;
            var targetId = _owner.EnsureRefStub(sym.OriginalDefinition, _packageId);
            var op = ClassifyFieldOp(node);
            EmitBodyEdge(EdgeKind.FieldAccess, sourceId, targetId, RangeUtil.FromSyntaxNode(node.Name), op);
            EmitContainerTypeUse(node.Expression, sourceId);
        }

        public override void VisitIdentifierName(IdentifierNameSyntax node)
        {
            base.VisitIdentifierName(node);
            if (ShouldSkipIdentifier(node)) return;

            var sym = _model.GetSymbolInfo(node).Symbol;
            if (sym is null) return;

            switch (sym)
            {
                case INamedTypeSymbol type:
                    {
                        var sourceId = ResolveSource(node);
                        if (sourceId == 0) return;
                        var targetId = _owner.EnsureRefStub(type.OriginalDefinition, _packageId);
                        if (sourceId == targetId) return;
                        var range = RangeUtil.FromSyntaxNode(node);
                        EmitBodyEdge(EdgeKind.TypeUse, sourceId, targetId, range);
                        EmitInstantiates(sourceId, type, range);
                        break;
                    }
                case IFieldSymbol or IPropertySymbol:
                    {
                        var sourceId = ResolveSource(node);
                        if (sourceId == 0) return;
                        var targetId = _owner.EnsureRefStub(sym.OriginalDefinition, _packageId);
                        var op = ClassifyFieldOp(node);
                        EmitBodyEdge(EdgeKind.FieldAccess, sourceId, targetId, RangeUtil.FromSyntaxNode(node), op);
                        break;
                    }
            }
        }

        public override void VisitGenericName(GenericNameSyntax node)
        {
            base.VisitGenericName(node);
            // VisitObjectCreationExpression already emits type_use + instantiates
            // for the type being constructed; skip to avoid a duplicate edge.
            if (node.Parent is ObjectCreationExpressionSyntax oce && oce.Type == node) return;
            if (_model.GetSymbolInfo(node).Symbol is INamedTypeSymbol { IsGenericType: true } type)
            {
                var sourceId = ResolveSource(node);
                if (sourceId == 0) return;
                var targetId = _owner.EnsureRefStub(type.OriginalDefinition, _packageId);
                var range = RangeUtil.FromSyntaxNode(node);
                if (sourceId != targetId)
                {
                    EmitBodyEdge(EdgeKind.TypeUse, sourceId, targetId, range);
                }
                EmitInstantiates(sourceId, type, range);
            }
        }

        private static bool ShouldSkipIdentifier(IdentifierNameSyntax node) =>
            node.Parent switch
            {
                // Member access (left expression OR right name): VisitMemberAccessExpression
                // already emits field_access for the right side; the left side resolves
                // to a container (often the static class) which we don't track as a
                // type_use. Skipping both arms avoids the previous duplicate-emit on
                // the right name.
                MemberAccessExpressionSyntax => true,
                QualifiedNameSyntax qn when qn.Right != node => true,
                // VisitObjectCreationExpression already emits type_use for `new T(…)`
                // with the same target + range; skip to avoid a duplicate edge.
                ObjectCreationExpressionSyntax oce when oce.Type == node => true,
                UsingDirectiveSyntax => true,
                NameEqualsSyntax => true,
                AttributeSyntax => true,
                ParameterSyntax p when p.Identifier == node.Identifier => true,
                VariableDeclaratorSyntax vd when vd.Identifier == node.Identifier => true,
                _ => false,
            };

        private uint ResolveSource(SyntaxNode node)
        {
            for (var n = node.Parent; n is not null; n = n.Parent)
            {
                var sym = _model.GetDeclaredSymbol(n);
                if (sym is null) continue;
                if (SymbolFilter.IsLocalSymbol(sym)) continue;
                if (sym is IMethodSymbol or IPropertySymbol or IFieldSymbol or IEventSymbol or INamedTypeSymbol)
                {
                    return _owner.EnsureRefStub(sym.OriginalDefinition, _packageId);
                }
            }
            return 0;
        }

        private void EmitInstantiates(uint sourceId, INamedTypeSymbol type, Range? range)
        {
            if (!type.IsGenericType || type.IsUnboundGenericType) return;
            foreach (var arg in type.TypeArguments)
            {
                if (arg is INamedTypeSymbol named && named.TypeKind != TypeKind.TypeParameter)
                {
                    var argId = _owner.EnsureRefStub(named.OriginalDefinition, _packageId);
                    if (sourceId != argId)
                    {
                        EmitBodyEdge(EdgeKind.Instantiates, sourceId, argId, range);
                    }
                }
            }
        }

        /// <summary>
        /// Emit a `type_use` edge when the LHS of a member access is itself
        /// a type reference (e.g. `MSBuildLocator.IsRegistered` → type_use
        /// MSBuildLocator). Only handles the immediate LHS — nested
        /// `Outer.Inner.Member` is handled recursively because the inner
        /// `Outer.Inner` MemberAccess gets its own VisitMemberAccessExpression.
        /// </summary>
        private void EmitContainerTypeUse(ExpressionSyntax lhs, uint sourceId)
        {
            if (lhs is not IdentifierNameSyntax && lhs is not GenericNameSyntax) return;
            if (_model.GetSymbolInfo(lhs).Symbol is not INamedTypeSymbol type) return;
            var targetId = _owner.EnsureRefStub(type.OriginalDefinition, _packageId);
            if (sourceId == targetId) return;
            EmitBodyEdge(EdgeKind.TypeUse, sourceId, targetId, RangeUtil.FromSyntaxNode(lhs));
        }

        private static FieldOp ClassifyFieldOp(SyntaxNode node)
        {
            var cur = node;
            while (cur is not null)
            {
                switch (cur.Parent)
                {
                    case AssignmentExpressionSyntax a when a.Left == cur: return FieldOp.Write;
                    case PostfixUnaryExpressionSyntax: return FieldOp.Write;
                    case PrefixUnaryExpressionSyntax pre when
                        pre.OperatorToken.IsKind(SyntaxKind.PlusPlusToken)
                        || pre.OperatorToken.IsKind(SyntaxKind.MinusMinusToken):
                        return FieldOp.Write;
                    case ArgumentSyntax arg when
                        arg.RefKindKeyword.IsKind(SyntaxKind.OutKeyword)
                        || arg.RefKindKeyword.IsKind(SyntaxKind.RefKeyword):
                        return FieldOp.Write;
                    case MemberAccessExpressionSyntax:
                        cur = cur.Parent;
                        continue;
                }
                break;
            }
            return FieldOp.Read;
        }
    }
}
