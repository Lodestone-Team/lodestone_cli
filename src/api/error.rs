//! Errors of the API subcommands, and how they map to exit codes.

use std::fmt;

/// Process exit codes of the API subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    /// The core refused the request, or the user asked for something that cannot be done.
    Failure = 1,
    /// The command line is wrong.
    Usage = 2,
    /// Not logged in, or the token is no longer valid.
    Auth = 3,
}

/// An error of an API subcommand: a machine readable `kind`, a message for people, and the
/// exit code it ends the process with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    /// For errors from the core, its `ErrorKind` (such as `NotFound` or `PermissionDenied`).
    /// For errors of the CLI itself, one of the kinds in [`kind`].
    pub kind: String,
    pub message: String,
    pub exit: ExitCode,
}

/// Error kinds of the CLI itself. The core's kinds are passed through unchanged.
pub mod kind {
    pub const USAGE: &str = "Usage";
    pub const NOT_LOGGED_IN: &str = "NotLoggedIn";
    pub const UNAUTHORIZED: &str = "Unauthorized";
    pub const NETWORK: &str = "Network";
    pub const TLS: &str = "Tls";
    pub const UNTRUSTED_CERTIFICATE: &str = "UntrustedCertificate";
    pub const IO: &str = "Io";
    pub const NOT_FOUND: &str = "NotFound";
    pub const AMBIGUOUS: &str = "Ambiguous";
    pub const CONFIRMATION_REQUIRED: &str = "ConfirmationRequired";
    pub const CANCELLED: &str = "Cancelled";
    pub const TIMEOUT: &str = "Timeout";
    pub const TASK_FAILED: &str = "TaskFailed";
    pub const BAD_RESPONSE: &str = "BadResponse";
    pub const CONFLICT: &str = "Conflict";
    pub const UNSUPPORTED: &str = "UnsupportedOperation";
    pub const BAD_REQUEST: &str = "BadRequest";
}

impl CliError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        let exit = match kind {
            kind::USAGE => ExitCode::Usage,
            kind::NOT_LOGGED_IN | kind::UNAUTHORIZED => ExitCode::Auth,
            _ => ExitCode::Failure,
        };
        Self {
            kind: kind.to_string(),
            message: message.into(),
            exit,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(kind::USAGE, message)
    }

    pub fn io(context: impl fmt::Display, e: std::io::Error) -> Self {
        Self::new(kind::IO, format!("{context}: {e}"))
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(kind::NOT_FOUND, message)
    }

    pub fn bad_response(message: impl Into<String>) -> Self {
        Self::new(kind::BAD_RESPONSE, message)
    }

    /// The error for a failed HTTP request, from its status and body. The core answers
    /// `{"kind": ..., "causes": [<message>]}`; some routes answer plain text.
    pub fn from_response(status: u16, body: &str) -> Self {
        #[derive(serde::Deserialize)]
        struct CoreError {
            kind: String,
            #[serde(default)]
            causes: Vec<String>,
        }
        let parsed = serde_json::from_str::<CoreError>(body).ok();
        let message = match &parsed {
            Some(e) if !e.causes.is_empty() => e.causes.join(": "),
            Some(e) => e.kind.clone(),
            None if body.trim().is_empty() => format!("The core answered HTTP {status}"),
            None => body.trim().chars().take(2000).collect(),
        };
        let kind = match (status, &parsed) {
            // a 401 always means the token is missing, expired or revoked
            (401, _) => kind::UNAUTHORIZED.to_string(),
            (409, None) => kind::CONFLICT.to_string(),
            (_, Some(e)) => e.kind.clone(),
            (400 | 422, None) => kind::BAD_REQUEST.to_string(),
            (403, None) => "PermissionDenied".to_string(),
            (404, None) => kind::NOT_FOUND.to_string(),
            (429, None) => "TooManyRequests".to_string(),
            _ => "Internal".to_string(),
        };
        let mut error = Self::new(&kind, message);
        if status == 401 {
            error.exit = ExitCode::Auth;
            error.message = format!(
                "{} (log in again with `lodestone_cli login`)",
                error.message.trim_end_matches('.')
            );
        }
        error
    }

    /// The error as the JSON document printed with `--json`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "error": { "kind": self.kind, "message": self.message } })
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CliError {}

pub type CliResult<T> = Result<T, CliError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_errors_keep_their_kind() {
        let e = CliError::from_response(
            404,
            r#"{"kind":"NotFound","causes":["Instance not found"]}"#,
        );
        assert_eq!(e.kind, "NotFound");
        assert_eq!(e.message, "Instance not found");
        assert_eq!(e.exit, ExitCode::Failure);

        let e = CliError::from_response(
            403,
            r#"{"kind":"PermissionDenied","causes":["Only the owner can do this"]}"#,
        );
        assert_eq!(e.kind, "PermissionDenied");
        assert_eq!(e.exit, ExitCode::Failure);
    }

    #[test]
    fn unauthorized_exits_with_the_auth_code() {
        let e =
            CliError::from_response(401, r#"{"kind":"Unauthorized","causes":["Unauthorized"]}"#);
        assert_eq!(e.kind, "Unauthorized");
        assert_eq!(e.exit, ExitCode::Auth);
        // axum-auth answers a missing header with plain text
        let e = CliError::from_response(401, "`Authorization` header is missing");
        assert_eq!(e.exit, ExitCode::Auth);
        assert_eq!(e.kind, "Unauthorized");
    }

    #[test]
    fn plain_text_errors_get_a_kind_from_the_status() {
        let e = CliError::from_response(409, "File exists");
        assert_eq!(e.kind, "Conflict");
        assert_eq!(e.message, "File exists");
        let e = CliError::from_response(500, "");
        assert_eq!(e.kind, "Internal");
        assert_eq!(e.message, "The core answered HTTP 500");
    }

    #[test]
    fn json_shape() {
        let e = CliError::new(kind::NOT_FOUND, "No instance named x");
        assert_eq!(
            e.to_json(),
            serde_json::json!({"error": {"kind": "NotFound", "message": "No instance named x"}})
        );
    }

    #[test]
    fn exit_codes() {
        assert_eq!(CliError::usage("x").exit, ExitCode::Usage);
        assert_eq!(CliError::new(kind::NOT_LOGGED_IN, "x").exit, ExitCode::Auth);
        assert_eq!(CliError::new(kind::TIMEOUT, "x").exit, ExitCode::Failure);
    }
}
