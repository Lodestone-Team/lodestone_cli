//! The API subcommands: everything the dashboard can do, from the command line.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

/// Options of every API subcommand.
#[derive(Args, Debug, Clone, Default)]
pub struct GlobalOpts {
    /// Print exactly one JSON document (NDJSON for streams) instead of text.
    /// Errors go to stderr as {"error": {"kind", "message"}}.
    #[arg(long, global = true)]
    pub json: bool,
    /// The saved login to use (see `profile list`).
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,
    /// Do not ask before destructive actions (deleting, killing, removing).
    #[arg(long, global = true)]
    pub yes: bool,
    /// Print the requests sent to stderr (never the token).
    #[arg(long, global = true)]
    pub verbose: bool,
}

impl GlobalOpts {
    pub fn any_set(&self) -> bool {
        self.json || self.profile.is_some() || self.yes || self.verbose
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Log in to a core and save the login as a profile.
    Login(LoginArgs),
    /// Forget a saved login.
    Logout(LogoutArgs),
    /// List and choose saved logins.
    #[command(subcommand)]
    Profile(ProfileCmd),
    /// The core: info, global settings, system usage, first-time setup.
    #[command(subcommand)]
    Core(CoreCmd),
    /// Servers: list, create, start, stop, settings, delete.
    #[command(subcommand, name = "server", visible_alias = "instance")]
    Instance(InstanceCmd),
    /// Show a server's console output, follow it, or send a command.
    Console(ConsoleArgs),
    /// List the players on a server.
    Players(PlayersArgs),
    /// CPU and memory usage of a server.
    Monitor(MonitorArgs),
    /// Files of a server, or of the core's machine with --global.
    Files(FilesArgs),
    /// Macros of a server (owner only).
    Macros(MacrosArgs),
    /// Mods or plugins of a server, from Modrinth or uploaded jars.
    Mods(ModsArgs),
    /// Search Modrinth (through the core) without a server, for modpacks for example.
    #[command(subcommand)]
    Modrinth(ModrinthCmd),
    /// Users and their permissions.
    #[command(subcommand)]
    Users(UsersCmd),
    /// API keys: named, limited, expiring credentials for scripts and agents. Needs a
    /// password login.
    #[command(subcommand, name = "api-keys")]
    ApiKeys(ApiKeysCmd),
    /// Notifications (events), or the live event stream with --follow.
    Events(EventsArgs),
    /// The playit.gg tunnel (owner only).
    #[command(subcommand)]
    Playit(PlayitCmd),
    /// Background tasks such as creating a server; --wait blocks until one ends.
    Tasks(TasksArgs),
    /// Extensions (owner only).
    #[command(subcommand)]
    Extension(ExtensionCmd),
}

/* ---------------------------------------------------------------- login */

#[derive(Args, Debug, Clone)]
pub struct ConnectArgs {
    /// The user name (or LODESTONE_USERNAME); asked for if missing.
    #[arg(long)]
    pub username: Option<String>,
    /// Read the password from the first line of stdin (or set LODESTONE_PASSWORD).
    #[arg(long)]
    pub password_stdin: bool,
    /// Trust the core's certificate if its SHA-256 fingerprint is this (from the core's
    /// log or Core Settings). The profile then accepts only that certificate.
    #[arg(long, value_name = "SHA256")]
    pub fingerprint: Option<String>,
    /// Trust certificates signed by this CA certificate (PEM file).
    #[arg(long, value_name = "FILE", conflicts_with = "fingerprint")]
    pub ca_cert: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
pub struct LoginArgs {
    /// The core's address: host, host:port or a URL. Defaults to https and port 16662.
    pub url: String,
    /// Log in with an API key instead of a password. The key is read from
    /// LODESTONE_API_KEY, else from stdin (asked for without echo on a terminal); it is
    /// never a command line argument.
    #[arg(long, conflicts_with_all = ["username", "password_stdin"])]
    pub api_key: bool,
    #[command(flatten)]
    pub connect: ConnectArgs,
}

#[derive(Args, Debug, Clone)]
pub struct LogoutArgs {
    /// Also end every session of this user on the core (all devices, the dashboard too).
    #[arg(long)]
    pub revoke: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ProfileCmd {
    /// List saved logins.
    List,
    /// Make a saved login the default.
    Use { name: String },
}

/* ----------------------------------------------------------------- core */

#[derive(Subcommand, Debug, Clone)]
pub enum CoreCmd {
    /// Version, host, hardware and TLS of the core. Needs no login.
    Info,
    /// The core's global settings.
    #[command(subcommand)]
    Settings(CoreSettingsCmd),
    /// RAM, CPU, disk, OS and uptime of the core's machine.
    System,
    /// First-time setup: create the owner account with the setup key from the core's log.
    Setup(SetupArgs),
    /// The game types servers can be created for.
    Games,
    /// Whether a port is free on the core's machine.
    CheckPort { port: u16 },
    /// Whether a server name is already used.
    CheckName { name: String },
    /// Open a port on the router with UPnP (owner only).
    OpenPort { port: u16 },
}

#[derive(Args, Debug, Clone)]
pub struct SetupArgs {
    /// The core's address, as for `login`.
    pub url: String,
    /// The setup key printed in the core's log; asked for if missing.
    pub key: Option<String>,
    #[command(flatten)]
    pub connect: ConnectArgs,
}

#[derive(Subcommand, Debug, Clone)]
pub enum CoreSettingsCmd {
    /// Show the global settings.
    Get,
    /// Change a global setting (owner only).
    Set {
        key: CoreSettingKey,
        /// The new value: text for name and domain (empty clears the domain),
        /// true/false for the others.
        value: String,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreSettingKey {
    /// The core's name.
    #[value(alias = "core_name", alias = "core-name")]
    Name,
    /// Safe mode: users other than the owner cannot change protected things.
    #[value(alias = "safe_mode")]
    SafeMode,
    /// The code execution kill switch (macros, atoms, scripts).
    #[value(alias = "code_execution")]
    CodeExecution,
    /// The domain shown for the core.
    Domain,
    /// Accept plain HTTP from other devices.
    #[value(alias = "allow_lan_http")]
    AllowLanHttp,
    /// playit.gg integration.
    #[value(alias = "playit_enabled", alias = "playit-enabled")]
    Playit,
}

/* ------------------------------------------------------------- instance */

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Vanilla,
    Fabric,
    Forge,
    Paper,
}

impl Flavour {
    /// The `HandlerGameType` the core expects in `/instance/create/{game_type}`.
    pub fn game_type(self) -> &'static str {
        match self {
            Flavour::Vanilla => "MinecraftJavaVanilla",
            Flavour::Fabric => "MinecraftFabric",
            Flavour::Forge => "MinecraftForge",
            Flavour::Paper => "MinecraftPaper",
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum InstanceCmd {
    /// List the servers you can see.
    List,
    /// Details of a server.
    Info {
        #[arg(value_name = "SERVER")]
        instance: String,
    },
    /// The state of a server: Starting, Running, Stopping, Stopped or Error.
    State {
        #[arg(value_name = "SERVER")]
        instance: String,
    },
    /// The Minecraft versions a server of a flavour can be created with.
    Versions { flavour: Flavour },
    /// Create a Minecraft server. Without options on a terminal, asks for each setting.
    Create(CreateArgs),
    /// Create a Fabric server from a Modrinth modpack (link, version id, project) or a
    /// .mrpack file.
    CreateModpack(CreateModpackArgs),
    /// Create an atom (generic) server from a URL (owner only, needs code execution).
    CreateAtom(CreateAtomArgs),
    /// Add a Docker container to Lodestone's watch list.
    DockerWatch {
        /// The container's name.
        container: String,
    },
    /// Start a server. Starting a Minecraft server accepts Mojang's EULA.
    Start(PowerArgs),
    /// Stop a server.
    Stop(PowerArgs),
    /// Restart a server.
    Restart(PowerArgs),
    /// Kill a server's process without saving.
    Kill {
        #[arg(value_name = "SERVER")]
        instance: String,
    },
    /// Delete a server and all its files. It must be stopped.
    Delete {
        #[arg(value_name = "SERVER")]
        instance: String,
    },
    /// Rename a server.
    Rename {
        #[arg(value_name = "SERVER")]
        instance: String,
        name: String,
    },
    /// Change a server's description.
    Description {
        #[arg(value_name = "SERVER")]
        instance: String,
        description: String,
    },
    /// Change the Minecraft version of a server.
    Version {
        #[arg(value_name = "SERVER")]
        instance: String,
        version: String,
    },
    /// A server's settings: command line, RAM and server.properties.
    #[command(subcommand)]
    Settings(InstanceSettingsCmd),
}

#[derive(Args, Debug, Clone)]
pub struct PowerArgs {
    /// The server: its name or UUID.
    #[arg(value_name = "SERVER")]
    pub instance: String,
    /// Wait until the server is Running (start, restart) or Stopped (stop).
    #[arg(long)]
    pub wait: bool,
    /// With --wait, give up after this many seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 600)]
    pub timeout: u64,
}

#[derive(Args, Debug, Clone, Default)]
pub struct NewInstanceOpts {
    /// The server's name.
    #[arg(long)]
    pub name: Option<String>,
    /// The server port. Default: the first free port from 25565.
    #[arg(long)]
    pub port: Option<u16>,
    /// Minimum RAM in MB.
    #[arg(long, value_name = "MB")]
    pub min_ram: Option<u32>,
    /// Maximum RAM in MB.
    #[arg(long, value_name = "MB")]
    pub max_ram: Option<u32>,
    #[arg(long)]
    pub description: Option<String>,
    /// Start the server when the core starts.
    #[arg(long)]
    pub auto_start: bool,
    /// Restart the server when it crashes.
    #[arg(long)]
    pub restart_on_crash: bool,
    /// Wait until the server is created (see `tasks --wait`).
    #[arg(long)]
    pub wait: bool,
    /// With --wait, give up after this many seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 1800)]
    pub timeout: u64,
}

#[derive(Args, Debug, Clone)]
pub struct CreateArgs {
    #[arg(long, alias = "flavor")]
    pub flavour: Option<Flavour>,
    /// The Minecraft version, or `latest`. See `server versions <flavour>`.
    #[arg(long)]
    pub version: Option<String>,
    /// Extra command line arguments for the server (needs global file permission).
    #[arg(long, allow_hyphen_values = true)]
    pub cmd_args: Option<String>,
    /// Ask for every setting, even those given as options.
    #[arg(long, short = 'i')]
    pub interactive: bool,
    #[command(flatten)]
    pub opts: NewInstanceOpts,
}

#[derive(Args, Debug, Clone)]
pub struct CreateModpackArgs {
    /// A Modrinth link, a modpack version id, a modpack project (slug or id), or a
    /// local .mrpack file.
    pub source: String,
    /// For a project: the pack version (version number or id). Default: newest release.
    #[arg(long, value_name = "VERSION")]
    pub pack_version: Option<String>,
    /// Only show what the pack contains; create nothing.
    #[arg(long)]
    pub inspect: bool,
    #[command(flatten)]
    pub opts: NewInstanceOpts,
}

#[derive(Args, Debug, Clone)]
pub struct CreateAtomArgs {
    /// The atom's URL.
    #[arg(long)]
    pub url: String,
    /// A JSON file with the atom's `setting_sections`.
    #[arg(long, value_name = "FILE")]
    pub settings_json: Option<PathBuf>,
    #[command(flatten)]
    pub opts: NewInstanceOpts,
}

#[derive(Subcommand, Debug, Clone)]
pub enum InstanceSettingsCmd {
    /// Show every setting with its value and type.
    Get {
        #[arg(value_name = "SERVER")]
        instance: String,
    },
    /// Change a setting. KEY is a setting id (such as `max_ram` or `max-players`),
    /// `section.setting`, or `auto_start` / `restart_on_crash` with true or false.
    Set {
        #[arg(value_name = "SERVER")]
        instance: String,
        key: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
}

/* -------------------------------------------------------------- console */

#[derive(Args, Debug, Clone)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct ConsoleArgs {
    #[command(subcommand)]
    pub action: Option<ConsoleCmd>,
    #[command(flatten)]
    pub show: ConsoleShowArgs,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConsoleCmd {
    /// Show recent output (the same as `console <server>`).
    Show(ConsoleShowArgs),
    /// Send a command to the server, such as `say hi` or `op Steve`.
    Send {
        #[arg(value_name = "SERVER")]
        instance: String,
        /// The command; several words are joined with spaces. Put words that start
        /// with `-` after `--`.
        #[arg(required = true, num_args = 1.., allow_negative_numbers = true)]
        command: Vec<String>,
    },
}

#[derive(Args, Debug, Clone)]
pub struct ConsoleShowArgs {
    /// The server: its name or UUID.
    #[arg(required = true, value_name = "SERVER")]
    pub instance: Option<String>,
    /// Keep streaming new output (reconnects if the connection drops).
    #[arg(long, short = 'f')]
    pub follow: bool,
    /// Show only the last N lines of the recent output.
    #[arg(long, short = 'n', value_name = "N")]
    pub lines: Option<usize>,
    /// With --follow, exit successfully once a line contains this text (such as `Done`).
    #[arg(long, value_name = "TEXT", requires = "follow")]
    pub until: Option<String>,
    /// With --follow, give up after this many seconds (exit code 1, kind Timeout).
    #[arg(long, value_name = "SECONDS", requires = "follow")]
    pub timeout: Option<u64>,
}

#[derive(Args, Debug, Clone)]
pub struct PlayersArgs {
    /// The server: its name or UUID.
    #[arg(value_name = "SERVER")]
    pub instance: String,
    /// Set the maximum number of players instead.
    #[arg(long, value_name = "N")]
    pub set_max: Option<u32>,
}

#[derive(Args, Debug, Clone)]
pub struct MonitorArgs {
    /// The server: its name or UUID.
    #[arg(value_name = "SERVER")]
    pub instance: String,
    /// Keep printing a report every second.
    #[arg(long, short = 'f')]
    pub follow: bool,
}

/* ---------------------------------------------------------------- files */

#[derive(Args, Debug, Clone)]
pub struct FilesArgs {
    /// Use the core machine's own file system (absolute paths) instead of a server's.
    #[arg(long)]
    pub global: bool,
    /// The server (name or UUID). Omit it with --global.
    #[arg(
        required_unless_present = "global",
        conflicts_with = "global",
        value_name = "SERVER"
    )]
    pub instance: Option<String>,
    #[command(subcommand)]
    pub action: FilesCmd,
}

#[derive(Subcommand, Debug, Clone)]
pub enum FilesCmd {
    /// List a folder (default: the server's root).
    Ls { path: Option<String> },
    /// Print a text file (at most 16 MiB; download bigger files).
    Cat { path: String },
    /// Download a file, or a folder as a zip. DEST `-` writes to stdout.
    Download {
        path: String,
        /// Where to save it (a file or a folder). Default: the current folder.
        dest: Option<PathBuf>,
    },
    /// Upload local files into a folder.
    Upload {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// The folder to upload into (default: the server's root).
        #[arg(long, value_name = "DIR")]
        to: Option<String>,
    },
    /// Replace a file's contents with stdin, or with a local file.
    Write {
        path: String,
        /// Read the contents from this local file instead of stdin.
        #[arg(long, value_name = "FILE")]
        from: Option<PathBuf>,
    },
    /// Create a folder.
    Mkdir { path: String },
    /// Create an empty file.
    Touch { path: String },
    /// Delete files, or folders with -r.
    Rm {
        #[arg(required = true)]
        paths: Vec<String>,
        /// Delete folders and everything in them.
        #[arg(long, short = 'r')]
        recursive: bool,
    },
    /// Move or rename a file or folder. TO is the new path, not a folder to move into.
    Mv { from: String, to: String },
    /// Copy files or folders into a folder (servers only).
    Cp {
        #[arg(required = true, num_args = 1..)]
        sources: Vec<String>,
        /// The folder to copy into.
        dest: String,
    },
    /// Zip files or folders into a new archive (servers only).
    Zip {
        #[arg(required = true)]
        paths: Vec<String>,
        /// The archive to create, such as `backups/world.zip`.
        #[arg(long, short = 'o', value_name = "ZIP")]
        output: String,
        /// Wait until the archive is complete.
        #[arg(long)]
        wait: bool,
    },
    /// Extract an archive (servers only). Default: next to the archive.
    Unzip {
        path: String,
        /// Extract into this folder.
        #[arg(long, value_name = "DIR", conflicts_with_all = ["smart", "into_folder"])]
        to: Option<String>,
        /// Into a folder named after the archive, unless it holds a single folder.
        #[arg(long, conflicts_with = "into_folder")]
        smart: bool,
        /// Into a new folder named after the archive.
        #[arg(long)]
        into_folder: bool,
        /// Wait until extraction is complete.
        #[arg(long)]
        wait: bool,
    },
}

/* --------------------------------------------------------------- macros */

#[derive(Args, Debug, Clone)]
pub struct MacrosArgs {
    /// The server: its name or UUID.
    #[arg(value_name = "SERVER")]
    pub instance: String,
    #[command(subcommand)]
    pub action: MacrosCmd,
}

#[derive(Subcommand, Debug, Clone)]
pub enum MacrosCmd {
    /// List the server's macros.
    List,
    /// Run a macro with arguments.
    Run {
        name: String,
        /// Arguments for the macro. Put arguments that start with `-` after `--`.
        #[arg(allow_negative_numbers = true)]
        args: Vec<String>,
    },
    /// Stop a running macro.
    Kill { pid: i64 },
    /// Running macros.
    Tasks,
    /// Macros that ran, with their exit status.
    History,
    /// A macro's configuration.
    #[command(subcommand)]
    Config(MacroConfigCmd),
}

#[derive(Subcommand, Debug, Clone)]
pub enum MacroConfigCmd {
    /// Show a macro's settings and their saved values.
    Get { name: String },
    /// Save a value for one of a macro's settings.
    Set {
        name: String,
        key: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
}

/* ----------------------------------------------------------------- mods */

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchSort {
    #[default]
    Relevance,
    Downloads,
    Follows,
    Newest,
    Updated,
}

impl SearchSort {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchSort::Relevance => "relevance",
            SearchSort::Downloads => "downloads",
            SearchSort::Follows => "follows",
            SearchSort::Newest => "newest",
            SearchSort::Updated => "updated",
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct PageArgs {
    #[arg(long, value_enum, default_value_t = SearchSort::Relevance)]
    pub sort: SearchSort,
    /// Results per page (at most 100).
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
    #[arg(long, default_value_t = 0)]
    pub offset: u32,
}

#[derive(Args, Debug, Clone)]
pub struct ModsArgs {
    /// The server: its name or UUID.
    #[arg(value_name = "SERVER")]
    pub instance: String,
    #[command(subcommand)]
    pub action: ModsCmd,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ModsCmd {
    /// The installed mods or plugins.
    List {
        /// Also ask Modrinth for newer versions.
        #[arg(long)]
        check_updates: bool,
    },
    /// Search Modrinth for mods (or plugins) for this server. A Modrinth link shows
    /// that project instead.
    Search {
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Do not limit results to the server's game version and loader.
        #[arg(long)]
        all_versions: bool,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Details of a Modrinth project.
    Show {
        /// A slug, project id or Modrinth link.
        project: String,
        /// Also print the project's full description.
        #[arg(long)]
        body: bool,
    },
    /// Versions of a project that fit this server.
    Versions {
        project: String,
        /// Show every version, not only those for this game version and loader.
        #[arg(long)]
        all: bool,
    },
    /// Install a mod from Modrinth with its required dependencies. Default: the newest
    /// compatible release.
    Install {
        /// A slug, project id, version id or Modrinth link.
        target: String,
        /// A version number or version id of the project.
        #[arg(long)]
        version: Option<String>,
        /// Do not install required dependencies.
        #[arg(long)]
        no_deps: bool,
        /// Install even if the version is not for this loader or game version.
        #[arg(long)]
        force: bool,
    },
    /// Upload mod or plugin jars.
    Upload {
        #[arg(required = true)]
        jars: Vec<PathBuf>,
        /// Replace files with the same name.
        #[arg(long)]
        overwrite: bool,
    },
    /// Enable disabled mods.
    Enable {
        /// File names, slugs or names of installed mods.
        #[arg(required = true)]
        mods: Vec<String>,
    },
    /// Disable mods (renames them to .jar.disabled).
    Disable {
        #[arg(required = true)]
        mods: Vec<String>,
    },
    /// Delete mods.
    Remove {
        #[arg(required = true)]
        mods: Vec<String>,
    },
    /// Update mods to their newest compatible version.
    Update {
        /// The mod to update.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        r#mod: Option<String>,
        /// Update every mod that has an update.
        #[arg(long)]
        all: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    Mod,
    Plugin,
    Modpack,
}

impl ProjectType {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectType::Mod => "mod",
            ProjectType::Plugin => "plugin",
            ProjectType::Modpack => "modpack",
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum ModrinthCmd {
    /// Search Modrinth projects (server-side only).
    Search {
        #[arg(num_args = 0..)]
        query: Vec<String>,
        #[arg(long = "type", value_enum, default_value_t = ProjectType::Mod)]
        project_type: ProjectType,
        /// Only projects for this game version, such as 1.21.1.
        #[arg(long)]
        game_version: Option<String>,
        /// Only projects for this loader, such as fabric.
        #[arg(long)]
        loader: Option<String>,
        #[command(flatten)]
        page: PageArgs,
    },
    /// Details of a project.
    Show {
        project: String,
        #[arg(long)]
        body: bool,
    },
    /// Versions of a project, newest first.
    Versions {
        project: String,
        #[arg(long)]
        game_version: Option<String>,
        #[arg(long)]
        loader: Option<String>,
    },
    /// Resolve a Modrinth link to its project (and version).
    Resolve { url: String },
}

/* ---------------------------------------------------------------- users */

#[derive(Subcommand, Debug, Clone)]
pub enum UsersCmd {
    /// List users.
    List,
    /// The logged-in user.
    Me,
    /// One user.
    Info { user: String },
    /// Create a user (no permissions until you grant some).
    Create {
        username: String,
        /// Read the new user's password from stdin; else it is asked for.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Delete a user.
    Delete { user: String },
    /// Rename a user.
    Rename { user: String, new_name: String },
    /// Change a password (default: your own). Your current password comes from
    /// LODESTONE_PASSWORD or a prompt.
    Passwd {
        user: Option<String>,
        /// Read the new password from stdin.
        #[arg(long)]
        password_stdin: bool,
    },
    /// End every session of a user (logs them out everywhere).
    Logout { user: String },
    /// Permissions of a user.
    #[command(subcommand)]
    Perms(PermsCmd),
}

#[derive(Subcommand, Debug, Clone)]
pub enum PermsCmd {
    /// Show a user's permissions.
    Get { user: String },
    /// Set one permission. For a per-server permission VALUE is a comma-separated list
    /// of servers (empty for none); for the others, true or false.
    Set {
        user: String,
        permission: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Give a per-server permission for servers.
    Grant {
        user: String,
        permission: String,
        #[arg(required = true, value_name = "SERVERS")]
        instances: Vec<String>,
    },
    /// Take a per-server permission away for servers.
    Revoke {
        user: String,
        permission: String,
        #[arg(required = true, value_name = "SERVERS")]
        instances: Vec<String>,
    },
}

/* ------------------------------------------------------------- api keys */

#[derive(Subcommand, Debug, Clone)]
pub enum ApiKeysCmd {
    /// List your API keys, or another user's with --user (owner or admin).
    List {
        #[arg(long)]
        user: Option<String>,
    },
    /// Create an API key for yourself. The key is printed once; store it safely.
    Create(CreateKeyArgs),
    /// Revoke an API key. Its other keys and the user's sessions keep working.
    Revoke {
        /// Revoke a key of this user (owner or admin).
        #[arg(long)]
        user: Option<String>,
        /// The key's id, or its `lsk_<id>_…` form.
        id: String,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyExpiry {
    #[value(name = "30d")]
    Days30,
    #[value(name = "90d")]
    Days90,
    #[value(name = "1y")]
    Year,
    Never,
}

impl KeyExpiry {
    /// Seconds until the key expires, or `None` for never.
    pub fn seconds(self) -> Option<i64> {
        const DAY: i64 = 86400;
        match self {
            KeyExpiry::Days30 => Some(30 * DAY),
            KeyExpiry::Days90 => Some(90 * DAY),
            KeyExpiry::Year => Some(365 * DAY),
            KeyExpiry::Never => None,
        }
    }
}

#[derive(Args, Debug, Clone)]
#[command(group(clap::ArgGroup::new("scope").required(true).args(["full", "allow"])))]
pub struct CreateKeyArgs {
    /// A name that says what the key is for, such as `ci` or `claude agent`.
    #[arg(long)]
    pub name: String,
    #[arg(long, value_enum, default_value = "90d")]
    pub expires: KeyExpiry,
    /// Everything you can do, including owner or admin powers if you have them.
    #[arg(long, conflicts_with = "allow")]
    pub full: bool,
    /// Allow one permission, as in `users perms set`: PERMISSION=SERVERS for a
    /// per-server permission (comma-separated names or UUIDs), PERMISSION=true for one
    /// of the whole core. Repeat for more. The key can never do more than you can.
    #[arg(long, value_name = "PERMISSION=VALUE")]
    pub allow: Vec<String>,
}

/* --------------------------------------------------------------- events */

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLevelArg {
    Info,
    Warning,
    Error,
}

impl EventLevelArg {
    pub fn as_str(self) -> &'static str {
        match self {
            EventLevelArg::Info => "Info",
            EventLevelArg::Warning => "Warning",
            EventLevelArg::Error => "Error",
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventTypeArg {
    #[value(name = "server", alias = "instance")]
    Instance,
    User,
    Macro,
    Fs,
    Progression,
    Playit,
    Settings,
}

impl EventTypeArg {
    pub fn as_str(self) -> &'static str {
        match self {
            EventTypeArg::Instance => "InstanceEvent",
            EventTypeArg::User => "UserEvent",
            EventTypeArg::Macro => "MacroEvent",
            EventTypeArg::Fs => "FSEvent",
            EventTypeArg::Progression => "ProgressionEvent",
            EventTypeArg::Playit => "PlayitggRunnerEvent",
            EventTypeArg::Settings => "GlobalSettingsChanged",
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct EventsArgs {
    /// Keep streaming new events (reconnects if the connection drops).
    #[arg(long, short = 'f')]
    pub follow: bool,
    /// Only these levels.
    #[arg(long, value_enum, value_delimiter = ',')]
    pub level: Vec<EventLevelArg>,
    /// Only these event types.
    #[arg(long = "type", value_enum, value_delimiter = ',')]
    pub event_type: Vec<EventTypeArg>,
    /// Only events of these servers.
    #[arg(
        long = "server",
        visible_alias = "instance",
        value_name = "SERVER",
        value_delimiter = ','
    )]
    pub instance: Vec<String>,
    /// Only events caused by or about these users.
    #[arg(long, value_delimiter = ',')]
    pub user: Vec<String>,
    /// Search the saved history instead of the recent buffer, from this long ago
    /// (such as 30m, 12h, 7d).
    #[arg(long, value_name = "AGE")]
    pub since: Option<String>,
    /// Show at most the last N events.
    #[arg(long, short = 'n', value_name = "N")]
    pub limit: Option<usize>,
    /// Include console lines (hidden by default, as in the dashboard).
    #[arg(long)]
    pub include_console: bool,
}

/* --------------------------------------------------------------- playit */

#[derive(Subcommand, Debug, Clone)]
pub enum PlayitCmd {
    /// Whether the tunnel agent runs and its key is valid.
    Status,
    /// Start linking a playit.gg account: prints the link to open.
    Signup,
    /// Check the saved playit.gg key.
    Verify,
    /// Start the tunnel agent.
    Start,
    /// Stop the tunnel agent.
    Stop,
    /// The account's tunnels.
    Tunnels,
}

/* ---------------------------------------------------------------- tasks */

#[derive(Args, Debug, Clone)]
pub struct TasksArgs {
    /// Wait until this task ends: a task id, or the UUID of a server being created or
    /// deleted. Exits 1 if the task fails.
    #[arg(long, value_name = "ID")]
    pub wait: Option<String>,
    /// With --wait, give up after this many seconds.
    #[arg(long, value_name = "SECONDS", requires = "wait")]
    pub timeout: Option<u64>,
    /// Also list finished tasks.
    #[arg(long, conflicts_with = "wait")]
    pub all: bool,
}

/* ------------------------------------------------------------ extension */

#[derive(Subcommand, Debug, Clone)]
pub enum ExtensionCmd {
    /// Whether git is installed on the core's machine.
    GitStatus,
    /// Fetch an extension's manifest from its repository URL.
    Manifest { url: String },
    /// Install an extension from its repository URL (needs code execution).
    Install { url: String },
}
