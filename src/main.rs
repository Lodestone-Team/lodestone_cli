mod api;
mod uninstall;
mod util;
mod versions;
use color_eyre::eyre::Result;
use color_eyre::owo_colors::OwoColorize;
use util::executable_name_without_version;

use std::{env, fmt::Display, fs::File, io::Write, path::PathBuf, str::FromStr};
use versions::VersionWithV;

mod run_core;
mod update_manager;
use run_core::run_lodestone;

use clap::Parser;
use serde::{Deserialize, Serialize};

use versions::get_current_version;

// parse from cargo.toml
thread_local! {
    pub static VERSION: semver::Version = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
}

// an info! macro that append the prefix "[i].green()" to the message
macro_rules! info {
    ($($arg:tt)*) => ({
        println!("{prefix} {}", format_args!($($arg)*), prefix = "[i]".green());
    })
}

#[macro_export]
// an warn! macro that append the prefix "[!!]".yellow() to the message
macro_rules! warn {
    ($($arg:tt)*) => ({
        println!("{prefix} {}", format_args!($($arg)*), prefix = "[!!]".yellow());
    })
}

// an error! macro that append the prefix "[!!!]".red() to the message
macro_rules! error {
    ($($arg:tt)*) => ({
        println!("{prefix} {}", format_args!($($arg)*), prefix = "[!!!]".red());
    })
}

pub(crate) use {error, info};

/// A simple CLI tool to install, update and run the Lodestone Core, and to manage a
/// running core: servers, mods, files, users and more.
///
/// Without a subcommand, it installs or updates Lodestone Core (and runs it with
/// --run-core). With a subcommand, it talks to a running core; see `login` first.
#[derive(Parser, Debug, Default, Serialize, Deserialize)]
#[command(author, about, long_about = None)]
struct Args {
    /// Manage a running core. Without a subcommand, install, update or run the core.
    #[command(subcommand)]
    #[serde(skip)]
    pub command: Option<api::cli::Command>,
    #[command(flatten)]
    #[serde(skip)]
    pub api: api::cli::GlobalOpts,
    /// Uninstall lodestone
    #[clap(long, short)]
    #[serde(default)]
    pub uninstall: bool,
    /// Install a specific version of lodestone.
    ///
    /// If not specified, the latest version will be installed
    #[clap(long, short)]
    pub version: Option<VersionWithV>,
    /// Say yes to all prompts.
    ///
    /// Bypasses pre-release confirmation, downgrade confirmation, dirty installation confirmation, and uninstall confirmation
    #[clap(long, short)]
    #[serde(default)]
    pub yes_all: bool,
    /// Tells the cli where to install lodestone.
    ///
    /// If not specified, the cli will install lodestone in ~/.lodestone
    ///
    /// This will set the LODESTONE_PATH environment variable for the current running process
    #[clap(long, short)]
    pub install_path: Option<PathBuf>,
    /// Skip ALL update check to GitHub, and try to use the local version of core if possible
    ///
    /// If the local version is not available, the cli will try to download the latest version from GitHub
    #[clap(long, short)]
    #[serde(default)]
    pub skip_update_check: bool,
    /// Run Lodestone Core automatically
    #[clap(long, short)]
    #[serde(default)]
    pub run_core: bool,
    /// List all available versions of lodestone
    #[clap(long, short)]
    #[serde(default)]
    pub list_versions: bool,
}

impl Args {
    pub fn merge(&mut self, other: Self) {
        if let Some(version) = other.version {
            self.version = Some(version);
        }
        if let Some(install_path) = other.install_path {
            self.install_path = Some(install_path);
        }
        self.uninstall |= other.uninstall;
        self.yes_all |= other.yes_all;
        self.skip_update_check |= other.skip_update_check;
        self.run_core |= other.run_core;
        self.list_versions |= other.list_versions;
    }
}

fn read_args_from_file() -> Option<Args> {
    serde_json::from_reader(std::fs::File::open("args.json").ok()?).ok()
}

