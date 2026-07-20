using System.Text;
using Microsoft.CodeAnalysis;

namespace Kenn.Dotnet.Indexing;

/// <summary>
/// Producer-internal id allocator and dedup table.
///
/// Symbols are deduped across compilations by an internal stable key
/// (built via <see cref="PubId"/>). The key shape is language-naked and
/// intra-package — the wire `key` field carries it verbatim and the
/// consumer assembles `pub_id` as `lang_prefix:key` from
/// `MetaFrame.language`.
///
/// Files are deduped by absolute path. Packages are deduped by
/// `(name, version)` via a separate intern table.
///
/// Thread-safety: every public mutator and reader is guarded by a single
/// internal lock. Callers may invoke from multiple worker tasks. Critical
/// sections are short (dictionary lookup + insert), so a single lock
/// outperforms reader/writer-asymmetric primitives at this workload.
/// </summary>
internal sealed class IdRegistry
{
    private readonly object _sync = new();
    private uint _next;
    private readonly Dictionary<string, uint> _symbolKey = new(StringComparer.Ordinal);
    private readonly Dictionary<string, uint> _filePath = new(StringComparer.Ordinal);
    /// <summary>
    /// `(name, version)` → package wire id. Multi-target compilations of
    /// the same library map to one PackageFrame.
    /// </summary>
    private readonly Dictionary<(string Name, string Version), uint> _packageKey = new();
    private readonly HashSet<uint> _fullyEmitted = new();

    /// <summary>Allocate a fresh id (e.g. for synthetic root packages).</summary>
    public uint Allocate()
    {
        lock (_sync)
        {
            _next += 1;
            return _next;
        }
    }

    public bool TryGetSymbol(string key, out uint id)
    {
        lock (_sync) { return _symbolKey.TryGetValue(key, out id); }
    }

    public uint RegisterSymbol(string key)
    {
        lock (_sync)
        {
            if (_symbolKey.TryGetValue(key, out var existing)) return existing;
            _next += 1;
            _symbolKey[key] = _next;
            return _next;
        }
    }

    /// <summary>
    /// Atomic register-or-get that reports whether THIS caller is responsible
    /// for emitting the corresponding wire frame. Mirrors
    /// <see cref="RegisterPackage"/>; required by <see cref="EnsureRefStub"/>
    /// so two concurrent walkers encountering the same cross-project symbol
    /// don't both emit a `StubFrame` for the same id.
    /// </summary>
    public (uint Id, bool IsNew) RegisterSymbolIfNew(string key)
    {
        lock (_sync)
        {
            if (_symbolKey.TryGetValue(key, out var existing)) return (existing, false);
            _next += 1;
            _symbolKey[key] = _next;
            return (_next, true);
        }
    }

    public bool TryGetFile(string absolutePath, out uint id)
    {
        lock (_sync) { return _filePath.TryGetValue(absolutePath, out id); }
    }

    public uint RegisterFile(string absolutePath)
    {
        lock (_sync)
        {
            if (_filePath.TryGetValue(absolutePath, out var existing)) return existing;
            _next += 1;
            _filePath[absolutePath] = _next;
            return _next;
        }
    }

    /// <summary>
    /// Get-or-allocate a package wire id keyed by `(name, version)`.
    /// Returns `(id, isNew)` so the caller knows whether to emit a
    /// PackageFrame for this id.
    /// </summary>
    public (uint Id, bool IsNew) RegisterPackage(string name, string version)
    {
        lock (_sync)
        {
            var key = (name, version);
            if (_packageKey.TryGetValue(key, out var existing)) return (existing, false);
            _next += 1;
            _packageKey[key] = _next;
            return (_next, true);
        }
    }

    /// <summary>
    /// Atomic test-and-set on the "this id has been emitted as a full
    /// SymbolFrame" bit. Returns `true` exactly once per id, regardless of
    /// concurrent callers. Lets <see cref="IndexerCore.EmitFullSymbol"/>
    /// keep the SymbolFrame emit + counter increment in lockstep when two
    /// workers walk a shared namespace (namespaces dedup cross-package).
    /// </summary>
    public bool MarkFullyEmittedIfFirst(uint id)
    {
        lock (_sync) { return _fullyEmitted.Add(id); }
    }

    /// <summary>
    /// Build the internal dedup key for an arbitrary ISymbol.
    ///
    /// Format is language-naked and intra-package:
    ///   Type       : &lt;Namespace&gt;.&lt;Type&gt;[`N]
    ///   Member     : &lt;Namespace&gt;.&lt;Type&gt;#&lt;Member&gt;[(sig)]
    ///   Namespace  : &lt;NsPath&gt; (cross-assembly — one logical thing)
    ///
    /// For namespaces, dedup is cross-package: `System.Collections` is
    /// one row regardless of which assembly references it. For types and
    /// members, the producer's caller salts the key with the assembly's
    /// package wire id at call time when multiple assemblies legitimately
    /// declare the same path (rare; multi-version transitive deps).
    /// </summary>
    public static string Key(StringBuilder buf, ISymbol sym) => sym switch
    {
        INamespaceSymbol ns => ns.IsGlobalNamespace
            ? "<global>"
            : PubId.ForNamespace(buf, ns) ?? "<global>",
        INamedTypeSymbol t => PubId.ForType(buf, t),
        IMethodSymbol or IFieldSymbol or IPropertySymbol or IEventSymbol
            => PubId.ForMember(buf, sym),
        _ => sym.ToDisplayString(),
    };

    /// <summary>
    /// Per-package salt for cross-package symbols that are NOT namespaces.
    /// Namespaces are intentionally cross-package (a single logical entity);
    /// types and members are package-scoped and the dedup key includes the
    /// package id so two packages independently declaring the same path
    /// (e.g. `Newtonsoft.Json.JsonConvert` v12 and v13) get distinct ids.
    /// </summary>
    public static string KeyForRegister(StringBuilder buf, ISymbol sym, uint pkgId)
    {
        var bare = Key(buf, sym);
        if (sym is INamespaceSymbol) return bare;
        return pkgId == 0 ? bare : $"{bare}@{pkgId}";
    }
}
