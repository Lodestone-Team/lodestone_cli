//! Printing results: tables and text for people, one JSON document (or NDJSON for streams)
//! with `--json`. Results go to stdout; messages, progress and errors go to stderr.

use std::io::Write;

use serde_json::Value;

use super::error::{CliError, CliResult};
use super::keys::redact;

#[derive(Debug, Clone, Copy, Default)]
pub struct Output {
    pub json: bool,
}

impl Output {
    /// Prints one JSON document on stdout.
    pub fn json(&self, value: &Value) {
        let mut stdout = std::io::stdout().lock();
        let _ = serde_json::to_writer_pretty(&mut stdout, value);
        let _ = writeln!(stdout);
    }

    /// Prints one line of NDJSON on stdout and flushes it, for streams. Fails once nothing
    /// reads stdout any more (kind `StdoutClosed`), so the stream can stop: the process
    /// ignores SIGPIPE, as Rust programs do, and would otherwise keep streaming forever.
    pub fn json_line(&self, value: &Value) -> CliResult<()> {
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer(&mut stdout, value)
            .map_err(std::io::Error::from)
            .and_then(|()| writeln!(stdout))
            .and_then(|()| stdout.flush())
            .map_err(CliError::output)
    }

    /// Prints one line of text on stdout and flushes it, for streams. Like `json_line`, it
    /// fails once nothing reads stdout, where `println!` would panic.
    pub fn line(&self, text: impl std::fmt::Display) -> CliResult<()> {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{text}")
            .and_then(|()| stdout.flush())
            .map_err(CliError::output)
    }

    /// For commands that change something and have nothing else to show: `{"ok": true}`
    /// plus `extra`'s fields with `--json`, else `message`.
    pub fn done(&self, message: impl AsRef<str>, extra: Value) {
        if self.json {
            let mut doc = serde_json::json!({ "ok": true });
            if let (Some(doc), Value::Object(extra)) = (doc.as_object_mut(), extra) {
                doc.extend(extra);
            }
            self.json(&doc);
        } else {
            println!("{}", message.as_ref());
        }
    }

    /// Prints `value` with `--json`, else runs `human`.
    pub fn show(&self, value: &Value, human: impl FnOnce()) {
        if self.json {
            self.json(value);
        } else {
            human();
        }
    }

    /// A note for people, on stderr. Not printed with `--json`. API keys in it are
    /// redacted, like everything else on stderr.
    pub fn note(&self, message: impl AsRef<str>) {
        if !self.json {
            eprintln!("{}", redact(message.as_ref()));
        }
    }

    /// A warning on stderr, printed in both modes (stderr is never parsed).
    pub fn warn(&self, message: impl AsRef<str>) {
        eprintln!("warning: {}", redact(message.as_ref()));
    }

    /// An error on stderr. API keys in it (for example echoed back by the core) are
    /// redacted.
    pub fn error(&self, error: &CliError) {
        let error = CliError {
            message: redact(&error.message),
            ..error.clone()
        };
        if self.json {
            let mut stderr = std::io::stderr().lock();
            let _ = serde_json::to_writer(&mut stderr, &error.to_json());
            let _ = writeln!(stderr);
        } else {
            eprintln!("error: {}", error.message);
        }
    }
}

/// A plain text table: columns padded to their widest cell.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new<S: Into<String>>(headers: impl IntoIterator<Item = S>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row<S: Into<String>>(&mut self, cells: impl IntoIterator<Item = S>) {
        self.rows.push(cells.into_iter().map(Into::into).collect());
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn render(&self) -> String {
        let columns = self.headers.len();
        let mut widths: Vec<usize> = self.headers.iter().map(|h| width(h)).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate().take(columns) {
                widths[i] = widths[i].max(width(cell));
            }
        }
        let mut out = String::new();
        let mut line = |cells: &[String]| {
            let mut text = String::new();
            for (i, cell) in cells.iter().enumerate().take(columns) {
                if i + 1 == columns {
                    text.push_str(cell);
                } else {
                    text.push_str(cell);
                    text.push_str(&" ".repeat(widths[i] - width(cell) + 2));
                }
            }
            out.push_str(text.trim_end());
            out.push('\n');
        };
        line(&self.headers);
        for row in &self.rows {
            line(row);
        }
        out
    }

    pub fn print(&self) {
        print!("{}", self.render());
    }

    /// Prints the table, or `empty` if it has no rows.
    pub fn print_or(&self, empty: &str) {
        if self.is_empty() {
            println!("{empty}");
        } else {
            self.print();
        }
    }
}

fn width(s: &str) -> usize {
    s.chars().count()
}

/// Prints `key: value` lines with the values aligned.
pub fn print_fields(fields: &[(&str, String)]) {
    let key_width = fields.iter().map(|(k, _)| width(k)).max().unwrap_or(0);
    for (key, value) in fields {
        println!("{key}:{} {value}", " ".repeat(key_width - width(key)));
    }
}

/// A byte count for people, such as `1.5 GiB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A Unix time in seconds, as local time.
pub fn format_unix_seconds(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| seconds.to_string())
}

/// A Unix time in milliseconds, as local time.
pub fn format_unix_millis(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| millis.to_string())
}

/// A duration for people, such as `3d 4h` or `12m 5s`.
pub fn format_duration(seconds: u64) -> String {
    let (d, h, m, s) = (
        seconds / 86400,
        seconds % 86400 / 3600,
        seconds % 3600 / 60,
        seconds % 60,
    );
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// A JSON value as a table cell: strings without quotes, `null` as `-`.
pub fn cell(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(cell).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_columns() {
        let mut table = Table::new(["NAME", "STATE"]);
        table.row(["survival", "Running"]);
        table.row(["c", "Stopped"]);
        assert_eq!(
            table.render(),
            "NAME      STATE\nsurvival  Running\nc         Stopped\n"
        );
    }

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(5), "5s");
        assert_eq!(format_duration(65), "1m 5s");
        assert_eq!(format_duration(3 * 3600 + 120), "3h 2m");
        assert_eq!(format_duration(2 * 86400 + 3600), "2d 1h");
    }

    #[test]
    fn cells() {
        assert_eq!(cell(&Value::Null), "-");
        assert_eq!(cell(&serde_json::json!("x")), "x");
        assert_eq!(cell(&serde_json::json!(["a", "b"])), "a, b");
        assert_eq!(cell(&serde_json::json!(3)), "3");
    }
}