fn prompt_for_confirmation(message: impl Display, predicate: impl FnOnce(String) -> bool) -> bool {
    print!("{message}");
    std::io::stdout().flush().unwrap();
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .expect("Failed to read line");
    predicate(input)
}

/// Whether the CLI should look for a newer version of itself on GitHub first: not with
/// --skip-update-check, which promises no update checks at all, and not to list the core's
/// versions or uninstall, which don't need a newer CLI.
fn should_self_update(args: &Args) -> bool {
    !args.skip_update_check && !args.list_versions && !args.uninstall
}

/// The CLI release to update to, newest first as GitHub lists them, and its build for this
/// platform (`bin_name`): `None` when `current` is up to date. A release without a build
/// for this platform (one platform's build can fail while the others publish) is skipped
/// for the newest one that has it; with none at all this is an error, not a panic.
fn cli_update(
    releases: &[self_update::update::Release],
    bin_name: &str,
    current: &semver::Version,
) -> Result<Option<(semver::Version, self_update::update::ReleaseAsset)>> {
    let (release, asset) = releases
        .iter()
        .find_map(|release| {
            release
                .asset_for(bin_name, None)
                .map(|asset| (release, asset))
        })
        .ok_or_else(|| {
            color_eyre::eyre::eyre!("no lodestone_cli release has a build named {bin_name}")
        })?;
    let version: semver::Version = VersionWithV::from_str(&release.version)?.into();
    Ok((version > *current).then_some((version, asset)))
}

async fn self_update() -> Result<()> {
    let bin_name = format!(
        "lodestone_cli_{os}_{aarch}",
        os = env::consts::OS,
        aarch = env::consts::ARCH
    );
    tokio::task::spawn_blocking(move || {
        let releases = self_update::backends::github::ReleaseList::configure()
            .repo_owner("Lodestone-Team")
            .repo_name("lodestone_cli")
            .build()?
            .fetch()?;
        let current_sem_ver = VERSION.with(|v| v.clone());
        let Some((latest_sem_ver, latest)) = cli_update(&releases, &bin_name, &current_sem_ver)?
        else {
            info!("CLI is up to date");
            return Ok(());
        };
        info!(
            "Updating CLI from {current} to {latest}",
            latest = latest_sem_ver,
            current = current_sem_ver
        );

        // create temp dir under the current directory
        let temp_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
        let bin_path = temp_dir.path().join(&bin_name);
        let bin_file = File::create(&bin_path)?;
        self_update::Download::from_url(&latest.download_url)
            .set_header(
                http::header::ACCEPT,
                "application/octet-stream".parse().unwrap(),
            )
            .show_progress(true)
            .download_to(bin_file)?;
        #[cfg(not(windows))]
        {
            use std::os::unix::prelude::PermissionsExt;
            let mut permissions = std::fs::metadata(&bin_path)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&bin_path, permissions)?;
        }

        self_replace::self_replace(&bin_path)?;
        info!("CLI updated successfully, changes will take effect after restart");
        Ok(())
    })
    // a panic in the update is reported like any other failure, and the run goes on
    .await?
}

/// Parses the command line. Errors in a subcommand's arguments are printed like other
/// subcommand errors (as JSON with `--json`) and exit with the usage code 2; help and
/// version requests print as usual.
fn parse_args() -> Args {
    match Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            use clap::error::ErrorKind;
            let is_subcommand = env::args().skip(1).any(|a| !a.starts_with('-'));
            let json = env::args().any(|a| a == "--json");
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion)
                || !is_subcommand
                || !json
            {
                e.exit();
            }
            std::process::exit(api::report_usage_error(json, &e.to_string()));
        }
    }
}

