using System.Text.Json;

namespace Kenn.Dotnet.Wire;

/// <summary>
/// JSONL frame envelope. Each frame writes itself directly via
/// <see cref="WriteTo"/> using <see cref="Utf8JsonWriter"/> — no reflection,
/// no polymorphic JsonSerializer, trim-safe by construction. The `type`
/// discriminator is emitted first by the base class.
/// </summary>
public abstract class Frame
{
    protected abstract string TypeName { get; }

    public void WriteTo(Utf8JsonWriter w)
    {
        w.WriteStartObject();
        w.WriteString("type", TypeName);
        WriteFields(w);
        w.WriteEndObject();
    }

    protected abstract void WriteFields(Utf8JsonWriter w);

    protected static void WriteRange(Utf8JsonWriter w, string name, Range r)
    {
        w.WriteStartArray(name);
        w.WriteNumberValue(r.Sl);
        w.WriteNumberValue(r.Sc);
        w.WriteNumberValue(r.El);
        w.WriteNumberValue(r.Ec);
        w.WriteEndArray();
    }
}

public sealed class MetaFrame : Frame
{
    protected override string TypeName => "meta";
    public int Version { get; init; } = 1;
    public required string ProjectRoot { get; init; }
    public required string Tool { get; init; }
    public required string ToolVersion { get; init; }
    public required string Language { get; init; }
    /// <summary>ISO 8601 UTC timestamp when the producer wrote this frame
    /// (millisecond precision, `YYYY-MM-DDTHH:mm:ss.sssZ`).</summary>
    public string Ts { get; init; } = FrameTimestamps.IsoNow();

    protected override void WriteFields(Utf8JsonWriter w)
    {
        w.WriteNumber("v", Version);
        w.WriteString("project_root", ProjectRoot);
        w.WriteString("tool", Tool);
        w.WriteString("tool_version", ToolVersion);
        w.WriteString("language", Language);
        w.WriteString("ts", Ts);
    }
}

internal static class FrameTimestamps
{
    /// <summary>UTC ISO 8601 with millisecond precision and `Z` suffix.</summary>
    public static string IsoNow() =>
        DateTimeOffset.UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", System.Globalization.CultureInfo.InvariantCulture);
}

public sealed class FileFrame : Frame
{
    protected override string TypeName => "file";
    public required uint Id { get; init; }
    public required string Path { get; init; }
    public required string ContentHash { get; init; }
    public bool Test { get; init; }
    public bool External { get; init; }
    /// <summary>
    /// File-level comment trivia, one entry per comment token (file
    /// header + each namespace-leading comment). Null/empty when the
    /// file has none. License-boilerplate filtering happens on the
    /// consumer, not here.
    /// </summary>
    public string[]? Doc { get; init; }

    protected override void WriteFields(Utf8JsonWriter w)
    {
        w.WriteNumber("id", Id);
        w.WriteString("path", Path);
        w.WriteString("content_hash", ContentHash);
        if (Test) w.WriteBoolean("test", true);
        if (External) w.WriteBoolean("external", true);
        if (Doc is { Length: > 0 })
        {
            w.WriteStartArray("doc");
            foreach (var d in Doc) w.WriteStringValue(d);
            w.WriteEndArray();
        }
    }
}

/// <summary>
/// Package metadata. One frame per logical (name, version) package.
/// Producers MUST intern producer-side by (name, version) so multi-target
/// compilations of the same package emit one frame.
/// </summary>
public sealed class PackageFrame : Frame
{
    protected override string TypeName => "package";
    public required uint Id { get; init; }
    public required string Name { get; init; }
    public string? Version { get; init; }
    public string? Manager { get; init; }
    public bool External { get; init; }

    protected override void WriteFields(Utf8JsonWriter w)
    {
        w.WriteNumber("id", Id);
        w.WriteString("name", Name);
        if (Version is not null) w.WriteString("version", Version);
        if (Manager is not null) w.WriteString("manager", Manager);
        if (External) w.WriteBoolean("external", true);
    }
}

/// <summary>
/// Forward-ref / external-symbol stub. Producers MUST use the same `id`
/// across the stub and any subsequent <see cref="SymbolFrame"/> upgrade.
/// External symbols emit exactly one StubFrame and no follow-up.
/// </summary>
public sealed class StubFrame : Frame
{
    protected override string TypeName => "stub";
    public required uint Id { get; init; }
    public required SymKind Kind { get; init; }
    public required string Name { get; init; }
    public required string Key { get; init; }
    public uint Package { get; init; }

