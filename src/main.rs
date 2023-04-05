mod util;
use std::env;
use tracing_subscriber::{EnvFilter, FmtSubscriber};
mod run_core;
mod update_manager;
use run_core::run_lodestone;

#[tokio::main]
async fn main() {
    color_eyre::install().unwrap();
    env::set_var("RUST_LOG", "warn");
    let env_filter = EnvFilter::from_default_env()
        .add_directive(tracing::level_filters::LevelFilter::INFO.into());
    let subscriber = FmtSubscriber::builder()
        .with_env_filter(env_filter)
        .finish();

    let _ = tracing::subscriber::set_global_default(subscriber);

    let executable_path = update_manager::try_update().await.unwrap();
    run_lodestone(&executable_path).unwrap();
    // TODO: implement backup and recovering from crashes
    // TODO: write logs to a file
}
