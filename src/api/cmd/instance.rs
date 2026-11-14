//! `instance`: listing, creating, controlling and configuring instances.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::api::cli::{
    CreateArgs, CreateAtomArgs, CreateModpackArgs, Flavour, InstanceCmd, InstanceSettingsCmd,
    NewInstanceOpts, PowerArgs, StartArgs,
};
use crate::api::error::{kind, CliError, CliResult};
use crate::api::events::{Target, TaskWaiter};
use crate::api::http::{Body, Connection};
use crate::api::multipart;
use crate::api::output::{cell, format_bytes, format_unix_seconds, print_fields, Table};
use crate::api::resolve::{self, InstanceRef};
use crate::api::{prompt, Ctx};

use super::mods::{self, ModrinthTarget};
use super::values::{configurable_value, describe_type, display_value, parse_bool};

/// "Minecraft Fabric", "Minecraft Bedrock", or a generic instance's game name.
pub fn game_name(game: &Value) -> String {
    match game["type"].as_str().unwrap_or_default() {
        "MinecraftJava" => {
            let variant = &game["variant"];
            let name = match variant["type"].as_str().unwrap_or_default() {
                "Other" => variant["name"].as_str().unwrap_or("Other"),
                other => other,
            };
            format!("Minecraft {name}")
        }
        "MinecraftBedrock" => "Minecraft Bedrock".to_string(),
        "Generic" => game["game_display_name"]
            .as_str()
            .unwrap_or("Generic")
            .to_string(),
        other => other.to_string(),
    }
}

fn players_cell(info: &Value) -> String {
    match (
        info["player_count"].as_u64(),
        info["max_player_count"].as_u64(),
    ) {
        (Some(n), Some(max)) => format!("{n}/{max}"),
        (Some(n), None) => n.to_string(),
        _ => "-".to_string(),
    }
}

