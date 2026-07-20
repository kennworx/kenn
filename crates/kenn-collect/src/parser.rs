//! Bash command parser built on `brush-parser`.
//!
//! Walks the parsed AST to collect output destinations (redirects, `tee`
//! invocations), applying a pure variable-expansion pass over each redirect
//! target word. Ported from periClaude's `parser.rs` with two kenn changes:
//!
//! - **D4** — expansion consults `std::env` as a fallback *below* in-command
//!   assignments. The assignment map is seeded from the ambient environment;
//!   in-command assignments override it, preserving in-command precedence.
//! - **Absolutization** — resolved targets are made absolute against a base
//!   directory (`CLAUDE_PROJECT_DIR` → cwd, supplied by the caller). Unresolved
//!   targets keep `resolved=false` and their literal text.

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use brush_parser::ast::{
    AssignmentName, AssignmentValue, Command, CommandPrefixOrSuffixItem, CompoundCommand,
    CompoundListItem, IoFileRedirectKind, IoFileRedirectTarget, IoRedirect, Pipeline, Program,
    SimpleCommand, Word,
};
use brush_parser::word::{Parameter, ParameterExpr, WordPiece};
use brush_parser::ParserOptions;

use crate::store::{Output, OutputKind};

/// Wraps errors from the underlying `brush-parser`. We only surface a string
/// because brush's `ParseError` is `miette`-flavoured and not particularly
/// useful to programmatic callers — the hook layer just records the command
/// with zero outputs and moves on.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("brush-parser failed: {0}")]
    Brush(String),
}

/// Result of parsing a Bash command string.
#[derive(Debug, Clone)]
pub struct ParsedCommand {
    pub outputs: Vec<Output>,
    pub signature: Option<String>,
}

/// Parse a Bash command string into outputs + signature. Never panics — a
/// malformed input yields `Err(ParseError::Brush(...))`.
///
/// `base` is the directory resolved targets are absolutized against (the
/// command's cwd, or `CLAUDE_PROJECT_DIR`). Unresolved targets keep their
/// literal text untouched.
pub fn parse(cmd: &str, base: &Path) -> Result<ParsedCommand, ParseError> {
    let opts = ParserOptions::default();
    let mut parser = brush_parser::Parser::new(BufReader::new(cmd.as_bytes()), &opts);
    let program = parser
        .parse_program()
        .map_err(|e| ParseError::Brush(e.to_string()))?;

    // D4: seed the assignment map from the ambient environment. In-command
    // assignments inserted during the walk override these, preserving
    // in-command precedence.
    let mut assignments: HashMap<String, String> = std::env::vars().collect();
    let mut outputs: Vec<Output> = Vec::new();
    walk_program(&program, &opts, &mut assignments, &mut outputs);

    for o in &mut outputs {
        if o.resolved {
            o.path = absolutize(&o.path, base);
        }
    }

    let sig = signature_from_program(&program, &opts);
    Ok(ParsedCommand {
        outputs,
        signature: sig,
    })
}

// ---------------------------------------------------------------------------
// AST walk
// ---------------------------------------------------------------------------

fn walk_program(
    program: &Program,
    opts: &ParserOptions,
    assignments: &mut HashMap<String, String>,
    outputs: &mut Vec<Output>,
) {
    for complete in &program.complete_commands {
        for item in &complete.0 {
            let CompoundListItem(and_or_list, _sep) = item;
            for (_op, pipeline) in and_or_list {
                walk_pipeline(pipeline, opts, assignments, outputs);
            }
        }
    }
}

fn walk_pipeline(
    pipeline: &Pipeline,
    opts: &ParserOptions,
    assignments: &mut HashMap<String, String>,
    outputs: &mut Vec<Output>,
) {
    for cmd in &pipeline.seq {
        walk_command(cmd, opts, assignments, outputs);
    }
}

