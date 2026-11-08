//! Questions to the user. Prompts go to stderr, so stdout stays clean for results. Without
//! a terminal nothing is asked: commands fail with a message instead of hanging.

use std::io::{BufRead, IsTerminal, Write};

use super::error::{kind, CliError, CliResult};

/// Whether the user can answer questions: stdin and stderr are terminals.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Asks for a line of text. `default` is used for an empty answer.
pub fn ask(question: &str, default: Option<&str>) -> CliResult<String> {
    if !interactive() {
        return Err(CliError::usage(format!(
            "{question}: no terminal to ask on; pass it as an option"
        )));
    }
    match default {
        Some(d) => eprint!("{question} [{d}]: "),
        None => eprint!("{question}: "),
    }
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| CliError::io("Failed to read the answer", e))?;
    let answer = line.trim().to_string();
    Ok(match (answer.is_empty(), default) {
        (true, Some(d)) => d.to_string(),
        _ => answer,
    })
}

/// Asks for a secret without echoing it.
pub fn ask_secret(question: &str) -> CliResult<String> {
    if !interactive() {
        return Err(CliError::usage(format!(
            "{question}: no terminal to ask on; use --password-stdin or the environment variable"
        )));
    }
    rpassword::prompt_password(format!("{question}: "))
        .map_err(|e| CliError::io("Failed to read the password", e))
}

/// Reads a secret from the first line of stdin (for `--password-stdin`).
pub fn read_secret_from_stdin() -> CliResult<String> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| CliError::io("Failed to read the password from stdin", e))?;
    let secret = line.trim_end_matches(['\r', '\n']).to_string();
    if secret.is_empty() {
        return Err(CliError::usage("Expected a password on stdin"));
    }
    Ok(secret)
}

/// Confirms a destructive action: `yes` (from `--yes`) skips the question; without a
/// terminal the command fails and says to pass `--yes`.
pub fn confirm(yes: bool, question: &str) -> CliResult<()> {
    if yes {
        return Ok(());
    }
    if !interactive() {
        return Err(CliError::new(
            kind::CONFIRMATION_REQUIRED,
            format!("{question} Pass --yes to confirm (no terminal to ask on)."),
        ));
    }
    eprint!("{question} [y/N]: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| CliError::io("Failed to read the answer", e))?;
    if matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CliError::new(
            kind::CANCELLED,
            "Cancelled, nothing was changed",
        ))
    }
}