#[tokio::main]
async fn main() {
    let parsed = parse_args();
    if let Some(command) = parsed.command {
        std::process::exit(api::run(parsed.api, command).await);
    }
    if parsed.api.any_set() {
        error!("--json, --profile, --yes and --verbose only apply to subcommands (see --help)");
        std::process::exit(2);
    }

    // setup_tracing();
    let _ = color_eyre::install().map_err(|e| error!("color eyre install error {e}"));

    info!("Lodestone CLI v{}", VERSION.with(|v| v.to_string()));

    // The args (with args.json) come first: --skip-update-check skips the CLI's own
    // update check too.
    let args = match read_args_from_file() {
        Some(mut args) => {
            info!(
                "{}",
                "Detected a valid args.json file. Performing a merge of command line args and args.json"
            );
            args.merge(Args::parse());
            args
        }
        None => Args::parse(),
    };

    if should_self_update(&args) {
        if let Err(e) = self_update().await {
            error!("Failed to self update: {e}");
        }
    }

    if executable_name_without_version().is_none() {
        error!(
            "{}",
            format!(
                "Your system ({} {}) is not supported by lodestone",
                env::consts::OS,
                env::consts::ARCH
            )
        );
        error!("Please open an issue on github if you think this is a mistake");
        if !prompt_for_confirmation(
            format!(
                "Would you still like to proceed? Weird things may happen! {}",
                "(y/n):".magenta().bold()
            ),
            |s| s.trim() == "y" || s.trim() == "yes",
        ) {
            error!("cli will now exit");
            std::process::exit(1);
        }
        info!("Proceeding with installation with unsupported system");
    }

    if args.list_versions {
        versions::list_versions().await.unwrap();
        return;
    }

    if let Some(path) = args.install_path {
        std::env::set_var("LODESTONE_PATH", path);
    }
    let lodestone_path = util::get_lodestone_path().ok_or_else(|| {
        error!("Could not find lodestone path. We couldn't find your home directory, and you didn't specify a path with the --install-path flag");
        error!("Please specify a path with the '{}' flag", "--install-path".bold().blue());
        error!("cli will now exit");
        std::process::exit(1);
    }).unwrap();
    info!(
        "LODESTONE_PATH={}",
        lodestone_path.to_string_lossy().bold().blue()
    );
    if let Some(v) = args.version.as_ref() {
        info!(
            "You chose to install a specific version of Lodestone Core ({}). {}",
            v.bold().blue(),
            get_current_version().await.ok().map_or_else(
                || "".to_string(),
                |current_version| {
                    format!(
                        "Current version : {}",
                        current_version.to_string().blue().bold()
                    )
                }
            )
        );
        info!(
            "If you want to install the latest version, run the command without the '{}' flag",
            "--version".bold().blue()
        );

        let mut require_confirmation = true;
        if let Ok(current_version) = get_current_version().await {
            if current_version > *v {
                error!(
                    "You are installing an older version of lodestone ({}) than the one you currently have installed ({})",
                    v.bold().blue(), current_version.bold().blue()
                );
                error!(
                    "Note that {} Doing so may cause {}",
                    "we do not support downgrading.".bold().red(),
                    "data loss or corruption".bold().red()
                );
                require_confirmation = true;
            }
        } else {
            warn!(
                "We couldn't find your current version of lodestone, so we can't check if you are downgrading",
            );
            warn!(
                "Note that {} Doing so may cause {}",
                "we do not support downgrading.".bold().yellow(),
                "data loss or corruption".bold().red()
            );
        }
        if !v.0.pre.is_empty() {
            warn!(
                "You are installing a pre-release version of lodestone {},",
                "which may be unstable".bold().yellow()
            );
            require_confirmation = true;
        }
        if !args.yes_all
            && require_confirmation
            && !prompt_for_confirmation(
                format!("Would you like to proceed? {}", "(y/n)".magenta().bold()),
                |s| s.trim() == "y" || s.trim() == "yes",
            )
        {
            info!("Aborting installation, no file changes were made.",);
            return;
        }
    }

    std::fs::create_dir_all(&lodestone_path).unwrap();
    if args.uninstall {
        warn!(
            "{}",
            format!(
                "This will delete the directory and all files in it: {}",
                lodestone_path.display()
            )
            .bold()
            .red()
        );
        if !args.yes_all
            && prompt_for_confirmation(
                format!(
                    "Are you sure you want to uninstall lodestone? {}:",
                    "(yes/n)".bold().magenta()
                ),
                |input| input.trim() == "yes",
            )
        {
            info!("Uninstalling lodestone...");
            if let Err(e) = uninstall::uninstall(&lodestone_path) {
                error!(
                    "Error uninstalling lodestone: {}, some files may need to be manually removed",
                    e
                );
            } else {
                info!("Uninstalled lodestone successfully");
            }
        } else {
            info!("Aborting uninstall, no file changes were made.");
        }
        return;
    }
    let executable_path = update_manager::try_update(
        &lodestone_path,
        args.version,
        args.yes_all,
        args.skip_update_check,
    )
    .await
    .map_err(|e| {
        error!(
            "{}: {}, cli will now crash...",
            "Error updating lodestone".bold().red(),
            e
        );
        e
    })
    .unwrap();
    if let Some(executable_path) = executable_path {
        if args.run_core
            || prompt_for_confirmation(
                format!(
                    "Would you like to run Lodestone Core right now? {}:",
                    "(y/n)".magenta().bold()
                ),
                |input| input.trim() == "y" || input.trim() == "yes",
            )
        {
            info!("Starting lodestone...");
            if !args.run_core {
                info!(
                    "If you would like to run lodestone automatically, pass in the '{}' flag",
                    "run-core".bold().blue()
                );
            }
            run_lodestone(&executable_path)
                .map_err(|e| {
                    error!("Error running lodestone: {}, cli will now crash...", e);
                    e
                })
                .unwrap()
        }
    } else {
        info!("No Lodestone Core executable found, cli will now exit...")
    }
}

