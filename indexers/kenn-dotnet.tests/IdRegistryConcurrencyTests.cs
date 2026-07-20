using Kenn.Dotnet.Indexing;
using Xunit;

namespace Kenn.Dotnet.Tests;

public class IdRegistryConcurrencyTests
{
    /// <summary>
    /// `RegisterSymbolIfNew` must report `IsNew = true` exactly once per
    /// key, even under heavy contention. Two concurrent walkers must not
    /// both end up emitting a stub for the same id.
    /// </summary>
    [Fact]
    public void RegisterSymbolIfNewIsAtomic()
    {
        var reg = new IdRegistry();
        const int threads = 32;
        const int keys = 200;

        var newCounts = new int[keys];

        Parallel.For(0, threads, new ParallelOptions { MaxDegreeOfParallelism = threads }, _ =>
        {
            for (var k = 0; k < keys; k++)
            {
                var (_, isNew) = reg.RegisterSymbolIfNew($"key-{k}");
                if (isNew) Interlocked.Increment(ref newCounts[k]);
            }
        });

        for (var k = 0; k < keys; k++)
        {
            Assert.Equal(1, newCounts[k]);
        }
    }

    /// <summary>
    /// `MarkFullyEmittedIfFirst` returns `true` exactly once per id even
    /// when invoked from many workers simultaneously. Without this,
    /// shared-namespace SymbolFrames would be emitted twice and the
    /// `_symbolFullCount` counter would over-count.
    /// </summary>
    [Fact]
    public void MarkFullyEmittedIfFirstIsAtomic()
    {
        var reg = new IdRegistry();
        const int threads = 32;
        const int ids = 200;

        var firstCounts = new int[ids];

        Parallel.For(0, threads, new ParallelOptions { MaxDegreeOfParallelism = threads }, _ =>
        {
            for (var i = 0; i < ids; i++)
            {
                if (reg.MarkFullyEmittedIfFirst((uint)(i + 1)))
                    Interlocked.Increment(ref firstCounts[i]);
            }
        });

        for (var i = 0; i < ids; i++)
        {
            Assert.Equal(1, firstCounts[i]);
        }
    }

    /// <summary>
    /// `RegisterSymbolIfNew` returns the *same* id for the same key across
    /// concurrent callers — the lock makes lookup + allocation atomic so
    /// no two callers can both observe "missing" and both allocate.
    /// </summary>
    [Fact]
    public void RegisterSymbolIfNewReturnsSameIdForSameKey()
    {
        var reg = new IdRegistry();
        const int threads = 16;

        var ids = new uint[threads];

        Parallel.For(0, threads, t =>
        {
            var (id, _) = reg.RegisterSymbolIfNew("shared-key");
            ids[t] = id;
        });

        var first = ids[0];
        Assert.NotEqual(0u, first);
        Assert.All(ids, id => Assert.Equal(first, id));
    }
}
