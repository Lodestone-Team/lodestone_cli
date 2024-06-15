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

/// A simple CLI tool to install, update and run the Lodestone Core
#[derive(Parser, Debug, Default, Serialize, Deserialize)]
#[command(author, about, long_about = None)]
struct Args {
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
        let latest = releases[0].asset_for(&bin_name, None).unwrap();
        let latest_sem_ver: semver::Version = VersionWithV::from_str(&releases[0].version)?.into();
        let current_sem_ver = VERSION.with(|v| v.clone());
        if latest_sem_ver <= current_sem_ver {
            info!("CLI is up to date");
            return Ok(());
        } else {
            info!(
                "Updating CLI from {current} to {latest}",
                latest = latest_sem_ver,
                current = current_sem_ver
            );
        }

        // create temp dir under the current directory
        let temp_dir = tempfile::tempdir_in(std::env::current_dir()?)?;
        let bin_path = temp_dir.path().join(&bin_name);
        let bin_file = File::create(&bin_path)?;
        self_update::Download::from_url(&latest.download_url)
            .set_header(
                reqwest::header::ACCEPT,
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
    .await
    .unwrap()
}

#[tokio::main]
async fn main() {
    // setup_tracing();
    let _ = color_eyre::install().map_err(|e| error!("color eyre install error {e}"));

    info!("Lodestone CLI v{}", VERSION.with(|v| v.to_string()));

    if let Err(e) = self_update().await {
        error!("Failed to self update: {e}");
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
        error!("cli will now exit");
        std::process::exit(1);
    }
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
