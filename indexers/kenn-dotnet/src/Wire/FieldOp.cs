namespace Kenn.Dotnet.Wire;

/// <summary>
/// Wire <c>field_op</c> value on <c>EdgeFrame</c> when
/// <c>edge_kind = "field_access"</c>. Mirrors <c>indexers/frames.ts</c>.
/// </summary>
public enum FieldOp
{
    Read,
    Write,
}

public static class FieldOpExtensions
{
    public static string ToWireString(this FieldOp op) => op switch
    {
        FieldOp.Read => "read",
        FieldOp.Write => "write",
        _ => "read",
    };
}
