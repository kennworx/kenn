use std::io::{self, IsTerminal, Write};

use anyhow::Result;
use kenn_store::{lifecycle, Layout, RollbackError, Store};

use crate::exit::ExitCodes;

/// Parameters resolved from CLI args + environment. Kept narrow so
/// [`execute`] is pure-function and unit-testable against a `Store`.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub yes: bool,
}

/// User decision after the interactive prompt (or `--yes` shortcut).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Confirmed,
    Aborted,
    /// Non-TTY context with no `--yes` — caller MUST exit with
    /// `ExitCodes::Usage` and print the error message embedded here.
    UsageError,
}

/// Render the confirmation prompt text. Pulled out so tests can lock
/// the exact wording without spawning stdout.
#[must_use]
pub fn confirm_prompt(live_label: &str) -> String {
    format!("Roll back from snapshot `{live_label}`? [y/N] ")
}

/// Map a (`yes` flag, `is_tty`, user reply) tuple to a [`Confirmation`].
/// Pure function — no I/O — so the decision matrix is independently
/// testable.
#[must_use]
pub fn decide_confirmation(yes: bool, is_tty: bool, user_reply: Option<&str>) -> Confirmation {
    if yes {
        return Confirmation::Confirmed;
    }
    if !is_tty {
        return Confirmation::UsageError;
    }
    match user_reply.map(|s| s.trim().to_lowercase()) {
        Some(s) if matches!(s.as_str(), "y" | "yes") => Confirmation::Confirmed,
        _ => Confirmation::Aborted,
    }
}

/// Map a [`lifecycle::rollback`] result to an [`ExitCodes`] +
/// human-readable line. Pure function — pulled out so the error-mapping
/// branches are testable without invoking a real Store.
///
/// Returns `(maybe_stdout, maybe_stderr, exit_code)`. `Err` on
/// unrecoverable lifecycle errors.
pub fn classify_rollback_result(
    result: Result<std::path::PathBuf, RollbackError>,
) -> Result<(Option<String>, Option<String>, ExitCodes)> {
    match result {
        Ok(target) => Ok((
            Some(format!("live → {}", target.display())),
            None,
            ExitCodes::Ok,
        )),
        Err(RollbackError::NoPrevious | RollbackError::NoLive) => Ok((
            None,
            Some("error: no previous snapshot retained".into()),
            ExitCodes::Generic,
        )),
        Err(e) => Err(e.into()),
    }
}

/// Run the rollback against a resolved Store. Side-effect-free except
/// for the lifecycle mutation itself; all I/O is delegated to caller.
///
/// `is_tty` and `user_reply` are passed in rather than queried so the
/// function is independently testable. `user_reply` is `Some(line)` if
/// the user typed at the prompt and `None` otherwise.
pub fn execute(
    store: &Store,
    params: Params,
    is_tty: bool,
    user_reply: Option<&str>,
) -> Result<(Option<String>, Option<String>, ExitCodes)> {
    match decide_confirmation(params.yes, is_tty, user_reply) {
        Confirmation::Confirmed => classify_rollback_result(lifecycle::rollback(store)),
        Confirmation::Aborted => Ok((Some("aborted".into()), None, ExitCodes::Ok)),
        Confirmation::UsageError => Ok((
            None,
            Some("error: rollback requires --yes in non-TTY contexts".into()),
            ExitCodes::Usage,
        )),
    }
}

pub fn run(layout: Layout, yes: bool) -> Result<ExitCodes> {
    let store = Store::open(layout)?;
    let live = store.live_target();
    let stdout = io::stdout();
    let is_tty = stdout.is_terminal();
    let params = Params { yes };

    // Read user reply only when we actually need it (interactive +
    // not --yes). Skip stdin otherwise.
    let user_reply = if !params.yes && is_tty {
        let target_label = live
            .as_ref()
            .and_then(|p| p.file_name().and_then(|s| s.to_str()))
            .unwrap_or("<none>");
        print!("{}", confirm_prompt(target_label));
        drop(io::stdout().flush());
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;
        Some(buf)
    } else {
        None
    };

    let (stdout_msg, stderr_msg, code) = execute(&store, params, is_tty, user_reply.as_deref())?;
    if let Some(line) = stdout_msg {
        println!("{line}");
    }
    if let Some(line) = stderr_msg {
        eprintln!("{line}");
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `decide_confirmation` is pure — exhaustively cover the matrix.
    #[test]
    fn decide_confirmation_yes_short_circuits() {
        assert_eq!(
            decide_confirmation(true, false, None),
            Confirmation::Confirmed
        );
        assert_eq!(
            decide_confirmation(true, true, Some("n")),
            Confirmation::Confirmed
        );
    }

    #[test]
    fn decide_confirmation_no_tty_and_not_yes_is_usage_error() {
        assert_eq!(
            decide_confirmation(false, false, None),
            Confirmation::UsageError
        );
        assert_eq!(
            decide_confirmation(false, false, Some("y")),
            Confirmation::UsageError
        );
    }

    #[test]
    fn decide_confirmation_tty_reply_y_confirms() {
        for reply in ["y", "Y", "yes", "YES", " yes ", "y\n"] {
            assert_eq!(
                decide_confirmation(false, true, Some(reply)),
                Confirmation::Confirmed,
                "{reply:?}",
            );
        }
    }

    #[test]
    fn decide_confirmation_tty_other_reply_aborts() {
        for reply in [
            None,
            Some(""),
            Some("n"),
            Some("no"),
            Some("nope"),
            Some("\n"),
        ] {
            assert_eq!(
                decide_confirmation(false, true, reply),
                Confirmation::Aborted,
                "{reply:?}",
            );
        }
    }

    #[test]
    fn confirm_prompt_contains_target_label() {
        let p = confirm_prompt("2026-05-01T12-00-00Z");
        assert!(p.contains("2026-05-01T12-00-00Z"));
        assert!(p.contains("[y/N]"));
    }

    /// `classify_rollback_result` covers all three lifecycle outcomes.
    #[test]
    fn classify_rollback_result_ok() {
        let (out, err, code) = classify_rollback_result(Ok("/tmp/x".into())).expect("ok");
        assert_eq!(code, ExitCodes::Ok);
        assert!(out.expect("stdout").contains("live → /tmp/x"));
        assert!(err.is_none());
    }

    #[test]
    fn classify_rollback_result_no_previous_is_generic_error() {
        let (out, err, code) =
            classify_rollback_result(Err(RollbackError::NoPrevious)).expect("ok");
        assert_eq!(code, ExitCodes::Generic);
        assert!(out.is_none());
        assert!(err.expect("stderr").contains("no previous snapshot"));
    }

    #[test]
    fn classify_rollback_result_no_live_is_generic_error() {
        let (_, _, code) = classify_rollback_result(Err(RollbackError::NoLive)).expect("ok");
        assert_eq!(code, ExitCodes::Generic);
    }
}
