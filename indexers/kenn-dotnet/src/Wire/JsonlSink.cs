using System.Buffers;
using System.Text.Json;

namespace Kenn.Dotnet.Wire;

/// <summary>
/// Buffered JSONL writer. Each <see cref="Write"/> serializes one
/// <see cref="Frame"/> as a single line of UTF-8 JSON (ending with `\n`)
/// into an internal buffer. The buffer is flushed to stdout when either
/// the byte or frame threshold is hit, on <see cref="Dispose"/>, and on
/// process exit.
/// </summary>
public sealed class JsonlSink : IDisposable
{
    private static readonly JsonWriterOptions WriterOpts =
        new() { Indented = false, SkipValidation = true };

    private readonly object _sync = new();
    private readonly Stream _stdout;
    private readonly int _flushBytes;
    private readonly int _flushFrames;
    private readonly ArrayBufferWriter<byte> _buf = new();
    private int _bufferedFrames;
    private bool _disposed;
    private readonly EventHandler _processExitHandler;

    // Per-thread serialization scratch space. Each parallel walker thread
    // serializes its frame into its own buffer (no lock), then takes the
    // shared lock briefly to append the resulting bytes to `_buf`. Shrinks
    // the critical section from "serialize + append + maybe-flush" to
    // "append + maybe-flush" — the serialization (the bulk of per-frame
    // work) runs concurrently across walker threads.
    [ThreadStatic]
    private static ArrayBufferWriter<byte>? _localBuf;
    [ThreadStatic]
    private static Utf8JsonWriter? _localWriter;

    private JsonlSink(Stream stdout, int flushBytes, int flushFrames)
    {
        _stdout = stdout;
        _flushBytes = flushBytes;
        _flushFrames = flushFrames;
        _processExitHandler = (_, _) => SafeFlush();
        AppDomain.CurrentDomain.ProcessExit += _processExitHandler;
    }

    public static JsonlSink OpenStdout(int flushBytes, int flushFrames) =>
        new(Console.OpenStandardOutput(), flushBytes, flushFrames);

    /// <summary>
    /// Open a sink writing to an arbitrary stream. The sink takes ownership
    /// of <paramref name="stream"/> and disposes it on <see cref="Dispose"/>.
    /// </summary>
    public static JsonlSink OpenStream(Stream stream, int flushBytes, int flushFrames) =>
        new(stream, flushBytes, flushFrames);

    public void Write(Frame frame)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);

        // Serialize into a per-thread buffer — no shared state, no lock.
        // This is the bulk of per-frame work (~1-5 µs); running it without
        // the global lock lets parallel walkers serialize concurrently.
        var localBuf = _localBuf ??= new ArrayBufferWriter<byte>(256);
        localBuf.ResetWrittenCount();
        var localWriter = _localWriter ??= new Utf8JsonWriter(localBuf, WriterOpts);
        frame.WriteTo(localWriter);
        localWriter.Flush();
        localWriter.Reset();
        localBuf.Write("\n"u8);

        // Append the serialized line to the shared buffer atomically. The
        // lock guarantees one frame per JSONL line on stdout (no
        // interleaving) and that the threshold check + flush see a
        // consistent buffer state.
        lock (_sync)
        {
            _buf.Write(localBuf.WrittenSpan);
            _bufferedFrames++;

            // `meta` is the protocol handshake — it names the tool, its version
            // and the project root, and the consumer reads `tool_version` from
            // it. Flush it unconditionally rather than leaving it to a byte or
            // frame threshold: this frame is written BEFORE `dotnet restore`,
            // which on a real solution runs for minutes, so a thresholded meta
            // leaves the consumer staring at an empty stream with no way to tell
            // a working producer from a hung one. One extra syscall, once.
            if (frame is MetaFrame
                || _buf.WrittenCount >= _flushBytes
                || _bufferedFrames >= _flushFrames)
            {
                FlushLocked();
            }
        }
    }

    public void Flush()
    {
        lock (_sync) { FlushLocked(); }
    }

    private void FlushLocked()
    {
        if (_buf.WrittenCount == 0)
        {
            return;
        }

        _stdout.Write(_buf.WrittenSpan);
        _stdout.Flush();

        _buf.ResetWrittenCount();
        _bufferedFrames = 0;
    }

    private void SafeFlush()
    {
        try { Flush(); } catch { /* shutdown path; nothing useful to do */ }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        AppDomain.CurrentDomain.ProcessExit -= _processExitHandler;
        try { Flush(); } catch { /* ignore */ }
        _stdout.Dispose();
        // Per-thread Utf8JsonWriter / ArrayBufferWriter instances are GC-
        // reclaimed when the owning thread exits; nothing to dispose here.
    }
}
