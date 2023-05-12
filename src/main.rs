mod uninstall;
mod util;

use std::{fmt::Display, io::Write, path::PathBuf};

use color_eyre::owo_colors::OwoColorize;
use semver::Version;

mod run_core;
mod update_manager;
use run_core::run_lodestone;

use clap::Parser;

use crate::update_manager::versions::get_current_version;

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

/// A simple CLI tool to install, update and run the lodestone core
#[derive(Parser, Debug)]
#[command(author, about, long_about = None)]
struct Args {
    /// Uninstall lodestone
    #[clap(long, short)]
    pub uninstall: bool,
    /// Install a specific version of lodestone.
    /// If not specified, the latest version will be installed
    #[clap(long, short)]
    pub version: Option<Version>,
    /// Say yes to all prompts.
    /// Bypasses pre-release confirmation, downgrade confirmation, dirty installation confirmation, and uninstall confirmation
    #[clap(long, short)]
    pub yes_all: bool,
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

#[tokio::main]
async fn main() {
    // setup_tracing();
    let args = Args::parse();
    color_eyre::install()
        .map_err(|e| println!("{:#?}", e))
        .unwrap();
    if let Some(v) = args.version.as_ref() {
        info!(
            "You chose to install a specific version of lodestone core ({}). {}",
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
            "If you want to install the latest version, run the command without the --version flag"
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
        if !v.pre.is_empty() {
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
    let lodestone_path = util::get_lodestone_path().ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "Failed to get lodestone path. The LODESTONE_PATH environment variable is not set and we couldn't get your home directory"
        )
    }).unwrap();
    std::fs::create_dir_all(&lodestone_path).unwrap();
    if args.uninstall {
        info!(
            "{}",
            format!(
                "This will delete all your files in the lodestone directory {}",
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
    let executable_path = update_manager::try_update(&lodestone_path, args.version, args.yes_all)
        .await
        .map_err(|e| {
            error!(
                "{}: {}, launcher will now crash...",
                "Error updating lodestone".bold().red(),
                e
            );
            e
        })
        .unwrap();
    if let Some(executable_path) = executable_path {
        let run_core_file_exists = PathBuf::from("run_core").is_file();
        if run_core_file_exists
            || prompt_for_confirmation(
                format!(
                    "Would you like to run lodestone core right now? {}:",
                    "(y/n)".magenta().bold()
                ),
                |input| input.trim() == "y" || input.trim() == "yes",
            )
        {
            info!("Starting lodestone...");
            info!("If you would like to run lodestone automatically, create a file called '{}' in the launcher directory", "run_core".bold().blue());
            run_lodestone(&executable_path)
                .map_err(|e| {
                    error!("Error running lodestone: {}, launcher will now crash...", e);
                    e
                })
                .unwrap()
        }
    } else {
        info!("No lodestone core executable found, launcher will now exit...")
    }
}