    protected override void WriteFields(Utf8JsonWriter w)
    {
        w.WriteNumber("id", Id);
        w.WriteString("kind", Kind.ToWireString());
        w.WriteString("name", Name);
        w.WriteString("key", Key);
        if (Package != 0) w.WriteNumber("pkg", Package);
    }
}

/// <summary>
/// Full symbol declaration. Producers MUST use <see cref="StubFrame"/>
/// instead when only partial info is available.
/// </summary>
public sealed class SymbolFrame : Frame
{
    protected override string TypeName => "symbol";
    public required uint Id { get; init; }
    public uint Package { get; init; }
    /// <summary>
    /// Cross-run-stable, language-naked, intra-package descriptor
    /// (e.g. `Models.Order#Save(int)`). The consumer assembles `pub_id`
    /// as `<lang_prefix>:<key>` from `MetaFrame.language`.
    /// </summary>
    public required string Key { get; init; }
    public required SymKind Kind { get; init; }
    public required string Name { get; init; }
    public uint Parent { get; init; }
    public uint File { get; init; }
    public required Range Range { get; init; }
    /// <summary>
    /// Full declaration span (attribute lists + member body), distinct from
    /// <see cref="Range"/> (the name-identifier span). Null (field omitted)
    /// for metadata-only symbols with no declaring syntax.
    /// </summary>
    public Range? Body { get; init; }
    public bool Partial { get; init; }
    public int Nargs { get; init; }
    public int Targs { get; init; }
    public bool Test { get; init; }
    /// <summary>Bare signature text (no code fence).</summary>
    public string? Sig { get; init; }
    public string? Doc { get; init; }

    protected override void WriteFields(Utf8JsonWriter w)
    {
        w.WriteNumber("id", Id);
        if (Package != 0) w.WriteNumber("pkg", Package);
        w.WriteString("key", Key);
        w.WriteString("kind", Kind.ToWireString());
        w.WriteString("name", Name);
        if (Parent != 0) w.WriteNumber("parent", Parent);
        if (File != 0) w.WriteNumber("file", File);
        WriteRange(w, "range", Range);
        if (Body is { } b) WriteRange(w, "body", b);
        if (Partial) w.WriteBoolean("partial", true);
        if (Nargs != 0) w.WriteNumber("nargs", Nargs);
        if (Targs != 0) w.WriteNumber("targs", Targs);
        if (Test) w.WriteBoolean("test", true);
        if (Sig is not null) w.WriteString("sig", Sig);
        if (Doc is not null) w.WriteString("doc", Doc);
    }
}

public sealed class EdgeFrame : Frame
{
    protected override string TypeName => "edge";
    public required EdgeKind EdgeKind { get; init; }
    public required uint Source { get; init; }
    public required uint Target { get; init; }
    public Range? Range { get; init; }
    public FieldOp? FieldOp { get; init; }

    protected override void WriteFields(Utf8JsonWriter w)
    {
        w.WriteString("edge_kind", EdgeKind.ToWireString());
        w.WriteNumber("source", Source);
        w.WriteNumber("target", Target);
        if (Range is { } r) WriteRange(w, "range", r);
        if (FieldOp is { } op) w.WriteString("field_op", op.ToWireString());
    }
}

public sealed class ErrorFrame : Frame
{
    protected override string TypeName => "error";
    public required string Severity { get; init; }
    public required string Source { get; init; }
    public required string Message { get; init; }
    public string? Path { get; init; }
    public Range? Range { get; init; }
    public string? Code { get; init; }

    protected override void WriteFields(Utf8JsonWriter w)
    {
        w.WriteString("severity", Severity);
        w.WriteString("source", Source);
        w.WriteString("message", Message);
        if (Path is not null) w.WriteString("path", Path);
        if (Range is { } r) WriteRange(w, "range", r);
        if (Code is not null) w.WriteString("code", Code);
    }
}

public sealed class EndFrame : Frame
{
    protected override string TypeName => "end";
    public required EndStats Stats { get; init; }
    /// <summary>ISO 8601 UTC timestamp when the producer wrote this frame
    /// (millisecond precision, `YYYY-MM-DDTHH:mm:ss.sssZ`).</summary>
    public string Ts { get; init; } = FrameTimestamps.IsoNow();

    protected override void WriteFields(Utf8JsonWriter w)
    {
        w.WriteStartObject("stats");
        w.WriteNumber("files", Stats.Files);
        w.WriteNumber("symbols", Stats.Symbols);
        w.WriteNumber("edges", Stats.Edges);
        w.WriteNumber("errors", Stats.Errors);
        w.WriteEndObject();
        w.WriteString("ts", Ts);
    }
}

public sealed class EndStats
{
    public long Files { get; init; }
    public long Symbols { get; init; }
    public long Edges { get; init; }
    public long Errors { get; init; }
}
