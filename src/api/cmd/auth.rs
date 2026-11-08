//! `login`, `logout`, `profile` and `core setup`: connecting to a core and saving logins.

use reqwest::Url;
use serde_json::{json, Value};

use crate::api::cli::{ConnectArgs, LoginArgs, LogoutArgs, ProfileCmd, SetupArgs};
use std::io::IsTerminal;

use crate::api::config::{AuthKind, Profile, Profiles, TlsSettings, DEFAULT_PROFILE};
use crate::api::error::{kind, CliError, CliResult, ExitCode};
use crate::api::http::{self, Body, Connection};
use crate::api::output::Table;
use crate::api::tls::{self, Trust};
use crate::api::{keys, prompt, warn_plain_http, Ctx, ENV_API_KEY, ENV_PASSWORD, ENV_USERNAME};

/// Decides how to trust the core at `url` before sending it a password: `--ca-cert`,
/// `--fingerprint`, the pin of an existing profile for the same URL, the system's trust,
/// or, for a certificate nobody vouches for, the user comparing its fingerprint (trust on
/// first use).
async fn establish_trust(
    url: &Url,
    args: &ConnectArgs,
    existing: Option<&Profile>,
) -> CliResult<(Trust, TlsSettings)> {
    if url.scheme() == "http" {
        return Ok((Trust::System, TlsSettings::default()));
    }
    if let Some(path) = &args.ca_cert {
        let pem = std::fs::read(path)
            .map_err(|e| CliError::io(format!("Cannot read {}", path.display()), e))?;
        let trust = Trust::CaCerts(tls::parse_pem_certs(&pem)?);
        let probe = tls::probe(url, &trust).await?;
        if let Some(e) = probe.error {
            return Err(CliError::new(
                kind::UNTRUSTED_CERTIFICATE,
                format!(
                    "The core's certificate is not signed by {}: {}",
                    path.display(),
                    e.message
                ),
            ));
        }
        let settings = TlsSettings {
            pinned_sha256: None,
            ca_cert_pem: Some(String::from_utf8_lossy(&pem).into_owned()),
        };
        return Ok((trust, settings));
    }
    if let Some(given) = &args.fingerprint {
        let pin = tls::parse_fingerprint(given)?;
        let probe = tls::probe(url, &Trust::Pinned(pin)).await?;
        if probe.error.is_some() {
            let actual = probe
                .fingerprint
                .map(|f| tls::format_fingerprint(&f))
                .unwrap_or_else(|| "unknown".to_string());
            return Err(CliError::new(
                kind::UNTRUSTED_CERTIFICATE,
                format!(
                    "The core's certificate does not match --fingerprint: its SHA-256 fingerprint is {actual}"
                ),
            ));
        }
        return Ok((Trust::Pinned(pin), pinned_settings(&pin)));
    }
    if let Some(profile) = existing {
        if profile.tls != TlsSettings::default() {
            // keep the earlier decision; a changed certificate needs a new confirmation
            let trust = Trust::from_settings(&profile.tls)?;
            let probe = tls::probe(url, &trust).await?;
            match probe.error {
                None => return Ok((trust, profile.tls.clone())),
                Some(e) => {
                    return Err(CliError::new(
                        kind::UNTRUSTED_CERTIFICATE,
                        format!(
                            "{}. If the core really has a new certificate, compare its fingerprint with the core's log or Core Settings and pass --fingerprint",
                            e.message.trim_end_matches('.')
                        ),
                    ))
                }
            }
        }
    }
    let probe = tls::probe(url, &Trust::System).await?;
    let Some(reason) = probe.error else {
        return Ok((Trust::System, TlsSettings::default()));
    };
    let fingerprint = probe
        .fingerprint
        .expect("probe errors come with the certificate");
    let shown = tls::format_fingerprint(&fingerprint);
    if !prompt::interactive() {
        return Err(CliError::new(
            kind::UNTRUSTED_CERTIFICATE,
            format!(
                "The certificate of {url} is not trusted by this computer ({}). Its SHA-256 fingerprint is {shown}. Check it against the fingerprint in the core's log or Core Settings, then pass --fingerprint {shown}",
                reason.message
            ),
        ));
    }
    eprintln!(
        "The certificate of {url} is not signed by an authority this computer trusts\n({}).\n\
         Lodestone cores use a self-signed certificate by default. Its SHA-256 fingerprint is:\n\n  {shown}\n\n\
         Compare it with the fingerprint in the core's log or in Core Settings on the dashboard.",
        reason.message
    );
    // `--yes` never answers this: trusting a certificate needs the user's own check
    prompt::confirm(false, "Is it the same, and do you trust this core?")?;
    Ok((Trust::Pinned(fingerprint), pinned_settings(&fingerprint)))
}