fn walk_command(
    cmd: &Command,
    opts: &ParserOptions,
    assignments: &mut HashMap<String, String>,
    outputs: &mut Vec<Output>,
) {
    match cmd {
        Command::Simple(s) => walk_simple(s, opts, assignments, outputs),
        Command::Compound(_cc, redirects) => {
            // Compound commands can carry their own redirect list. Their
            // inner bodies (subshells, if/while/...) may also contain simple
            // commands; recurse so we don't miss redirects inside.
            if let Some(rl) = redirects {
                for io in &rl.0 {
                    if let Some(o) = output_from_redirect(io, opts, assignments) {
                        outputs.push(o);
                    }
                }
            }
            walk_compound(cmd, opts, assignments, outputs);
        }
        Command::Function(_) | Command::ExtendedTest(_, _) => {
            // Function defs and `[[ ... ]]` tests don't produce log files we
            // care about.
        }
    }
}

fn walk_compound(
    cmd: &Command,
    opts: &ParserOptions,
    assignments: &mut HashMap<String, String>,
    outputs: &mut Vec<Output>,
) {
    let Command::Compound(cc, _) = cmd else {
        return;
    };
    // Only brace-groups and subshells contain simple commands whose redirects
    // we surface. Other forms (if/while/for/case) are best-effort skipped — the
    // 95% pattern is top-level simple commands and pipelines.
    let list = match cc {
        CompoundCommand::BraceGroup(b) => &b.list,
        CompoundCommand::Subshell(s) => &s.list,
        _ => return,
    };
    for item in &list.0 {
        let CompoundListItem(aol, _) = item;
        for (_op, p) in aol {
            walk_pipeline(p, opts, assignments, outputs);
        }
    }
}

/// Walk a simple command's prefix: record redirect outputs and collect
/// assignment words (which also seed `local` for later expansion in this
/// command). Returns the prefix assignments in source order.
fn collect_prefix(
    sc: &SimpleCommand,
    opts: &ParserOptions,
    local: &mut HashMap<String, String>,
    outputs: &mut Vec<Output>,
) -> Vec<(String, String)> {
    let mut prefix_assigns: Vec<(String, String)> = Vec::new();
    let Some(prefix) = &sc.prefix else {
        return prefix_assigns;
    };
    for item in &prefix.0 {
        match item {
            CommandPrefixOrSuffixItem::AssignmentWord(a, w) => {
                let AssignmentName::VariableName(name) = &a.name else {
                    continue;
                };
                let val = match &a.value {
                    AssignmentValue::Scalar(sw) => {
                        expand_word(sw, opts, local).unwrap_or_else(|| sw.value.clone())
                    }
                    AssignmentValue::Array(_) => w.value.clone(),
                };
                local.insert(name.clone(), val.clone());
                prefix_assigns.push((name.clone(), val));
            }
            CommandPrefixOrSuffixItem::IoRedirect(io) => {
                if let Some(o) = output_from_redirect(io, opts, local) {
                    outputs.push(o);
                }
            }
            CommandPrefixOrSuffixItem::Word(_)
            | CommandPrefixOrSuffixItem::ProcessSubstitution(_, _) => {}
        }
    }
    prefix_assigns
}

/// One word in a `tee` argv tail: skip flags (`-a`, `--`, …), expand the rest.
/// Returns the resolved target if `w` is a tee output destination.
fn tee_word_target(
    w: &Word,
    opts: &ParserOptions,
    local: &HashMap<String, String>,
    flags_done: &mut bool,
) -> Option<(Word, String, bool)> {
    // Expand; on failure fall back to the literal so the agent still sees what
    // they typed.
    let (text, resolved) = match expand_word(w, opts, local) {
        Some(t) => (t, true),
        None => (w.value.clone(), false),
    };
    if !*flags_done && text == "--" {
        *flags_done = true;
        return None;
    }
    if !*flags_done && text.starts_with('-') {
        return None; // tee flags: -a, --append, -i, --ignore-interrupts ...
    }
    Some((w.clone(), text, resolved))
}