pub async fn run(ctx: &Ctx, cmd: InstanceCmd) -> CliResult<()> {
    let conn = ctx.connect()?;
    match cmd {
        InstanceCmd::List => {
            let list = conn.get(&["instance", "list"]).await?;
            ctx.out.show(&list, || {
                let mut table = Table::new([
                    "NAME", "STATE", "GAME", "VERSION", "PORT", "PLAYERS", "UUID",
                ]);
                for info in list.as_array().into_iter().flatten() {
                    table.row([
                        cell(&info["name"]),
                        cell(&info["state"]),
                        game_name(&info["game_type"]),
                        cell(&info["version"]),
                        cell(&info["port"]),
                        players_cell(info),
                        cell(&info["uuid"]),
                    ]);
                }
                table.print_or("No servers.");
            });
            Ok(())
        }
        InstanceCmd::Info { instance } => {
            let inst = resolve::instance(&conn, &instance).await?;
            let info = conn.get(&["instance", &inst.uuid, "info"]).await?;
            ctx.out.show(&info, || print_info(&info));
            Ok(())
        }
        InstanceCmd::State { instance } => {
            let inst = resolve::instance(&conn, &instance).await?;
            let state = conn.get(&["instance", &inst.uuid, "state"]).await?;
            ctx.out
                .show(&json!({"uuid": inst.uuid, "state": state}), || {
                    println!("{}", cell(&state))
                });
            Ok(())
        }
        InstanceCmd::Versions { flavour } => {
            let manifest = conn.get(&["setup_manifest", flavour.game_type()]).await?;
            let versions = version_options(&manifest)?;
            ctx.out.show(&json!(versions), || {
                for v in &versions {
                    println!("{v}");
                }
            });
            Ok(())
        }
        InstanceCmd::Create(args) => create(ctx, &conn, args).await,
        InstanceCmd::CreateModpack(args) => create_modpack(ctx, &conn, args).await,
        InstanceCmd::CreateAtom(args) => create_atom(ctx, &conn, args).await,
        InstanceCmd::DockerWatch { container } => {
            conn.put(
                &["generic_setup_manifest"],
                Body::Json(json!({ "url": container })),
            )
            .await?;
            ctx.out.done(
                format!("Added Docker container `{container}` to the watch list"),
                json!({"container": container}),
            );
            Ok(())
        }
        InstanceCmd::Start(args) => start(ctx, &conn, args, "start").await,
        InstanceCmd::Stop(args) => {
            let inst = resolve::instance(&conn, &args.instance).await?;
            power(ctx, &conn, &inst, &args, "stop").await
        }
        InstanceCmd::Restart(args) => start(ctx, &conn, args, "restart").await,
        InstanceCmd::Eula { instance, accept } => {
            let inst = resolve::instance(&conn, &instance).await?;
            let route = ["instance", inst.uuid.as_str(), "eula"];
            let status = if accept {
                conn.put(&route, Body::Empty).await
            } else {
                conn.get(&route).await
            }
            .map_err(|e| {
                if is_missing_route(&e) {
                    CliError::new(
                        kind::UNSUPPORTED,
                        "This core is too old to track the Minecraft EULA: it accepts it when a server starts",
                    )
                } else {
                    e
                }
            })?;
            let accepted = status["accepted"].as_bool().unwrap_or(accept);
            let result = json!({"uuid": inst.uuid, "accepted": accepted});
            if accept {
                ctx.out
                    .done(format!("Accepted the Minecraft EULA for {inst}"), result);
            } else {
                ctx.out.show(&result, || {
                    if accepted {
                        println!("{inst} has the Minecraft EULA accepted");
                    } else {
                        println!(
                            "{inst} does not have the Minecraft EULA ({EULA_URL}) accepted, so it cannot start. \
                             To agree to it, run `lodestone_cli server eula {} --accept`",
                            shell_word(&inst)
                        );
                    }
                });
            }
            Ok(())
        }
        InstanceCmd::Kill { instance } => {
            let inst = resolve::instance(&conn, &instance).await?;
            prompt::confirm(
                ctx.global.yes,
                &format!("Kill {inst}? The server stops at once without saving."),
            )?;
            conn.put(&["instance", &inst.uuid, "kill"], Body::Empty)
                .await?;
            ctx.out
                .done(format!("Killed {inst}"), json!({"uuid": inst.uuid}));
            Ok(())
        }
        InstanceCmd::Delete { instance } => {
            let inst = resolve::instance(&conn, &instance).await?;
            prompt::confirm(
                ctx.global.yes,
                &format!(
                    "Delete {inst} ({})? Its worlds, settings and backups are deleted for good.",
                    inst.uuid
                ),
            )?;
            conn.delete(&["instance", &inst.uuid]).await?;
            ctx.out
                .done(format!("Deleted {inst}"), json!({"uuid": inst.uuid}));
            Ok(())
        }
        InstanceCmd::Rename { instance, name } => {
            let inst = resolve::instance(&conn, &instance).await?;
            conn.put(&["instance", &inst.uuid, "name"], Body::Json(json!(name)))
                .await?;
            ctx.out.done(
                format!("Renamed {inst} to {name}"),
                json!({"uuid": inst.uuid, "name": name}),
            );
            Ok(())
        }
        InstanceCmd::Description {
            instance,
            description,
        } => {
            let inst = resolve::instance(&conn, &instance).await?;
            conn.put(
                &["instance", &inst.uuid, "description"],
                Body::Json(json!(description)),
            )
            .await?;
            ctx.out.done(
                format!("Changed the description of {inst}"),
                json!({"uuid": inst.uuid, "description": description}),
            );
            Ok(())
        }
        InstanceCmd::Version { instance, version } => {
            let inst = resolve::instance(&conn, &instance).await?;
            conn.put(&["instance", &inst.uuid, "version", &version], Body::Empty)
                .await?;
            ctx.out.done(
                format!("Changed the version of {inst} to {version}"),
                json!({"uuid": inst.uuid, "version": version}),
            );
            Ok(())
        }
        InstanceCmd::Settings(InstanceSettingsCmd::Get { instance }) => {
            let inst = resolve::instance(&conn, &instance).await?;
            let manifest = conn.get(&["instance", &inst.uuid, "settings"]).await?;
            ctx.out.show(&manifest, || print_settings(&manifest));
            Ok(())
        }
        InstanceCmd::Settings(InstanceSettingsCmd::Set {
            instance,
            key,
            value,
        }) => {
            let inst = resolve::instance(&conn, &instance).await?;
            let switch = match key.as_str() {
                "auto_start" | "auto-start" => Some("auto_start"),
                "restart_on_crash" | "restart-on-crash" => Some("restart_on_crash"),
                _ => None,
            };
            if let Some(route) = switch {
                let enabled = parse_bool(&value)?;
                conn.put(&["instance", &inst.uuid, route], Body::Json(json!(enabled)))
                    .await
                    .map_err(|e| older_core_error(e, route))?;
                ctx.out.done(
                    format!("Set {route} of {inst} to {enabled}"),
                    json!({"uuid": inst.uuid, "setting": route, "value": enabled}),
                );
                return Ok(());
            }
            let manifest = conn.get(&["instance", &inst.uuid, "settings"]).await?;
            let (section_id, setting_id, setting) = find_setting(&manifest, &key)?;
            let new_value = configurable_value(&setting["value_type"], &value)?;
            conn.put(
                &["instance", &inst.uuid, "settings", &section_id, &setting_id],
                Body::Json(new_value.clone()),
            )
            .await?;
            ctx.out.done(
                format!("Set {setting_id} of {inst} to {}", display_value(&new_value)),
                json!({"uuid": inst.uuid, "section": section_id, "setting": setting_id, "value": new_value}),
            );
            Ok(())
        }
    }
}

/// Where users read the Minecraft EULA.
const EULA_URL: &str = "https://aka.ms/MinecraftEULA";

/// A route the core does not have answers a bare 404: older cores lack the newer routes.
fn is_missing_route(e: &CliError) -> bool {
    e.kind == kind::NOT_FOUND && e.message.starts_with("The core answered HTTP")
}

