namespace Kenn.Dotnet.Wire;

/// <summary>
/// Wire <c>edge_kind</c> taxonomy. Mirrors <c>indexers/frames.ts</c>.
/// </summary>
public enum EdgeKind
{
    DefinedIn,
    Contains,
    Calls,
    TypeUse,
    FieldAccess,
    Implements,
    Overrides,
    Instantiates,
    GenericConstraint,
    Imports,
    CorrespondsTo,
    ExtendsType,
}

public static class EdgeKindExtensions
{
    public static string ToWireString(this EdgeKind k) => k switch
    {
        EdgeKind.DefinedIn => "defined_in",
        EdgeKind.Contains => "contains",
        EdgeKind.Calls => "calls",
        EdgeKind.TypeUse => "type_use",
        EdgeKind.FieldAccess => "field_access",
        EdgeKind.Implements => "implements",
        EdgeKind.Overrides => "overrides",
        EdgeKind.Instantiates => "instantiates",
        EdgeKind.GenericConstraint => "generic_constraint",
        EdgeKind.Imports => "imports",
        EdgeKind.CorrespondsTo => "corresponds_to",
        EdgeKind.ExtendsType => "extends_type",
        _ => "defined_in",
    };

    /// <summary>
    /// Parse the wire snake_case string back to <see cref="EdgeKind"/>.
    /// Returns null for unknown values; used by the CLI's
    /// <c>--edge-kinds</c> allowlist parser.
    /// </summary>
    public static EdgeKind? TryParseWireString(string s) => s switch
    {
        "defined_in" => EdgeKind.DefinedIn,
        "contains" => EdgeKind.Contains,
        "calls" => EdgeKind.Calls,
        "type_use" => EdgeKind.TypeUse,
        "field_access" => EdgeKind.FieldAccess,
        "implements" => EdgeKind.Implements,
        "overrides" => EdgeKind.Overrides,
        "instantiates" => EdgeKind.Instantiates,
        "generic_constraint" => EdgeKind.GenericConstraint,
        "imports" => EdgeKind.Imports,
        "corresponds_to" => EdgeKind.CorrespondsTo,
        "extends_type" => EdgeKind.ExtendsType,
        _ => null,
    };
}
