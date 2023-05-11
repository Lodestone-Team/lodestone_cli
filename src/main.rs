mod util;

use tracing::{error, info};

use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
};
mod run_core;
mod update_manager;
use run_core::run_lodestone;

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
    color_eyre::install().unwrap();
    setup_tracing();

    let executable_path = update_manager::try_update()
        .await
        .map_err(|e| {
            error!("Error updating lodestone: {}, updater will now crash...", e);
            e
        })
        .unwrap();
    info!("Starting lodestone...");
    run_lodestone(&executable_path)
        .map_err(|e| {
            error!("Error running lodestone: {}, updater will now crash...", e);
            e
        })
        .unwrap()
    // TODO: implement backup and recovering from crashes
    // TODO: write logs to a file
}