/// Walk a simple command's suffix: record redirect outputs and (when the
/// command is `tee`) the tee target words.
fn collect_suffix(
    sc: &SimpleCommand,
    opts: &ParserOptions,
    local: &HashMap<String, String>,
    is_tee: bool,
    outputs: &mut Vec<Output>,
    tee_targets: &mut Vec<(Word, String, bool)>,
) {
    let Some(suffix) = &sc.suffix else { return };
    let mut flags_done = false;
    for item in &suffix.0 {
        match item {
            CommandPrefixOrSuffixItem::IoRedirect(io) => {
                if let Some(o) = output_from_redirect(io, opts, local) {
                    outputs.push(o);
                }
            }
            CommandPrefixOrSuffixItem::Word(w) if is_tee => {
                if let Some(t) = tee_word_target(w, opts, local, &mut flags_done) {
                    tee_targets.push(t);
                }
            }
            CommandPrefixOrSuffixItem::Word(_)
            | CommandPrefixOrSuffixItem::AssignmentWord(_, _)
            | CommandPrefixOrSuffixItem::ProcessSubstitution(_, _) => {}
        }
    }
}

fn walk_simple(
    sc: &SimpleCommand,
    opts: &ParserOptions,
    assignments: &mut HashMap<String, String>,
    outputs: &mut Vec<Output>,
) {
    // Per-command assignment view: standalone assignments from *earlier*
    // commands (and the ambient env seed) plus prefix assignments on *this*
    // command.
    let mut local = assignments.clone();
    let prefix_assigns = collect_prefix(sc, opts, &mut local, outputs);

    // Standalone assignment statement (no `word_or_name`, prefix-only): commit
    // the prefix assignments into the outer scope.
    if sc.word_or_name.is_none() && !prefix_assigns.is_empty() {
        for (k, v) in prefix_assigns {
            assignments.insert(k, v);
        }
        return;
    }

    let argv0 = sc
        .word_or_name
        .as_ref()
        .and_then(|w| expand_word(w, opts, &local));
    let is_tee = argv0.as_deref() == Some("tee");

    let mut tee_targets: Vec<(Word, String, bool)> = Vec::new();
    collect_suffix(sc, opts, &local, is_tee, outputs, &mut tee_targets);

    for (w, path, resolved) in tee_targets {
        if is_uninteresting_sink(&path) {
            continue;
        }
        outputs.push(Output {
            path,
            kind: OutputKind::Tee,
            fd: None,
            op: None,
            resolved,
            literal_text: w.value,
        });
    }
    // Prefix assignments scope to this command only; they are NOT committed to
    // the outer `assignments` map (the standalone-assignment branch above
    // handles the commit case).
}

// ---------------------------------------------------------------------------
// Redirect → Output
// ---------------------------------------------------------------------------

/// Sinks that are syntactically valid redirect targets but never useful to
/// surface to the user — they're not log files anyone tails.
fn is_uninteresting_sink(path: &str) -> bool {
    matches!(
        path,
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "/dev/zero"
    ) || path.starts_with("/dev/fd/")
}

