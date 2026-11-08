//! The API subcommands of the CLI: a client for a running Lodestone Core.

pub mod b64;
pub mod cli;
mod cmd;
pub mod config;
pub mod error;
pub mod events;
pub mod http;
pub mod multipart;
pub mod output;
pub mod prompt;
pub mod resolve;
pub mod tls;
pub mod ws;

use cli::{Command, GlobalOpts};
use config::{Profile, Profiles};
use error::{kind, CliError, CliResult, ExitCode};
use http::Connection;
use output::Output;
use tls::Trust;

/// Environment variables that override the saved profile, for CI and agents.
pub const ENV_URL: &str = "LODESTONE_URL";
pub const ENV_TOKEN: &str = "LODESTONE_TOKEN";
pub const ENV_FINGERPRINT: &str = "LODESTONE_FINGERPRINT";
pub const ENV_CA_CERT: &str = "LODESTONE_CA_CERT";
pub const ENV_USERNAME: &str = "LODESTONE_USERNAME";
pub const ENV_PASSWORD: &str = "LODESTONE_PASSWORD";

pub struct Ctx {
    pub out: Output,
    pub global: GlobalOpts,
}

/// Where the connection settings came from.
pub struct Session {
    pub conn: Connection,
    /// The profile used, if any.
    pub profile_name: Option<String>,
    pub profile: Option<Profile>,
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl Ctx {
    /// The connection for this command: `LODESTONE_URL`/`LODESTONE_TOKEN` if set, else the
    /// selected profile. A profile's token and certificate pin are only used for the
    /// profile's own URL, so an overridden URL never gets them.
    pub fn session(&self, require_token: bool) -> CliResult<Session> {
        let profiles = Profiles::load()?;
        let name = profiles.selected_name(self.global.profile.as_deref());
        let profile = profiles.profiles.get(&name).cloned();
        if self.global.profile.is_some() && profile.is_none() {
            return Err(CliError::new(
                kind::NOT_LOGGED_IN,
                format!("There is no profile named `{name}` (see `lodestone_cli profile list`)"),
            ));
        }
        let url = match (env_var(ENV_URL), &profile) {
            (Some(url), _) => http::parse_core_url(&url)?,
            (None, Some(profile)) => http::parse_core_url(&profile.url)?,
            (None, None) => {
                return Err(CliError::new(
                    kind::NOT_LOGGED_IN,
                    "Not logged in: run `lodestone_cli login <url>`, or set LODESTONE_URL and LODESTONE_TOKEN",
                ))
            }
        };
        let same_core = profile
            .as_ref()
            .and_then(|p| http::parse_core_url(&p.url).ok())
            .is_some_and(|profile_url| profile_url == url);
        let profile = profile.filter(|_| same_core);
        let token = env_var(ENV_TOKEN).or_else(|| profile.as_ref().map(|p| p.token.clone()));
        let trust = if let Some(fingerprint) = env_var(ENV_FINGERPRINT) {
            Trust::Pinned(tls::parse_fingerprint(&fingerprint)?)
        } else if let Some(path) = env_var(ENV_CA_CERT) {
            let pem = std::fs::read(&path)
                .map_err(|e| CliError::io(format!("Cannot read {ENV_CA_CERT} {path}"), e))?;
            Trust::CaCerts(tls::parse_pem_certs(&pem)?)
        } else if let Some(profile) = &profile {
            Trust::from_settings(&profile.tls)?
        } else {
            Trust::System
        };
        if require_token && token.is_none() {
            return Err(CliError::new(
                kind::NOT_LOGGED_IN,
                format!(
                    "Not logged in to {url}: run `lodestone_cli login {url}`, or set LODESTONE_TOKEN"
                ),
            ));
        }
        warn_plain_http(&self.out, &url);
        Ok(Session {
            conn: Connection::new(url, token, trust, self.global.verbose)?,
            profile_name: profile.as_ref().map(|_| name),
            profile,
        })
    }

    /// The connection for a command that needs a login.
    pub fn connect(&self) -> CliResult<Connection> {
        Ok(self.session(true)?.conn)
    }
}

/// Warns that a plain `http://` URL to another machine sends passwords and tokens
/// unencrypted.
pub fn warn_plain_http(out: &Output, url: &reqwest::Url) {
    if url.scheme() == "http" && !http::is_loopback_host(url) {
        out.warn(format!(
            "{url} uses plain HTTP: passwords and tokens cross the network unencrypted. Use https:// (cores refuse plain HTTP from other devices by default)."
        ));
    }
}

/// Runs an API subcommand and returns the process exit code.
pub async fn run(global: GlobalOpts, command: Command) -> i32 {
    let ctx = Ctx {
        out: Output { json: global.json },
        global,
    };
    match cmd::dispatch(&ctx, command).await {
        Ok(()) => ExitCode::Success as i32,
        Err(e) => {
            ctx.out.error(&e);
            e.exit as i32
        }
    }
}

/// Prints a command line error (from clap) the way the API subcommands print errors, and
/// returns the usage exit code.
pub fn report_usage_error(json: bool, message: &str) -> i32 {
    let error = CliError::usage(message.trim().to_string());
    Output { json }.error(&error);
    ExitCode::Usage as i32
}