fn pinned_settings(pin: &[u8; 32]) -> TlsSettings {
    TlsSettings {
        pinned_sha256: Some(tls::format_fingerprint(pin)),
        ca_cert_pem: None,
    }
}

fn username(args: &ConnectArgs) -> CliResult<String> {
    if let Some(name) = &args.username {
        return Ok(name.clone());
    }
    if let Ok(name) = std::env::var(ENV_USERNAME) {
        if !name.is_empty() {
            return Ok(name);
        }
    }
    let name = prompt::ask("User name", None)?;
    if name.is_empty() {
        return Err(CliError::usage("A user name is required"));
    }
    Ok(name)
}

fn password(args: &ConnectArgs, question: &str) -> CliResult<String> {
    if args.password_stdin {
        return prompt::read_secret_from_stdin();
    }
    if let Ok(password) = std::env::var(ENV_PASSWORD) {
        if !password.is_empty() {
            return Ok(password);
        }
    }
    prompt::ask_secret(question)
}

/// The profile name for a login to `url`: `--profile`, else the profile already saved for
/// that URL, else `default` if it is free, else the host and port.
fn profile_name_for(ctx: &Ctx, profiles: &Profiles, url: &Url) -> String {
    if let Some(name) = &ctx.global.profile {
        return name.clone();
    }
    if let Some((name, _)) = profiles
        .profiles
        .iter()
        .find(|(_, p)| http::parse_core_url(&p.url).ok().as_ref() == Some(url))
    {
        return name.clone();
    }
    if !profiles.profiles.contains_key(DEFAULT_PROFILE) {
        return DEFAULT_PROFILE.to_string();
    }
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        _ => url.to_string(),
    }
}

fn tls_json(settings: &TlsSettings) -> Value {
    if let Some(pin) = &settings.pinned_sha256 {
        json!({"mode": "pinned", "fingerprint_sha256": pin})
    } else if settings.ca_cert_pem.is_some() {
        json!({"mode": "ca_cert"})
    } else {
        json!({"mode": "system"})
    }
}

/// How a profile authenticates, for `profile list` and login results. Never the key itself.
fn auth_json(auth: AuthKind, token: &str) -> Value {
    match auth {
        AuthKind::Session => json!({"type": "session"}),
        AuthKind::ApiKey => json!({"type": "api_key", "key": keys::display(token)}),
    }
}

/// Saves a login reply (`LoginReply`: a token and the user) as a profile and makes it the
/// current one.
fn save_login(
    ctx: &Ctx,
    url: &Url,
    reply: &Value,
    settings: TlsSettings,
    auth: AuthKind,
    action: &str,
) -> CliResult<()> {
    let token = reply["token"]
        .as_str()
        .ok_or_else(|| CliError::bad_response("The login reply has no token"))?;
    let user = &reply["user"];
    let username = user["username"].as_str().unwrap_or_default().to_string();
    let mut profiles = Profiles::load()?;
    let name = profile_name_for(ctx, &profiles, url);
    profiles.profiles.insert(
        name.clone(),
        Profile {
            url: url.to_string().trim_end_matches('/').to_string(),
            token: token.to_string(),
            username: username.clone(),
            uid: user["uid"].as_str().unwrap_or_default().to_string(),
            tls: settings.clone(),
            auth,
        },
    );
    profiles.current = Some(name.clone());
    profiles.save()?;
    let with = match auth {
        AuthKind::Session => String::new(),
        AuthKind::ApiKey => format!(" with API key {}", keys::display(token)),
    };
    ctx.out.done(
        format!("{action} {url} as {username}{with}; saved as profile `{name}`"),
        json!({
            "profile": name,
            "url": url.as_str().trim_end_matches('/'),
            "user": user,
            "tls": tls_json(&settings),
            "auth": auth_json(auth, token),
        }),
    );
    Ok(())
}

