//! Saved logins ("profiles") in `<config dir>/lodestone_cli/profiles.json`.
//!
//! The file holds login tokens, so it is written atomically (a temporary file in the same
//! folder, then a rename) and, on Unix, readable by its owner only (mode 0600, folder 0700).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{kind, CliError, CliResult};

/// Overrides the folder that holds `profiles.json`, for tests and portable setups.
pub const CONFIG_DIR_ENV: &str = "LODESTONE_CLI_CONFIG_DIR";

/// How the CLI checks the TLS certificate of a profile's core.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsSettings {
    /// Accept only the certificate with this SHA-256 fingerprint (colon-separated
    /// uppercase hex), whoever signed it. Set on login by trust on first use or
    /// `--fingerprint`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_sha256: Option<String>,
    /// Trust only certificates signed by these CA certificates (PEM), from `--ca-cert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert_pem: Option<String>,
}

/// What a profile's token is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// A session token from a password login (profiles from before API keys are these).
    #[default]
    Session,
    /// An API key (`lsk_...`) from `login --api-key`.
    ApiKey,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// The core's base URL, such as `https://192.168.1.20:16662`, without `/api/v1`.
    pub url: String,
    /// The session token or API key. Never printed, not even by `Debug`.
    pub token: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub tls: TlsSettings,
    #[serde(default)]
    pub auth: AuthKind,
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let token = match self.auth {
            AuthKind::ApiKey => super::keys::display(&self.token),
            AuthKind::Session => "…".to_string(),
        };
        f.debug_struct("Profile")
            .field("url", &self.url)
            .field("token", &token)
            .field("username", &self.username)
            .field("uid", &self.uid)
            .field("tls", &self.tls)
            .field("auth", &self.auth)
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

pub const DEFAULT_PROFILE: &str = "default";

/// The folder of `profiles.json`.
pub fn config_dir() -> CliResult<PathBuf> {
    if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    dirs::config_dir()
        .map(|dir| dir.join("lodestone_cli"))
        .ok_or_else(|| {
            CliError::new(
                kind::IO,
                format!("Could not find the user config folder; set {CONFIG_DIR_ENV}"),
            )
        })
}

pub fn profiles_path() -> CliResult<PathBuf> {
    Ok(config_dir()?.join("profiles.json"))
}

impl Profiles {
    pub fn load() -> CliResult<Self> {
        Self::load_from(&profiles_path()?)
    }

    pub fn load_from(path: &Path) -> CliResult<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                CliError::new(
                    kind::IO,
                    format!("{} is not a valid profiles file: {e}", path.display()),
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CliError::io(
                format!("Failed to read {}", path.display()),
                e,
            )),
        }
    }

    pub fn save(&self) -> CliResult<()> {
        self.save_to(&profiles_path()?)
    }

    pub fn save_to(&self, path: &Path) -> CliResult<()> {
        let text = serde_json::to_string_pretty(self).expect("profiles serialize");
        write_private_file_atomic(path, text.as_bytes())
    }

    /// The name of the profile to use: `requested`, else the current one, else the only
    /// one, else `default`.
    pub fn selected_name(&self, requested: Option<&str>) -> String {
        if let Some(name) = requested {
            return name.to_string();
        }
        if let Some(current) = &self.current {
            return current.clone();
        }
        if self.profiles.len() == 1 {
            if let Some(name) = self.profiles.keys().next() {
                return name.clone();
            }
        }
        DEFAULT_PROFILE.to_string()
    }
}

/// Writes `contents` to `path` through a temporary file in the same folder and a rename,
/// so readers see the old or the new file and never a partial one. On Unix the file is
/// created with mode 0600 and its folder with mode 0700.
pub fn write_private_file_atomic(path: &Path, contents: &[u8]) -> CliResult<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "profiles.json".to_string());
    let temp_path = dir.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        #[cfg(unix)]
        {
            // the mode above only applies to new files
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp_path, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(CliError::io(
            format!("Failed to write {}", path.display()),
            e,
        ));
    }
    Ok(())
}

fn create_private_dir(dir: &Path) -> CliResult<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|e| CliError::io(format!("Failed to create {}", dir.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(url: &str) -> Profile {
        Profile {
            url: url.to_string(),
            token: "secret-token".to_string(),
            username: "owner".to_string(),
            uid: "uid".to_string(),
            tls: TlsSettings {
                pinned_sha256: Some("AB:CD".to_string()),
                ca_cert_pem: None,
            },
            auth: AuthKind::Session,
        }
    }

    #[test]
    fn old_profiles_are_sessions_and_debug_hides_tokens() {
        let old: Profile =
            serde_json::from_str(r#"{"url": "https://h", "token": "jwt-secret"}"#).unwrap();
        assert_eq!(old.auth, AuthKind::Session);
        assert!(!format!("{old:?}").contains("jwt-secret"));
        let key = Profile {
            token: "lsk_ab12cd34ef56_c2VjcmV0c2VjcmV0c2VjcmV0".into(),
            auth: AuthKind::ApiKey,
            ..old
        };
        let debug = format!("{key:?}");
        assert!(debug.contains("lsk_ab12cd34ef56_…"), "{debug}");
        assert!(!debug.contains("c2Vj"), "{debug}");
        assert_eq!(
            serde_json::to_value(&key).unwrap()["auth"],
            serde_json::json!("api_key")
        );
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("profiles.json");
        assert_eq!(Profiles::load_from(&path).unwrap(), Profiles::default());
        let mut profiles = Profiles::default();
        profiles
            .profiles
            .insert("home".to_string(), profile("https://h:16662"));
        profiles.current = Some("home".to_string());
        profiles.save_to(&path).unwrap();
        assert_eq!(Profiles::load_from(&path).unwrap(), profiles);
        // no temporary file is left behind
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["profiles.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg").join("profiles.json");
        Profiles::default().save_to(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);

        // an existing file that others could read is made private on the next save
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        Profiles::default().save_to(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn selected_name() {
        let mut profiles = Profiles::default();
        assert_eq!(profiles.selected_name(None), "default");
        profiles.profiles.insert("a".into(), profile("https://a"));
        assert_eq!(profiles.selected_name(None), "a");
        profiles.profiles.insert("b".into(), profile("https://b"));
        assert_eq!(profiles.selected_name(None), "default");
        profiles.current = Some("b".into());
        assert_eq!(profiles.selected_name(None), "b");
        assert_eq!(profiles.selected_name(Some("a")), "a");
    }

    #[test]
    fn invalid_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(Profiles::load_from(&path).is_err());
    }
}
