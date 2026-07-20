using System.Text.Json;
using Kenn.Dotnet.Wire;
using Xunit;

namespace Kenn.Dotnet.Tests;

public class JsonlSinkConcurrencyTests
{
    /// <summary>
    /// The `meta` handshake must reach the consumer immediately, NOT wait for a
    /// byte/frame threshold.
    ///
    /// Real case: meta is written before `dotnet restore`, which on a real
    /// solution runs for minutes. With meta thresholded, the consumer polls an
    /// empty stream the whole time and cannot distinguish a working producer
    /// from a hung one — observed as 0 bytes of stdout while restore ground on.
    /// Thresholds here are set absurdly high so ONLY an unconditional flush can
    /// make the frame appear.
    /// </summary>
    [Fact]
    public void MetaFrameIsFlushedImmediately()
    {
        var stream = new MemoryStream();
        using var sink = JsonlSink.OpenStream(stream, flushBytes: 1 << 24, flushFrames: 1 << 20);

        sink.Write(new MetaFrame
        {
            ProjectRoot = "file:///ws",
            Tool = "kenn-dotnet",
            ToolVersion = "1.2.3",
            Language = "csharp",
            Ts = "2026-05-23T00:00:00.000Z",
        });

        var written = System.Text.Encoding.UTF8.GetString(stream.ToArray());
        Assert.Contains("\"type\":\"meta\"", written);
        Assert.Contains("1.2.3", written);
        Assert.EndsWith("\n", written);

        // A non-handshake frame must still respect the thresholds, or this
        // "fix" would degrade into an unbuffered writer and cost a syscall per
        // frame across hundreds of thousands of symbols.
        var before = stream.ToArray().Length;
        sink.Write(new PackageFrame { Id = 1, Name = "P", Version = "0.0.0" });
        Assert.Equal(before, stream.ToArray().Length);
    }

    /// <summary>
    /// Regression for the parallel-project-walks change: under concurrent
    /// `Write` calls from N workers, every output line must still be a
    /// valid JSON object. Without the internal lock around the
    /// serialize-and-newline sequence, two callers' bytes interleave
    /// inside one line and `JsonDocument.Parse` will fail on at least
    /// one of them.
    /// </summary>
    [Fact]
    public void ConcurrentWritesProduceValidJsonl()
    {
        const int writers = 16;
        const int framesPerWriter = 500;

        // ToArray() works on a disposed MemoryStream (it reads from the
        // underlying byte[]), which lets us capture output after JsonlSink
        // takes ownership and closes the stream on Dispose.
        var stream = new MemoryStream();
        using (var sink = JsonlSink.OpenStream(stream, flushBytes: 1 << 20, flushFrames: 1 << 16))
        {
            Parallel.For(0, writers, new ParallelOptions { MaxDegreeOfParallelism = writers }, w =>
            {
                for (var i = 0; i < framesPerWriter; i++)
                {
                    sink.Write(new PackageFrame
                    {
                        Id = (uint)(w * framesPerWriter + i + 1),
                        Name = $"writer-{w}-frame-{i}",
                        Version = "1.0.0",
                        Manager = "test",
                        External = false,
                    });
                }
            });
        }

        using var reader = new StreamReader(new MemoryStream(stream.ToArray()));

        var lineCount = 0;
        string? line;
        while ((line = reader.ReadLine()) is not null)
        {
            lineCount++;
            // Every line must parse as a single, complete JSON object.
            // Interleaved-bytes corruption would surface here as a
            // JsonException on one or more lines.
            using var doc = JsonDocument.Parse(line);
            Assert.Equal(JsonValueKind.Object, doc.RootElement.ValueKind);
            Assert.Equal("package", doc.RootElement.GetProperty("type").GetString());
        }

        Assert.Equal(writers * framesPerWriter, lineCount);
    }
}
