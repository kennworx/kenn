using System.Buffers.Binary;
using System.IO.Hashing;
using System.IO.MemoryMappedFiles;
using System.Runtime.InteropServices;
using Microsoft.CodeAnalysis;
using Kenn.Dotnet.Wire;

namespace Kenn.Dotnet.Indexing;

/// <summary>
/// Emits a FileFrame on first sight of each source path; subsequent
/// sightings return the cached id. Path is workspace-relative with forward
/// slashes; id comes from the shared <see cref="IdRegistry"/>.
/// </summary>
internal sealed class FileTracker
{
    private readonly object _sync = new();
    private readonly Dictionary<string, uint> _seen = new(StringComparer.Ordinal);
    private readonly HashSet<uint> _testFileIds = new();
    // Absolute paths of files in test projects (detected from MSBuild
    // references). A test project flags every file in it — fixtures,
    // `*TestHost.cs`, `*TestBase.cs` — not just `*Test.cs` file names.
    private readonly HashSet<string> _testProjectPaths = new(StringComparer.Ordinal);
    private readonly string _workspaceRoot;
    private readonly JsonlSink _sink;
    private readonly IdRegistry _ids;
    private readonly TestPathMatcher _testMatcher;

    public FileTracker(JsonlSink sink, string workspaceRoot, IdRegistry ids, TestPathMatcher testMatcher)
    {
        _sink = sink;
        _workspaceRoot = workspaceRoot;
        _ids = ids;
        _testMatcher = testMatcher;
    }

    public int Count
    {
        get { lock (_sync) { return _seen.Count; } }
    }

    /// <summary>
    /// Returns the file's int id; emits FileFrame on first sight.
    ///
    /// The lookup, id allocation, and frame emit are all inside one lock
    /// so two concurrent callers for the same path can't both emit a
    /// FileFrame. The hash computation runs inside the lock too — slightly
    /// reduces parallelism, but per-path I/O happens at most once anyway,
    /// and the alternative (compute outside, emit inside) opens a window
    /// where one caller advances `_seen` after another has already started
    /// hashing the same file.
    /// </summary>
    public uint RegisterIfNew(string? absolutePath, SyntaxTree? tree = null)
    {
        if (string.IsNullOrEmpty(absolutePath)) return 0;
        lock (_sync)
        {
            if (_seen.TryGetValue(absolutePath, out var existing)) return existing;

            var id = _ids.RegisterFile(absolutePath);
            _seen[absolutePath] = id;

            var rel = Path.GetRelativePath(_workspaceRoot, absolutePath).Replace('\\', '/');
            var isTest = _testMatcher.IsMatch(rel) || _testProjectPaths.Contains(absolutePath);
            if (isTest) _testFileIds.Add(id);
            var frame = new FileFrame
            {
                Id = id,
                Path = rel,
                Test = isTest,
                External = false,
                ContentHash = TryHashFile(absolutePath),
                // File-level comment trivia, extracted once on first
                // sight when the owning syntax tree is available.
                Doc = tree is null ? null : FileDoc.Extract(tree),
            };
            _sink.Write(frame);
            return id;
        }
    }

    /// <summary>
    /// Returns true when the given file id was registered for a path that
    /// matched <see cref="LooksLikeTest"/>. Used so that symbol emission
    /// can mirror the file-level test flag (without this, SymbolFrame.Test
    /// would always be false even for symbols defined in *Test.cs files).
    /// </summary>
    public bool IsTest(uint fileId)
    {
        if (fileId == 0) return false;
        lock (_sync) { return _testFileIds.Contains(fileId); }
    }

    /// <summary>
    /// Mark a test project's files: every file registered for one of these
    /// paths emits <c>test = true</c> regardless of the file-name globs.
    /// Called before the walk — kenn-dotnet identifies test projects from
    /// their MSBuild references (xunit / nunit / mstest / TestPlatform), so a
    /// project's fixtures and `*TestHost.cs`/`*TestBase.cs` count as test too.
    /// </summary>
    public void MarkTestProject(IEnumerable<string?> absolutePaths)
    {
        lock (_sync)
        {
            foreach (var p in absolutePaths)
            {
                if (!string.IsNullOrEmpty(p)) _testProjectPaths.Add(p);
            }
        }
    }

    /// <summary>
    /// Files larger than this skip mmap and fall back to a metadata-based
    /// hash (path + mtime + size). Real C# source files cap well below
    /// 1 MB; a few MB is enough for ANTLR-style generated parsers and
    /// embedded resources. Anything bigger is almost certainly checked-in
    /// data we don't index for content — opening it pollutes our virtual
    /// address space and wastes I/O.
    /// </summary>
    private const long MmapMaxBytes = 16L * 1024 * 1024;

    /// <summary>
    /// xxh64 of the file contents, zero-copy via memory mapping. The OS
    /// faults pages in on demand; we hand the mapped span straight to
    /// XxHash64 with no intermediate buffer allocation.
    ///
    /// Three fast-paths short-circuit before opening the file:
    ///   - missing / stat failure → sentinel zero hash,
    ///   - empty file → xxh64 of the empty span,
    ///   - oversized (&gt; <see cref="MmapMaxBytes"/>) → synthetic hash
    ///     over (absolute path, mtime ticks, length). The metadata is
    ///     enough for staleness detection: a real edit bumps mtime, and
    ///     we never need to read those bytes for indexing anyway.
    /// </summary>
    private static unsafe string TryHashFile(string path)
    {
        try
        {
            var fi = new FileInfo(path);
            if (!fi.Exists || fi.Length == 0) return "0000000000000000";
            var len = fi.Length;
            if (len > MmapMaxBytes) return MetadataHash(path, fi.LastWriteTimeUtc, len);

            using var mmf = MemoryMappedFile.CreateFromFile(
                path, FileMode.Open, mapName: null, capacity: 0, MemoryMappedFileAccess.Read);
            using var accessor = mmf.CreateViewAccessor(0, 0, MemoryMappedFileAccess.Read);

            byte* ptr = null;
            accessor.SafeMemoryMappedViewHandle.AcquirePointer(ref ptr);
            try
            {
                var x = new XxHash64();
                x.Append(new ReadOnlySpan<byte>(ptr, (int)len));
                return FormatXxh64(x);
            }
            finally
            {
                accessor.SafeMemoryMappedViewHandle.ReleasePointer();
            }
        }
        catch
        {
            return "0000000000000000";
        }
    }

    /// <summary>
    /// Synthetic content-hash for oversized files: xxh64 of
    /// `path` (UTF-16 code units) || `mtime.Ticks` (le i64) || `length` (le i64).
    /// Stable across runs as long as the file isn't touched; changes if any
    /// of (path, mtime, size) changes.
    /// </summary>
    private static string MetadataHash(string path, DateTime mtimeUtc, long length)
    {
        var x = new XxHash64();
        // Path bytes via MemoryMarshal — no allocation. UTF-16 form is
        // fine: this hash is only ever produced and consumed by us.
        x.Append(MemoryMarshal.AsBytes(path.AsSpan()));
        Span<byte> stamp = stackalloc byte[16];
        BinaryPrimitives.WriteInt64LittleEndian(stamp[..8], mtimeUtc.Ticks);
        BinaryPrimitives.WriteInt64LittleEndian(stamp[8..], length);
        x.Append(stamp);
        return FormatXxh64(x);
    }

    private static string FormatXxh64(XxHash64 x)
    {
        Span<byte> hash = stackalloc byte[8];
        x.GetCurrentHash(hash);
        return BinaryPrimitives.ReadUInt64BigEndian(hash).ToString("x16");
    }

}
