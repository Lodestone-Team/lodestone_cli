mod uninstall;
mod util;

use std::path::PathBuf;

use tracing::{error, info, warn};

use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};
mod run_core;
mod update_manager;
use run_core::run_lodestone;

use clap::Parser;

/// A simple CLI tool to install, update and run the lodestone core
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// uninstall lodestone
    #[clap(long)]
    pub uninstall: bool,
}

fn setup_tracing() {
    // set up a subscriber that logs formatted tracing events to stdout without colors without setting it as the default

    #[cfg(debug_assertions)]
    {
        let fmt_layer_stdout = tracing_subscriber::fmt::layer()
            // Use a more compact, abbreviated log format
            .compact()
            // Display source code file paths
            .with_file(true)
            // Display source code line numbers
            .with_line_number(true)
            // Display the thread ID an event was recorded on
            .with_thread_ids(false)
            // Don't display the event's target (module path)
            .with_target(true)
            .with_writer(std::io::stdout);

        tracing_subscriber::registry()
            .with(fmt_layer_stdout)
            .with(EnvFilter::from("lodestone_launcher=debug"))
            .init();
    }

    #[cfg(not(debug_assertions))]
    {
        let fmt_layer_stdout = tracing_subscriber::fmt::layer()
            // Use a more compact, abbreviated log format
            .compact()
            // Display source code file paths
            .with_file(false)
            // Display source code line numbers
            .with_line_number(false)
            // Display the thread ID an event was recorded on
            .with_thread_ids(false)
            // Don't display the event's target (module path)
            .with_target(false)
            .without_time()
            .with_writer(std::io::stdout);

        tracing_subscriber::registry()
            // .with(ErrorLayer::default())
            .with(fmt_layer_stdout)
            .with(EnvFilter::from("lodestone_launcher=info"))
            .init();
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    color_eyre::install().unwrap();
    setup_tracing();
    let lodestone_path = util::get_lodestone_path().ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "Failed to get lodestone path. The LODESTONE_PATH environment variable is not set and we couldn't get your home directory"
        )
    }).unwrap();
    std::fs::create_dir_all(&lodestone_path).unwrap();
    if args.uninstall {
        let mut are_you_sure = String::new();
        info!("Are you sure you want to uninstall lodestone? (yes/n)");
        warn!(
            "{}",
            ansi_term::Color::Red.paint(format!(
                "This will delete all your files in the lodestone directory {}",
                lodestone_path.display()
            ))
        );
        std::io::stdin().read_line(&mut are_you_sure).unwrap();
        if are_you_sure.trim() == "yes" {
            info!("Uninstalling lodestone...");
            let _ = uninstall::uninstall(&lodestone_path).map_err(|e| {
                error!(
                    "Error uninstalling lodestone: {}, some files may need to be manually removed",
                    e
                );
                e
            });
            info!("Uninstall complete.");
        } else {
            info!("Aborting uninstall, no file changes were made.");
        }
        return;
    }
    let executable_path = update_manager::try_update(&lodestone_path)
        .await
        .map_err(|e| {
            error!("Error updating lodestone: {}, launcher will now crash...", e);
            e
        })
        .unwrap();
    let mut should_run_core = false;
    if PathBuf::from("run_core").is_file() {
        info!("Found run_core file, running lodestone core...");
        should_run_core = true;
    } else {
        let mut input = String::new();
        info!("Would you like to run lodestone core right now? (y/n)");
        std::io::stdin().read_line(&mut input).unwrap();
        if input.trim() == "y" {
            should_run_core = true;
            info!("If you would like the launcher to run lodestone core automatically every time, create a file called 'run_core' in the same directory as the launcher.")
        }
    }
    if should_run_core {
        info!("Starting lodestone...");

        run_lodestone(&executable_path)
            .map_err(|e| {
                error!("Error running lodestone: {}, launcher will now crash...", e);
                e
            })
            .unwrap()
    }
}