pub async fn login(ctx: &Ctx, args: LoginArgs) -> CliResult<()> {
    let url = http::parse_core_url(&args.url)?;
    warn_plain_http(&ctx.out, &url);
    let profiles = Profiles::load()?;
    let existing = profiles
        .profiles
        .values()
        .find(|p| http::parse_core_url(&p.url).ok().as_ref() == Some(&url));
    let (trust, settings) = establish_trust(&url, &args.connect, existing).await?;
    let conn = Connection::new(url.clone(), None, trust, ctx.global.verbose)?;
    let info = conn.get(&["info"]).await?;
    if info["is_setup"].as_bool() == Some(false) {
        return Err(CliError::new(
            kind::NOT_LOGGED_IN,
            format!(
                "This core has no owner yet. Set it up with `lodestone_cli core setup {url} <setup key>` (the key is in the core's log)"
            ),
        ));
    }
    if args.api_key {
        let key = api_key()?;
        let mut conn = conn;
        conn.set_token(key.clone());
        let user = conn.get(&["user", "info"]).await.map_err(|mut e| {
            if e.exit == ExitCode::Auth {
                e.message = format!(
                    "The core refused API key {}: it is wrong, expired or revoked",
                    keys::display(&key)
                );
            }
            e
        })?;
        let reply = json!({"token": key, "user": user});
        return save_login(
            ctx,
            &url,
            &reply,
            settings,
            AuthKind::ApiKey,
            "Logged in to",
        );
    }
    let username = username(&args.connect)?;
    let password = password(&args.connect, "Password")?;
    let reply = conn.login(&username, &password).await?;
    save_login(
        ctx,
        &url,
        &reply,
        settings,
        AuthKind::Session,
        "Logged in to",
    )
}

/// The API key for `login --api-key`: `LODESTONE_API_KEY`, else the first line of stdin
/// (asked for without echo on a terminal).
fn api_key() -> CliResult<String> {
    let key = match std::env::var(ENV_API_KEY) {
        Ok(key) if !key.trim().is_empty() => key,
        _ if std::io::stdin().is_terminal() => prompt::ask_secret("API key")?,
        _ => prompt::read_secret_from_stdin()?,
    };
    let key = key.trim().to_string();
    if !keys::is_api_key(&key) || keys::key_id(&key).is_none() {
        // never echo what was given: it may be a password or a token
        return Err(CliError::usage(
            "That is not a Lodestone API key (they start with lsk_)",
        ));
    }
    Ok(key)
}

pub async fn setup(ctx: &Ctx, args: SetupArgs) -> CliResult<()> {
    let url = http::parse_core_url(&args.url)?;
    warn_plain_http(&ctx.out, &url);
    let (trust, settings) = establish_trust(&url, &args.connect, None).await?;
    let conn = Connection::new(url.clone(), None, trust, ctx.global.verbose)?;
    let info = conn.get(&["info"]).await?;
    if info["is_setup"].as_bool() == Some(true) {
        return Err(CliError::new(
            "PermissionDenied",
            format!("This core already has an owner; log in with `lodestone_cli login {url}`"),
        ));
    }
    let key = match args.key {
        Some(key) => key,
        None => prompt::ask_secret("Setup key (from the core's log)")?,
    };
    let username = username(&args.connect)?;
    let password = password(&args.connect, "Password for the new owner")?;
    if !args.connect.password_stdin && prompt::interactive() && std::env::var(ENV_PASSWORD).is_err()
    {
        let again = prompt::ask_secret("Password again")?;
        if again != password {
            return Err(CliError::usage("The passwords do not match"));
        }
    }
    let reply = conn
        .post(
            &["setup", key.trim()],
            Body::Json(json!({ "username": username, "password": password })),
        )
        .await?;
    save_login(ctx, &url, &reply, settings, AuthKind::Session, "Set up")
}