#[cfg(test)]
mod parse_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use self_update::update::{Release, ReleaseAsset};

    fn release(version: &str, assets: &[&str]) -> Release {
        Release {
            version: version.to_string(),
            assets: assets
                .iter()
                .map(|name| ReleaseAsset {
                    name: name.to_string(),
                    download_url: format!("https://example.invalid/{version}/{name}"),
                })
                .collect(),
            ..Release::default()
        }
    }

    #[test]
    fn a_release_without_this_platforms_build_is_skipped() {
        let current = semver::Version::new(1, 0, 4);
        let mac_intel = "lodestone_cli_macos_x86_64";
        // the newest release's Intel Mac build failed (LS-70)
        let releases = [
            release("v1.0.6", &["lodestone_cli_macos_aarch64"]),
            release("v1.0.5", &["lodestone_cli_macos_aarch64", mac_intel]),
        ];
        let (version, asset) = cli_update(&releases, mac_intel, &current).unwrap().unwrap();
        assert_eq!(version, semver::Version::new(1, 0, 5));
        assert_eq!(asset.name, mac_intel);
        // up to date
        let newest = semver::Version::new(1, 0, 5);
        assert!(cli_update(&releases, mac_intel, &newest).unwrap().is_none());
        // no build for this platform at all, or no releases: an error, not a panic
        assert!(cli_update(&releases, "lodestone_cli_windows_aarch64", &current).is_err());
        assert!(cli_update(&[], mac_intel, &current).is_err());
    }

    #[test]
    fn skip_update_check_skips_the_cli_update_too() {
        let parse = |words: &[&str]| {
            Args::try_parse_from(std::iter::once("lodestone_cli").chain(words.iter().copied()))
                .unwrap()
        };
        assert!(should_self_update(&parse(&[])));
        assert!(should_self_update(&parse(&["--run-core"])));
        // LS-71
        assert!(!should_self_update(&parse(&[
            "--skip-update-check",
            "--run-core"
        ])));
        assert!(!should_self_update(&parse(&["--list-versions"])));
        assert!(!should_self_update(&parse(&["--uninstall"])));
        // from args.json too
        let mut args = parse(&["--run-core"]);
        args.merge(serde_json::from_str(r#"{"skip_update_check": true}"#).unwrap());
        assert!(!should_self_update(&args));
    }
}
