// Wire range: (start_line, start_col, end_line, end_col), 0-based.
// Named ValueTuple — no per-call allocation. Shadows System.Range, which we
// don't use anywhere in this project (slicing via `[a..b]` syntax doesn't
// reference the type by name).
global using Range = (int Sl, int Sc, int El, int Ec);