pub async fn logout(ctx: &Ctx, args: LogoutArgs) -> CliResult<()> {
    let mut profiles = Profiles::load()?;
    let name = profiles.selected_name(ctx.global.profile.as_deref());
    let Some(profile) = profiles.profiles.get(&name).cloned() else {
        return Err(CliError::new(
            kind::NOT_LOGGED_IN,
            format!("There is no profile named `{name}`"),
        ));
    };
    if args.revoke {
        ctx.credential.set(Some(profile.auth));
        let url = http::parse_core_url(&profile.url)?;
        let trust = Trust::from_settings(&profile.tls)?;
        let conn = Connection::new(url, Some(profile.token.clone()), trust, ctx.global.verbose)?;
        let uid = if profile.uid.is_empty() {
            conn.get(&["user", "info"]).await?["uid"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        } else {
            profile.uid.clone()
        };
        conn.post(&["user", "logout", &uid], Body::Empty).await?;
    }
    profiles.profiles.remove(&name);
    if profiles.current.as_deref() == Some(name.as_str()) {
        profiles.current = None;
    }
    profiles.save()?;
    let revoked = if args.revoke {
        " and ended its sessions on the core"
    } else {
        ""
    };
    ctx.out.done(
        format!("Removed profile `{name}`{revoked}"),
        json!({"profile": name, "revoked": args.revoke}),
    );
    Ok(())
}

pub fn profile(ctx: &Ctx, cmd: ProfileCmd) -> CliResult<()> {
    let mut profiles = Profiles::load()?;
    match cmd {
        ProfileCmd::List => {
            let current = profiles.selected_name(None);
            let list: Vec<Value> = profiles
                .profiles
                .iter()
                .map(|(name, p)| {
                    json!({
                        "name": name,
                        "url": p.url,
                        "username": p.username,
                        "current": *name == current,
                        "tls": tls_json(&p.tls),
                        "auth": auth_json(p.auth, &p.token),
                    })
                })
                .collect();
            ctx.out.show(&json!(list), || {
                let mut table = Table::new(["", "NAME", "URL", "USER", "LOGIN", "TLS"]);
                for (name, p) in &profiles.profiles {
                    let tls = match (&p.tls.pinned_sha256, &p.tls.ca_cert_pem) {
                        (Some(_), _) => "pinned certificate",
                        (None, Some(_)) => "CA certificate",
                        (None, None) => "system trust",
                    };
                    let tls = if p.url.starts_with("http://") {
                        "none (http)"
                    } else {
                        tls
                    };
                    let login = match p.auth {
                        AuthKind::Session => "password".to_string(),
                        AuthKind::ApiKey => format!("API key {}", keys::display(&p.token)),
                    };
                    table.row([
                        if *name == current { "*" } else { "" },
                        name,
                        &p.url,
                        &p.username,
                        &login,
                        tls,
                    ]);
                }
                table.print_or("No saved logins. Log in with `lodestone_cli login <url>`.");
            });
            Ok(())
        }
        ProfileCmd::Use { name } => {
            if !profiles.profiles.contains_key(&name) {
                return Err(CliError::not_found(format!(
                    "There is no profile named `{name}`"
                )));
            }
            profiles.current = Some(name.clone());
            profiles.save()?;
            ctx.out.done(
                format!("Now using profile `{name}`"),
                json!({"profile": name}),
            );
            Ok(())
        }
    }
}