fn output_from_redirect(
    io: &IoRedirect,
    opts: &ParserOptions,
    assignments: &HashMap<String, String>,
) -> Option<Output> {
    let out = match io {
        IoRedirect::File(fd, kind, target) => {
            let op = match kind {
                IoFileRedirectKind::Write => ">",
                IoFileRedirectKind::Append => ">>",
                IoFileRedirectKind::Clobber => ">|",
                // Reads and duplicates don't produce a log file we want to
                // track.
                IoFileRedirectKind::Read
                | IoFileRedirectKind::ReadAndWrite
                | IoFileRedirectKind::DuplicateInput
                | IoFileRedirectKind::DuplicateOutput => return None,
            };
            let word = match target {
                IoFileRedirectTarget::Filename(w) => w,
                IoFileRedirectTarget::Fd(_)
                | IoFileRedirectTarget::ProcessSubstitution(_, _)
                | IoFileRedirectTarget::Duplicate(_) => return None,
            };
            let resolved = expand_word(word, opts, assignments);
            let (path, is_resolved) = match resolved {
                Some(p) => (p, true),
                None => (word.value.clone(), false),
            };
            // Default fd: stdout for write/append/clobber when caller omits.
            let fd_num = fd.or(Some(1));
            Some(Output {
                path,
                kind: OutputKind::Redirect,
                fd: fd_num,
                op: Some(op.to_string()),
                resolved: is_resolved,
                literal_text: word.value.clone(),
            })
        }
        IoRedirect::HereDocument(_fd, _hd) => {
            // Heredocs are stdin sources, not output destinations. The
            // file the surrounding command writes to (if any) is captured by
            // the sibling `IoRedirect::File` on the same SimpleCommand.
            None
        }
        IoRedirect::OutputAndError(word, append) => {
            let op = if *append { "&>>" } else { "&>" };
            let resolved = expand_word(word, opts, assignments);
            let (path, is_resolved) = match resolved {
                Some(p) => (p, true),
                None => (word.value.clone(), false),
            };
            Some(Output {
                path,
                kind: OutputKind::Redirect,
                fd: None,
                op: Some(op.to_string()),
                resolved: is_resolved,
                literal_text: word.value.clone(),
            })
        }
        IoRedirect::HereString(_, _) => None,
    };
    out.filter(|o| !is_uninteresting_sink(&o.path))
}

// ---------------------------------------------------------------------------
// Expansion
// ---------------------------------------------------------------------------

/// Try to fully resolve `word` to a literal string using the assignment map.
/// Returns `None` if any piece is unresolvable (command substitution,
/// undefined variable, arithmetic, complex parameter op, …).
fn expand_word(
    word: &Word,
    opts: &ParserOptions,
    assignments: &HashMap<String, String>,
) -> Option<String> {
    expand_str(&word.value, opts, assignments)
}

fn expand_str(
    raw: &str,
    opts: &ParserOptions,
    assignments: &HashMap<String, String>,
) -> Option<String> {
    let pieces = brush_parser::word::parse(raw, opts).ok()?;
    let mut out = String::new();
    for p in &pieces {
        let s = expand_piece(&p.piece, opts, assignments)?;
        out.push_str(&s);
    }
    Some(out)
}

fn expand_piece(
    piece: &WordPiece,
    opts: &ParserOptions,
    assignments: &HashMap<String, String>,
) -> Option<String> {
    match piece {
        WordPiece::Text(s) | WordPiece::SingleQuotedText(s) | WordPiece::AnsiCQuotedText(s) => {
            Some(s.clone())
        }
        WordPiece::EscapeSequence(s) => {
            // `\x` — drop the backslash, keep the next char.
            Some(s.strip_prefix('\\').unwrap_or(s).to_string())
        }
        WordPiece::DoubleQuotedSequence(pieces)
        | WordPiece::GettextDoubleQuotedSequence(pieces) => {
            let mut out = String::new();
            for p in pieces {
                out.push_str(&expand_piece(&p.piece, opts, assignments)?);
            }
            Some(out)
        }
        WordPiece::ParameterExpansion(pe) => expand_param(pe, opts, assignments),
        WordPiece::CommandSubstitution(_)
        | WordPiece::BackquotedCommandSubstitution(_)
        | WordPiece::ArithmeticExpression(_)
        | WordPiece::TildeExpansion(_) => None,
    }
}

