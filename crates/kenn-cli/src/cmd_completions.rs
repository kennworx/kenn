//! `kenn completions <shell>` — print a shell completion script to stdout.

use std::io::Write;

use clap::CommandFactory;
use clap_complete::Shell;

use crate::exit::ExitCodes;
use crate::Cli;

pub fn run(shell: Shell) -> ExitCodes {
    // Generate into a buffer first so a downstream `| head` (BrokenPipe)
    // doesn't panic inside clap_complete's internal `write!` calls.
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, bin_name, &mut buf);
    // clap_complete's fish generator (≤4.6) skips positional value
    // completions. Append an overlay so `kenn completions <TAB>`
    // suggests the shell values instead of falling back to files.
    if shell == Shell::Fish {
        buf.extend_from_slice(
            b"\ncomplete -c kenn -n '__fish_kenn_using_subcommand completions' \
              -f -a 'bash zsh fish powershell elvish'\n",
        );
    }
    // BrokenPipe on stdout (e.g. `kenn completions fish | head`) is the
    // standard Unix signal that the reader is done — swallow it.
    drop(std::io::stdout().write_all(&buf));
    ExitCodes::Ok
}