/// An instance's name for a command the user can paste: quoted when it has spaces or
/// other characters a shell would split or expand.
fn shell_word(inst: &InstanceRef) -> String {
    let word = if inst.name.is_empty() {
        &inst.uuid
    } else {
        &inst.name
    };
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        word.clone()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The core refuses to start a Minecraft server whose EULA is not accepted. Its message
/// says so; this adds how to accept it from the CLI.
fn eula_hint(e: CliError, inst: &InstanceRef) -> CliError {
    if e.kind == kind::BAD_REQUEST && e.message.contains("EULA") {
        CliError {
            message: format!(
                "{} To agree to it, run `lodestone_cli server eula {} --accept`, or start with --accept-eula.",
                e.message.trim_end(),
                shell_word(inst)
            ),
            ..e
        }
    } else {
        e
    }
}

fn older_core_error(e: CliError, route: &str) -> CliError {
    if is_missing_route(&e) {
        CliError::new(
            kind::UNSUPPORTED,
            format!("This core is too old to change {route} after creation; update Lodestone Core"),
        )
    } else {
        e
    }
}

fn print_info(info: &Value) {
    print_fields(&[
        ("Name", cell(&info["name"])),
        ("UUID", cell(&info["uuid"])),
        ("Game", game_name(&info["game_type"])),
        ("Version", cell(&info["version"])),
        ("State", cell(&info["state"])),
        ("Port", cell(&info["port"])),
        ("Players", players_cell(info)),
        ("Description", cell(&info["description"])),
        ("Auto start", cell(&info["auto_start"])),
        ("Restart on crash", cell(&info["restart_on_crash"])),
        (
            "Created",
            format_unix_seconds(info["creation_time"].as_i64().unwrap_or_default()),
        ),
        ("Path", cell(&info["path"])),
    ]);
}

fn print_settings(manifest: &Value) {
    print_fields(&[
        ("auto_start", cell(&manifest["auto_start"])),
        ("restart_on_crash", cell(&manifest["restart_on_crash"])),
    ]);
    for (_, section) in manifest["setting_sections"]
        .as_object()
        .into_iter()
        .flatten()
    {
        println!(
            "\n{} ({})",
            cell(&section["name"]),
            cell(&section["section_id"])
        );
        let mut table = Table::new(["SETTING", "VALUE", "TYPE", "NAME"]);
        for (id, setting) in section["settings"].as_object().into_iter().flatten() {
            let value = if setting["is_secret"].as_bool().unwrap_or(false) {
                "(secret)".to_string()
            } else {
                display_value(&setting["value"])
            };
            let mut ty = describe_type(&setting["value_type"]);
            if setting["is_mutable"].as_bool() == Some(false) {
                ty.push_str(", read-only");
            }
            table.row([id.clone(), value, ty, cell(&setting["name"])]);
        }
        table.print_or("  (none yet: start the server once to fill server.properties)");
    }
}

/// Finds a setting by `section.setting` or by a setting id unique across sections.
pub fn find_setting(manifest: &Value, key: &str) -> CliResult<(String, String, Value)> {
    let sections = manifest["setting_sections"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some((section_id, setting_id)) = key.split_once('.') {
        if let Some(setting) = sections
            .get(section_id)
            .and_then(|s| s["settings"].get(setting_id))
        {
            return Ok((
                section_id.to_string(),
                setting_id.to_string(),
                setting.clone(),
            ));
        }
    }
    let matches: Vec<(String, String, Value)> = sections
        .iter()
        .filter_map(|(section_id, section)| {
            section["settings"]
                .get(key)
                .map(|s| (section_id.clone(), key.to_string(), s.clone()))
        })
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one match")),
        0 => Err(CliError::not_found(format!(
            "No setting `{key}` (see `server settings get`; server.properties settings appear once the server ran)"
        ))),
        _ => Err(CliError::new(
            kind::AMBIGUOUS,
            format!(
                "`{key}` is in several sections; use one of: {}",
                matches
                    .iter()
                    .map(|(s, k, _)| format!("{s}.{k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/* ---------------------------------------------------------- power */

/// Starts or restarts a server, first accepting its Minecraft EULA with --accept-eula.
async fn start(ctx: &Ctx, conn: &Connection, args: StartArgs, action: &str) -> CliResult<()> {
    let inst = resolve::instance(conn, &args.power.instance).await?;
    if args.accept_eula {
        match conn
            .put(&["instance", &inst.uuid, "eula"], Body::Empty)
            .await
        {
            Ok(_) => ctx
                .out
                .note(format!("Accepted the Minecraft EULA for {inst}")),
            // Older cores have no EULA route: they accept the EULA when a server starts.
            Err(e) if is_missing_route(&e) => {}
            Err(e) => return Err(e),
        }
    }
    power(ctx, conn, &inst, &args.power, action).await
}

async fn power(
    ctx: &Ctx,
    conn: &Connection,
    inst: &InstanceRef,
    args: &PowerArgs,
    action: &str,
) -> CliResult<()> {
    conn.put(&["instance", &inst.uuid, action], Body::Empty)
        .await
        .map_err(|e| eula_hint(e, inst))?;
    let past = match action {
        "start" => "Starting",
        "stop" => "Stopping",
        _ => "Restarting",
    };
    if !args.wait {
        ctx.out.done(
            format!("{past} {inst}"),
            json!({"uuid": inst.uuid, "action": action}),
        );
        return Ok(());
    }
    let target = if action == "stop" {
        "Stopped"
    } else {
        "Running"
    };
    ctx.out
        .note(format!("{past} {inst}, waiting until it is {target}"));
    let state = wait_for_state(conn, inst, target, Duration::from_secs(args.timeout)).await?;
    ctx.out.done(
        format!("{inst} is {state}"),
        json!({"uuid": inst.uuid, "action": action, "state": state}),
    );
    Ok(())
}

/// Polls the instance's state until it is `target`. An instance that falls into Error, or
/// stops while it should be starting, fails.
async fn wait_for_state(
    conn: &Connection,
    inst: &InstanceRef,
    target: &str,
    timeout: Duration,
) -> CliResult<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut seen_transition = false;
    loop {
        let state = conn.get(&["instance", &inst.uuid, "state"]).await?;
        let state = state.as_str().unwrap_or_default().to_string();
        if state == target {
            return Ok(state);
        }
        match state.as_str() {
            "Error" => {
                return Err(CliError::new(
                    kind::TASK_FAILED,
                    format!("{inst} went into the Error state; see `lodestone_cli console {inst}`"),
                ))
            }
            "Starting" | "Stopping" => seen_transition = true,
            "Stopped" if target == "Running" && seen_transition => {
                return Err(CliError::new(
                    kind::TASK_FAILED,
                    format!("{inst} stopped while starting; see `lodestone_cli console {inst}`"),
                ))
            }
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::new(
                kind::TIMEOUT,
                format!("{inst} is still {state} after {}s", timeout.as_secs()),
            ));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/* --------------------------------------------------------- create */

/// The version options of a setup manifest, newest first.
fn version_options(manifest: &Value) -> CliResult<Vec<String>> {
    let (_, setting) = setup_setting(manifest, "version")
        .ok_or_else(|| CliError::bad_response("The setup manifest has no version setting"))?;
    Ok(setting["value_type"]["options"]
        .as_array()
        .map(|o| {
            o.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

/// Whether `version` names a release, like `1.21.4` or `26.1`: numbers and dots only, as
/// in the core. Snapshots (`25w45a`, `26.1-snapshot-1`), pre-releases (`1.21.9-pre1`) and
/// release candidates (`1.21.9-rc1`) aren't.
fn is_release(version: &str) -> bool {
    let mut parts = version.split('.');
    let count = parts.clone().count();
    count >= 2 && parts.all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// The version `latest` (and the default) stands for: the newest release, not a snapshot
/// or pre-release, since those can break worlds and most mods don't support them yet. The
/// core's form default is used when it is a release; cores before it picked releases
/// defaulted to the first option, which can be a snapshot, so the CLI then looks for the
/// newest release itself. With no release listed at all, the core's default or the newest
/// version is used.
fn default_version(manifest: &Value, versions: &[String]) -> Option<String> {
    let core_default = setup_setting(manifest, "version")
        .and_then(|(_, s)| s["value"]["value"].as_str())
        .filter(|v| versions.iter().any(|o| o == v));
    core_default
        .filter(|v| is_release(v))
        .or_else(|| versions.iter().map(String::as_str).find(|v| is_release(v)))
        .or(core_default)
        .or_else(|| versions.first().map(String::as_str))
        .map(str::to_string)
}

fn setup_setting<'a>(manifest: &'a Value, id: &str) -> Option<(&'a str, &'a Value)> {
    manifest["setting_sections"]
        .as_object()?
        .iter()
        .find_map(|(section_id, section)| {
            section["settings"]
                .get(id)
                .map(|s| (section_id.as_str(), s))
        })
}

/// The first port from 25565 that is neither in use nor given to an instance.
async fn free_port(conn: &Connection) -> CliResult<u16> {
    for port in 25565u16..25665 {
        let status = conn.get(&["check", "port", &port.to_string()]).await?;
        if status["is_in_use"] == json!(false) && status["is_allocated"] == json!(false) {
            return Ok(port);
        }
    }
    Err(CliError::new(
        kind::BAD_REQUEST,
        "No free port between 25565 and 25664; pass --port",
    ))
}

fn ask_u32(question: &str, default: u32) -> CliResult<u32> {
    let answer = prompt::ask(question, Some(&default.to_string()))?;
    answer
        .trim()
        .parse()
        .map_err(|_| CliError::usage(format!("`{answer}` is not a positive whole number")))
}

/// Fills in the values of a setup manifest. `values` maps setting ids to the chosen
/// `ConfigurableValue`s; other settings keep their default.
pub fn setup_sections(manifest: &Value, values: &Map<String, Value>) -> Value {
    let mut sections = Map::new();
    for (section_id, section) in manifest["setting_sections"]
        .as_object()
        .into_iter()
        .flatten()
    {
        let mut settings = Map::new();
        for (id, setting) in section["settings"].as_object().into_iter().flatten() {
            let value = values
                .get(id)
                .cloned()
                .unwrap_or_else(|| setting["value"].clone());
            settings.insert(id.clone(), json!({ "value": value }));
        }
        sections.insert(section_id.clone(), json!({ "settings": settings }));
    }
    Value::Object(sections)
}

fn setup_value(opts: &NewInstanceOpts, name: &str, sections: Value) -> Value {
    json!({
        "name": name,
        "description": opts.description,
        "auto_start": opts.auto_start,
        "restart_on_crash": opts.restart_on_crash,
        "setting_sections": sections,
    })
}

/// Adds `accept_eula: true` to a creation body when the user agreed to the Minecraft EULA
/// (--accept-eula). Without it nothing is sent, and the core creates the server with the
/// EULA not accepted: only the user can agree to it, so the CLI never does on its own.
fn with_eula(mut body: Value, accept_eula: bool) -> Value {
    if accept_eula {
        body["accept_eula"] = json!(true);
    }
    body
}

/// Prints the result of starting an instance creation, waiting for it with `--wait`.
/// With `eula_pending` (a Minecraft server created without --accept-eula) it also says
/// how to accept the EULA, which the server needs to start.
async fn report_creation(
    ctx: &Ctx,
    conn: &Connection,
    uuid: &str,
    name: &str,
    opts: &NewInstanceOpts,
    eula_pending: bool,
) -> CliResult<()> {
    let eula_note = || {
        if !eula_pending {
            return;
        }
        let inst = InstanceRef {
            uuid: uuid.to_string(),
            name: name.to_string(),
        };
        let message = format!(
            "{name} cannot start until the Minecraft EULA ({EULA_URL}) is accepted{}. To agree to it, run `lodestone_cli server eula {} --accept`.",
            if opts.auto_start { ", so it will not start on its own yet" } else { "" },
            shell_word(&inst)
        );
        // --auto-start does nothing until then, which scripts should hear about too
        if opts.auto_start {
            ctx.out.warn(message);
        } else {
            ctx.out.note(message);
        }
    };
    if !opts.wait {
        ctx.out.done(
            format!("Creating {name} ({uuid}). Follow it with `lodestone_cli tasks --wait {uuid}`"),
            json!({"uuid": uuid, "name": name, "state": "creating"}),
        );
        eula_note();
        return Ok(());
    }
    ctx.out.note(format!("Creating {name} ({uuid})..."));
    let waiter = TaskWaiter {
        conn,
        out: &ctx.out,
        timeout: Some(Duration::from_secs(opts.timeout)),
    };
    let ((), task) = waiter
        .wait(Target::Id(uuid.to_string()), || async { Ok(()) })
        .await?;
    let message = task.end_message.clone().unwrap_or_default();
    ctx.out.done(
        format!("Created {name} ({uuid}). {message}"),
        json!({"uuid": uuid, "name": name, "state": "created", "task": task.to_json()}),
    );
    eula_note();
    Ok(())
}

async fn create(ctx: &Ctx, conn: &Connection, args: CreateArgs) -> CliResult<()> {
    let ask_all = args.interactive;
    let interactive = ask_all || (args.flavour.is_none() && prompt::interactive());
    if !interactive && args.flavour.is_none() {
        return Err(CliError::usage(
            "--flavour is required (vanilla, fabric, forge or paper)",
        ));
    }
    let flavour = match args.flavour {
        Some(f) if !ask_all => f,
        default => {
            let default = default.unwrap_or(Flavour::Fabric);
            let answer = prompt::ask(
                "Flavour (vanilla, fabric, forge, paper)",
                Some(&format!("{default:?}").to_lowercase()),
            )?;
            <Flavour as clap::ValueEnum>::from_str(answer.trim(), true)
                .map_err(|_| CliError::usage(format!("`{answer}` is not a flavour")))?
        }
    };
    let manifest = conn.get(&["setup_manifest", flavour.game_type()]).await?;
    let versions = version_options(&manifest)?;
    let newest = default_version(&manifest, &versions)
        .ok_or_else(|| CliError::bad_response("The core lists no versions"))?;
    let version = match args.version.as_deref() {
        Some(v) if !ask_all => v.to_string(),
        given => {
            if interactive {
                eprintln!(
                    "{} versions available (newest first): {}{}",
                    versions.len(),
                    versions
                        .iter()
                        .take(8)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    if versions.len() > 8 { ", ..." } else { "" }
                );
                prompt::ask("Version", Some(given.unwrap_or(&newest)))?
            } else {
                "latest".to_string()
            }
        }
    };
    let version = if version.eq_ignore_ascii_case("latest") {
        newest
    } else if versions.contains(&version) {
        version
    } else {
        return Err(CliError::usage(format!(
            "Version `{version}` is not available for {flavour:?}; see `lodestone_cli server versions {}`",
            format!("{flavour:?}").to_lowercase()
        )));
    };
    let name = match &args.opts.name {
        Some(n) if !ask_all => n.clone(),
        given => {
            if !interactive {
                return Err(CliError::usage("--name is required"));
            }
            let default = given
                .clone()
                .unwrap_or_else(|| format!("{flavour:?} {version}"));
            prompt::ask("Name", Some(&default))?
        }
    };
    let port = match args.opts.port {
        Some(p) if !ask_all => p,
        given => {
            let default = match given {
                Some(p) => p,
                None => free_port(conn).await?,
            };
            if interactive {
                let answer = ask_u32("Port", default as u32)?;
                u16::try_from(answer)
                    .map_err(|_| CliError::usage(format!("{answer} is not a port")))?
            } else {
                default
            }
        }
    };
    let default_ram = |id: &str, fallback: u32| {
        setup_setting(&manifest, id)
            .and_then(|(_, s)| s["value"]["value"].as_u64())
            .map(|v| v as u32)
            .unwrap_or(fallback)
    };
    let min_ram = match args.opts.min_ram {
        Some(v) if !ask_all => v,
        given => {
            let default = given.unwrap_or_else(|| default_ram("min_ram", 1024));
            if interactive {
                ask_u32("Minimum RAM (MB)", default)?
            } else {
                default
            }
        }
    };
    let max_ram = match args.opts.max_ram {
        Some(v) if !ask_all => v,
        given => {
            let default = given.unwrap_or_else(|| default_ram("max_ram", 2048).max(min_ram));
            if interactive {
                ask_u32("Maximum RAM (MB)", default)?
            } else {
                default
            }
        }
    };
    if min_ram > max_ram {
        return Err(CliError::usage("--min-ram cannot be more than --max-ram"));
    }
    let mut values = Map::new();
    values.insert("version".into(), json!({"type": "Enum", "value": version}));
    values.insert(
        "port".into(),
        json!({"type": "UnsignedInteger", "value": port}),
    );
    values.insert(
        "min_ram".into(),
        json!({"type": "UnsignedInteger", "value": min_ram}),
    );
    values.insert(
        "max_ram".into(),
        json!({"type": "UnsignedInteger", "value": max_ram}),
    );
    values.insert(
        "cmd_args".into(),
        match &args.cmd_args {
            Some(a) if !a.is_empty() => json!({"type": "String", "value": a}),
            _ => Value::Null,
        },
    );
    let body = with_eula(
        setup_value(&args.opts, &name, setup_sections(&manifest, &values)),
        args.accept_eula,
    );
    let uuid = conn
        .post(
            &["instance", "create", flavour.game_type()],
            Body::Json(body),
        )
        .await?;
    let uuid = uuid
        .as_str()
        .ok_or_else(|| CliError::bad_response("The core did not return the new server's UUID"))?
        .to_string();
    report_creation(ctx, conn, &uuid, &name, &args.opts, !args.accept_eula).await
}

/// The pack to create an instance from.
enum PackSource {
    Modrinth { version_id: String },
    Upload { upload_id: String },
}

async fn create_modpack(ctx: &Ctx, conn: &Connection, args: CreateModpackArgs) -> CliResult<()> {
    let local = Path::new(&args.source);
    let (source, info) = if args.source.to_lowercase().ends_with(".mrpack") && local.is_file() {
        let part = multipart::Part::for_file(local)?;
        ctx.out.note(format!(
            "Uploading {} ({})",
            part.file_name,
            format_bytes(part.size)
        ));
        let info = conn
            .post(
                &["modpack", "upload"],
                multipart::body("file", vec![part], |_| {}),
            )
            .await?;
        let upload_id = info["upload_id"]
            .as_str()
            .ok_or_else(|| CliError::bad_response("The core did not return an upload id"))?
            .to_string();
        (PackSource::Upload { upload_id }, info)
    } else {
        let version_id = match mods::resolve_target(conn, &args.source).await? {
            ModrinthTarget::Version(version) => {
                if args.pack_version.is_some() {
                    return Err(CliError::usage(
                        "--pack-version only applies to a project, not to a version",
                    ));
                }
                mods::str_of(&version, "id")
            }
            ModrinthTarget::Project { id, version_id } => {
                let project = conn.get(&["modrinth", "project", &id]).await?;
                if project["project_type"].as_str() != Some("modpack") {
                    return Err(CliError::usage(format!(
                        "`{}` is a {}, not a modpack",
                        mods::str_of(&project, "title"),
                        mods::str_of(&project, "project_type")
                    )));
                }
                match (version_id, &args.pack_version) {
                    (Some(v), None) => v,
                    (_, wanted) => {
                        let versions = conn.get(&["modrinth", "project", &id, "versions"]).await?;
                        let versions = versions.as_array().cloned().unwrap_or_default();
                        mods::pick_version(&versions, wanted.as_deref())?
                    }
                }
            }
        };
        let info = conn
            .post(
                &["modpack", "inspect"],
                Body::Json(json!({ "version_id": version_id })),
            )
            .await?;
        (PackSource::Modrinth { version_id }, info)
    };
    if args.inspect {
        ctx.out.show(&info, || print_pack(&info));
        return Ok(());
    }
    if let Some(reason) = info["unsupported_reason"].as_str() {
        return Err(CliError::new(
            kind::UNSUPPORTED,
            format!("This modpack is not supported: {reason}"),
        ));
    }
    if !ctx.out.json {
        print_pack(&info);
    }
    let name = args
        .opts
        .name
        .clone()
        .unwrap_or_else(|| mods::str_of(&info, "name"));
    let port = match args.opts.port {
        Some(p) => p,
        None => free_port(conn).await?,
    };
    let source = match source {
        PackSource::Modrinth { version_id } => {
            json!({"type": "modrinth", "version_id": version_id})
        }
        PackSource::Upload { upload_id } => json!({"type": "upload", "upload_id": upload_id}),
    };
    let mut body = json!({
        "source": source,
        "name": name,
        "port": port,
        "auto_start": args.opts.auto_start,
        "restart_on_crash": args.opts.restart_on_crash,
    });
    for (key, value) in [
        (
            "description",
            args.opts.description.clone().map(Value::from),
        ),
        ("min_ram", args.opts.min_ram.map(Value::from)),
        ("max_ram", args.opts.max_ram.map(Value::from)),
    ] {
        if let Some(value) = value {
            body[key] = value;
        }
    }
    let body = with_eula(body, args.accept_eula);
    let uuid = conn
        .post(&["instance", "create_modpack"], Body::Json(body))
        .await?;
    let uuid = uuid
        .as_str()
        .ok_or_else(|| CliError::bad_response("The core did not return the new server's UUID"))?
        .to_string();
    report_creation(ctx, conn, &uuid, &name, &args.opts, !args.accept_eula).await
}

fn print_pack(info: &Value) {
    print_fields(&[
        ("Pack", cell(&info["name"])),
        ("Version", cell(&info["version_number"])),
        ("Summary", cell(&info["summary"])),
        ("Minecraft", cell(&info["game_version"])),
        (
            "Loader",
            format!(
                "{} {}",
                cell(&info["loader"]),
                cell(&info["loader_version"])
            ),
        ),
        ("Server files", cell(&info["server_file_count"])),
        (
            "Download size",
            format_bytes(info["server_download_size"].as_u64().unwrap_or_default()),
        ),
        ("Unsupported", cell(&info["unsupported_reason"])),
    ]);
}

async fn create_atom(ctx: &Ctx, conn: &Connection, args: CreateAtomArgs) -> CliResult<()> {
    let name = args
        .opts
        .name
        .clone()
        .ok_or_else(|| CliError::usage("--name is required"))?;
    let sections = match &args.settings_json {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| CliError::io(format!("Cannot read {}", path.display()), e))?;
            serde_json::from_str(&text).map_err(|e| {
                CliError::usage(format!("{} is not valid JSON: {e}", path.display()))
            })?
        }
        None => json!({}),
    };
    let body = json!({
        "url": args.url,
        "setup_value": setup_value(&args.opts, &name, sections),
    });
    let uuid = conn
        .post(&["instance", "create_generic"], Body::Json(body))
        .await?;
    match uuid.as_str() {
        // atom servers aren't Minecraft servers, so they have no EULA
        Some(uuid) => report_creation(ctx, conn, uuid, &name, &args.opts, false).await,
        None => {
            // older cores return nothing, so there is nothing to wait for
            if args.opts.wait {
                ctx.out.warn(
                    "This core does not return the new server's UUID, so --wait cannot follow it",
                );
            }
            ctx.out.done(
                format!("Creating atom server {name}. Follow it with `lodestone_cli tasks`"),
                json!({"uuid": null, "name": name, "state": "creating"}),
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Value {
        json!({"setting_sections": {
            "section_1": {"section_id": "section_1", "settings": {
                "version": {"value": {"type": "Enum", "value": "1.21.1"}, "value_type": {"type": "Enum", "options": ["1.21.1", "1.20.1"]}},
                "port": {"value": {"type": "UnsignedInteger", "value": 25565}, "value_type": {"type": "UnsignedInteger", "min": 1, "max": 65535}}
            }},
            "section_2": {"section_id": "section_2", "settings": {
                "min_ram": {"value": {"type": "UnsignedInteger", "value": 1024}, "value_type": {"type": "UnsignedInteger"}},
                "cmd_args": {"value": null, "value_type": {"type": "String", "regex": null}}
            }}
        }})
    }

    #[test]
    fn setup_sections_fill_values_and_defaults() {
        let mut values = Map::new();
        values.insert("version".into(), json!({"type": "Enum", "value": "1.20.1"}));
        values.insert("cmd_args".into(), Value::Null);
        let sections = setup_sections(&manifest(), &values);
        assert_eq!(
            sections,
            json!({
                "section_1": {"settings": {
                    "version": {"value": {"type": "Enum", "value": "1.20.1"}},
                    "port": {"value": {"type": "UnsignedInteger", "value": 25565}}
                }},
                "section_2": {"settings": {
                    "min_ram": {"value": {"type": "UnsignedInteger", "value": 1024}},
                    "cmd_args": {"value": null}
                }}
            })
        );
        assert_eq!(version_options(&manifest()).unwrap(), ["1.21.1", "1.20.1"]);
    }

    fn version_manifest(default: &str, options: &[&str]) -> Value {
        json!({"setting_sections": {"section_1": {"settings": {
            "version": {"value": {"type": "Enum", "value": default}, "value_type": {"type": "Enum", "options": options}}
        }}}})
    }

    #[test]
    fn latest_is_the_newest_release() {
        let options = ["26.4-snapshot-8", "26.3-rc1", "26.3", "26.2", "25w45a"];
        let versions: Vec<String> = options.iter().map(|v| v.to_string()).collect();
        // the core's default when it is a release
        let m = version_manifest("26.3", &options);
        assert_eq!(default_version(&m, &versions).unwrap(), "26.3");
        // older cores defaulted to the first option, a snapshot (LS-73)
        let m = version_manifest("26.4-snapshot-8", &options);
        assert_eq!(default_version(&m, &versions).unwrap(), "26.3");
        // no default at all
        let m = json!({"setting_sections": {}});
        assert_eq!(default_version(&m, &versions).unwrap(), "26.3");
        // only snapshots: the core's default, else the newest
        let snapshots = vec!["25w46a".to_string(), "25w45a".to_string()];
        let m = version_manifest("25w45a", &["25w46a", "25w45a"]);
        assert_eq!(default_version(&m, &snapshots).unwrap(), "25w45a");
        assert_eq!(default_version(&json!({}), &snapshots).unwrap(), "25w46a");
        assert_eq!(default_version(&json!({}), &[]), None);
        assert!(is_release("1.21.4") && is_release("26.1"));
        assert!(!is_release("1.21.9-pre1") && !is_release("25w45a") && !is_release("1"));
    }

    #[test]
    fn the_eula_is_only_accepted_when_asked() {
        let body = setup_value(&NewInstanceOpts::default(), "x", json!({}));
        assert!(with_eula(body.clone(), false).get("accept_eula").is_none());
        assert_eq!(with_eula(body, true)["accept_eula"], true);
    }

    #[test]
    fn eula_errors_say_how_to_accept() {
        let inst = InstanceRef {
            uuid: "INSTANCE_1".into(),
            name: "My World".into(),
        };
        let e = CliError::from_response(
            400,
            r#"{"kind":"BadRequest","causes":["Accept the Minecraft EULA (https://aka.ms/MinecraftEULA) before you start this server."]}"#,
        );
        let e = eula_hint(e, &inst);
        assert_eq!(e.kind, "BadRequest");
        assert!(
            e.message.ends_with(
                "run `lodestone_cli server eula 'My World' --accept`, or start with --accept-eula."
            ),
            "{}",
            e.message
        );
        let other = CliError::new(kind::BAD_REQUEST, "Port in use");
        assert_eq!(eula_hint(other.clone(), &inst), other);
        let named = |name: &str| InstanceRef {
            uuid: "INSTANCE_1".into(),
            name: name.into(),
        };
        assert_eq!(shell_word(&named("survival-2")), "survival-2");
        assert_eq!(shell_word(&named("it's")), "'it'\\''s'");
        assert_eq!(shell_word(&named("")), "INSTANCE_1");
    }

    #[test]
    fn settings_by_id_or_section() {
        let m = json!({"setting_sections": {
            "cmd_args_section": {"settings": {"max_ram": {"value_type": {"type": "UnsignedInteger"}}}},
            "server_properties_section": {"settings": {"max-players": {"value_type": {"type": "Integer"}}, "max_ram": {}}}
        }});
        let (section, id, _) = find_setting(&m, "max-players").unwrap();
        assert_eq!(
            (section.as_str(), id.as_str()),
            ("server_properties_section", "max-players")
        );
        assert_eq!(
            find_setting(&m, "max_ram").unwrap_err().kind,
            kind::AMBIGUOUS
        );
        let (section, _, _) = find_setting(&m, "cmd_args_section.max_ram").unwrap();
        assert_eq!(section, "cmd_args_section");
        assert_eq!(find_setting(&m, "motd").unwrap_err().kind, kind::NOT_FOUND);
    }

    #[test]
    fn game_names() {
        assert_eq!(
            game_name(&json!({"type": "MinecraftJava", "variant": {"type": "Fabric"}})),
            "Minecraft Fabric"
        );
        assert_eq!(
            game_name(
                &json!({"type": "Generic", "game_name": "Generic", "game_display_name": "Terraria"})
            ),
            "Terraria"
        );
    }
}
