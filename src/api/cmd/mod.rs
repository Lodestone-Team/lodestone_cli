//! The implementation of each API subcommand.

mod auth;
mod console;
mod core;
mod files;
mod instance;
mod macros;
mod misc;
mod mods;
mod users;
pub(crate) mod values;

use super::cli::Command;
use super::error::CliResult;
use super::Ctx;

pub async fn dispatch(ctx: &Ctx, command: Command) -> CliResult<()> {
    match command {
        Command::Login(args) => auth::login(ctx, args).await,
        Command::Logout(args) => auth::logout(ctx, args).await,
        Command::Profile(cmd) => auth::profile(ctx, cmd),
        Command::Core(cmd) => core::run(ctx, cmd).await,
        Command::Instance(cmd) => instance::run(ctx, cmd).await,
        Command::Console(args) => console::console(ctx, args).await,
        Command::Players(args) => console::players(ctx, args).await,
        Command::Monitor(args) => console::monitor(ctx, args).await,
        Command::Files(args) => files::run(ctx, args).await,
        Command::Macros(args) => macros::run(ctx, args).await,
        Command::Mods(args) => mods::run(ctx, args).await,
        Command::Modrinth(cmd) => mods::modrinth(ctx, cmd).await,
        Command::Users(cmd) => users::run(ctx, cmd).await,
        Command::Events(args) => misc::events(ctx, args).await,
        Command::Playit(cmd) => misc::playit(ctx, cmd).await,
        Command::Tasks(args) => misc::tasks(ctx, args).await,
        Command::Extension(cmd) => misc::extension(ctx, cmd).await,
    }
}