fn expand_param(
    pe: &ParameterExpr,
    opts: &ParserOptions,
    assignments: &HashMap<String, String>,
) -> Option<String> {
    match pe {
        ParameterExpr::Parameter {
            parameter: Parameter::Named(name),
            indirect: false,
        } => assignments.get(name).cloned(),
        ParameterExpr::UseDefaultValues {
            parameter: Parameter::Named(name),
            indirect: false,
            default_value,
            ..
        }
        | ParameterExpr::AssignDefaultValues {
            parameter: Parameter::Named(name),
            indirect: false,
            default_value,
            ..
        } => {
            if let Some(v) = assignments.get(name) {
                if !v.is_empty() {
                    return Some(v.clone());
                }
            }
            // Fall through to default. `default_value` is the raw text
            // between `:-` / `:=` and `}`; recurse so e.g.
            // `${X:-${Y:-./tmp/z}}` works.
            match default_value {
                Some(d) => expand_str(d, opts, assignments),
                None => Some(String::new()),
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Absolutization
// ---------------------------------------------------------------------------

/// Resolve a parsed output path against `base`.
///
/// - Absolute paths are returned unchanged.
/// - `~/...` is expanded against `$HOME`; if home is unavailable, the original
///   string is returned unchanged.
/// - Relative paths are joined onto `base` and normalized (`.` dropped, `..`
///   folded). Does not touch the filesystem.
fn absolutize(path: &str, base: &Path) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return normalize_path(PathBuf::from(home).join(rest));
        }
        return path.to_string();
    }
    normalize_path(base.join(p))
}

/// Drop `.` segments, fold `..` against prior `Normal` components. Preserves
/// the root / prefix. Does not touch the filesystem.
fn normalize_path(p: impl AsRef<Path>) -> String {
    let mut out = PathBuf::new();
    for c in p.as_ref().components() {
        match c {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
        }
    }
    out.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Signature
// ---------------------------------------------------------------------------

fn signature_from_program(program: &Program, opts: &ParserOptions) -> Option<String> {
    let complete = program.complete_commands.first()?;
    let CompoundListItem(aol, _) = complete.0.first()?;
    let pipeline = &aol.first;
    let leftmost = pipeline.seq.first()?;
    let Command::Simple(sc) = leftmost else {
        return None;
    };
    let argv0_word = sc.word_or_name.as_ref()?;
    // For signatures we don't bother with assignment maps — `cargo`, `python`,
    // `pnpm` etc. are always literal. Fall back to `Word.value` if expansion
    // somehow fails (it shouldn't for a bare literal).
    let assignments: HashMap<String, String> = HashMap::new();
    let argv0 =
        expand_word(argv0_word, opts, &assignments).unwrap_or_else(|| argv0_word.value.clone());

    let first_arg = sc.suffix.as_ref().and_then(|suf| {
        suf.0.iter().find_map(|item| match item {
            CommandPrefixOrSuffixItem::Word(w) => {
                let text = expand_word(w, opts, &assignments).unwrap_or_else(|| w.value.clone());
                if text.starts_with('-') {
                    None
                } else {
                    Some(text)
                }
            }
            _ => None,
        })
    });

    Some(match first_arg {
        Some(a) => format!("{argv0} {a}"),
        None => argv0,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse with a fixed base so absolutization is deterministic. Tests that
    /// care about resolved-path values pass a known base; tests that only
    /// check `resolved`/`literal_text` ignore it.
    fn outs(cmd: &str) -> Vec<Output> {
        parse(cmd, Path::new("/base")).expect("parse").outputs
    }

    fn sig(cmd: &str) -> Option<String> {
        parse(cmd, Path::new("/base")).expect("parse").signature
    }

    // ---- AST: redirects ----

    #[test]
    fn simple_stdout_redirect() {
        // `2>&1` is a DuplicateOutput and intentionally not recorded.
        let o = outs("cargo build > /tmp/build.log 2>&1");
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].kind, OutputKind::Redirect);
        assert_eq!(o[0].fd, Some(1));
        assert_eq!(o[0].op.as_deref(), Some(">"));
        assert_eq!(o[0].literal_text, "/tmp/build.log");
        // Absolute path passes through unchanged.
        assert_eq!(o[0].path, "/tmp/build.log");
    }

    #[test]
    fn append_redirect_with_stderr_fd() {
        let o = outs("python script.py 2>> ./tmp/err.log");
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].kind, OutputKind::Redirect);
        assert_eq!(o[0].fd, Some(2));
        assert_eq!(o[0].op.as_deref(), Some(">>"));
        assert_eq!(o[0].literal_text, "./tmp/err.log");
        // Relative path absolutized against the base.
        assert_eq!(o[0].path, "/base/tmp/err.log");
    }

    #[test]
    fn heredoc_to_file_captures_file_redirect() {
        let o = outs("cat > ./tmp/note.md <<EOF\nline one\nEOF\n");
        let file = o
            .iter()
            .find(|x| x.kind == OutputKind::Redirect)
            .expect("file redirect");
        assert_eq!(file.fd, Some(1));
        assert_eq!(file.op.as_deref(), Some(">"));
        assert_eq!(file.path, "/base/tmp/note.md");
    }

    #[test]
    fn tee_with_multiple_files() {
        let o = outs("tee ./tmp/a.log ./tmp/b.log ./tmp/c.log");
        let tees: Vec<&Output> = o.iter().filter(|x| x.kind == OutputKind::Tee).collect();
        assert_eq!(tees.len(), 3);
        for t in &tees {
            assert!(t.fd.is_none());
            assert!(t.op.is_none());
        }
        let paths: Vec<&str> = tees.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["/base/tmp/a.log", "/base/tmp/b.log", "/base/tmp/c.log"]
        );
    }

    #[test]
    fn tee_skips_append_flag() {
        let o = outs("tee -a ./tmp/append.log");
        let tees: Vec<&Output> = o.iter().filter(|x| x.kind == OutputKind::Tee).collect();
        assert_eq!(tees.len(), 1);
        assert_eq!(tees[0].path, "/base/tmp/append.log");
        assert!(!o.iter().any(|x| x.path == "-a"));
    }

    #[test]
    fn quoted_redirect_text_is_not_a_redirect() {
        let o = outs(r#"echo "redirect not real: > foo""#);
        assert!(o.is_empty(), "got: {o:?}");
    }

    #[test]
    fn subshell_inner_redirect_is_captured() {
        // A redirect inside a subshell's simple command is surfaced.
        let o = outs("(echo hi > ./tmp/sub.log)");
        let paths: Vec<&str> = o.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, vec!["/base/tmp/sub.log"]);
    }

    #[test]
    fn brace_group_inner_redirect_is_captured() {
        // A redirect inside a `{ ...; }` brace group is surfaced.
        let o = outs("{ echo hi > ./tmp/brace.log; }");
        let paths: Vec<&str> = o.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, vec!["/base/tmp/brace.log"]);
    }

    #[test]
    fn compound_outer_redirect_is_captured() {
        // A redirect attached to the compound command itself (after the group).
        let o = outs("{ echo hi; } > ./tmp/outer.log");
        let paths: Vec<&str> = o.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, vec!["/base/tmp/outer.log"]);
    }

    #[test]
    fn pipeline_with_redirects_on_each_stage() {
        let o = outs("foo > a.log | bar > b.log");
        let mut paths: Vec<&str> = o
            .iter()
            .filter(|x| x.kind == OutputKind::Redirect)
            .map(|x| x.path.as_str())
            .collect();
        paths.sort_unstable();
        assert_eq!(paths, vec!["/base/a.log", "/base/b.log"]);
    }

    #[test]
    fn if_compound_is_best_effort_skipped() {
        // Other compound forms (if/while/for/case) are not walked — no panic,
        // no outputs.
        let o = outs("if true; then echo hi; fi");
        assert!(o.is_empty(), "got: {o:?}");
    }

    // ---- Expansion ----

    #[test]
    fn prefix_assignment_expands() {
        let o = outs("LOG=./tmp/run.log do_work > $LOG");
        assert_eq!(o.len(), 1);
        assert!(o[0].resolved);
        assert_eq!(o[0].path, "/base/tmp/run.log");
        assert_eq!(o[0].literal_text, "$LOG");
    }

    #[test]
    fn standalone_assignment_then_use() {
        let o = outs("LOG=./tmp/run.log; do_work > $LOG");
        assert_eq!(o.len(), 1);
        assert!(o[0].resolved, "expected resolved, got {o:?}");
        assert_eq!(o[0].path, "/base/tmp/run.log");
        assert_eq!(o[0].literal_text, "$LOG");
    }

    #[test]
    fn parameter_expansion_with_default() {
        let o = outs(r#"do_work > "${KENN_UNSET_TMPDIR:-./tmp}/x.log""#);
        assert_eq!(o.len(), 1);
        assert!(o[0].resolved, "got {o:?}");
        assert_eq!(o[0].path, "/base/tmp/x.log");
    }

    #[test]
    fn command_substitution_is_unresolved() {
        let o = outs("do_work > $(date +./tmp/%H.log)");
        assert_eq!(o.len(), 1);
        assert!(!o[0].resolved);
        assert_eq!(o[0].literal_text, "$(date +./tmp/%H.log)");
        // Unresolved targets keep the literal text as the path (not absolutized).
        assert_eq!(o[0].path, o[0].literal_text);
    }

    #[test]
    fn undefined_variable_is_unresolved() {
        let o = outs("do_work > $KENN_DEFINITELY_UNSET_VAR_XYZ");
        assert_eq!(o.len(), 1);
        assert!(!o[0].resolved);
        assert_eq!(o[0].literal_text, "$KENN_DEFINITELY_UNSET_VAR_XYZ");
    }

    // D4: an ambient environment variable resolves the redirect target.
    // `std::env::set_var` is safe on edition 2021. We use uniquely-named keys
    // so the (multithreaded) test harness can't observe cross-test mutation.
    #[test]
    fn ambient_env_variable_resolves() {
        let key = "KENN_COLLECT_TEST_OUTDIR";
        std::env::set_var(key, "/env/out");
        let o = parse(
            "do_work > $KENN_COLLECT_TEST_OUTDIR/run.log",
            Path::new("/base"),
        )
        .expect("parse")
        .outputs;
        std::env::remove_var(key);
        assert_eq!(o.len(), 1);
        assert!(o[0].resolved, "ambient env var should resolve: {o:?}");
        // Absolute (from the env value) — passes through unchanged.
        assert_eq!(o[0].path, "/env/out/run.log");
    }

    // D4: an in-command assignment overrides the ambient environment.
    #[test]
    fn in_command_assignment_overrides_env() {
        let key = "KENN_COLLECT_TEST_OVERRIDE";
        std::env::set_var(key, "/env/value");
        let o = parse(
            "KENN_COLLECT_TEST_OVERRIDE=/in/cmd do_work > $KENN_COLLECT_TEST_OVERRIDE/run.log",
            Path::new("/base"),
        )
        .expect("parse")
        .outputs;
        std::env::remove_var(key);
        assert_eq!(o.len(), 1);
        assert!(o[0].resolved);
        assert_eq!(o[0].path, "/in/cmd/run.log");
    }

    // ---- Signature ----

    #[test]
    fn signature_cargo_subcommand() {
        assert_eq!(
            sig("cargo test --test foo --release").as_deref(),
            Some("cargo test")
        );
    }

    #[test]
    fn signature_python_script() {
        assert_eq!(
            sig("python train.py --epochs 5").as_deref(),
            Some("python train.py")
        );
    }

    #[test]
    fn signature_pipeline_leftmost_wins() {
        assert_eq!(
            sig("cargo test 2>&1 | tee ./tmp/test.log").as_deref(),
            Some("cargo test")
        );
    }

    #[test]
    fn signature_no_arguments() {
        assert_eq!(sig("ls").as_deref(), Some("ls"));
    }

    // ---- Errors ----

    #[test]
    fn malformed_input_does_not_panic() {
        let r = parse("echo \"unterminated", Path::new("/base"));
        match r {
            Ok(_) | Err(ParseError::Brush(_)) => {}
        }
    }
}
