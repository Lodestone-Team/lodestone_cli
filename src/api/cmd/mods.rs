//! `mods` and `modrinth`: Modrinth search through the core, and an instance's mods or
//! plugins.

use reqwest::Method;
use serde_json::{json, Value};

use crate::api::cli::{ModrinthCmd, ModsArgs, ModsCmd, PageArgs};
use crate::api::error::{kind, CliError, CliResult};
use crate::api::http::{Body, Connection};
use crate::api::multipart;
use crate::api::output::{cell, format_bytes, print_fields, Table};
use crate::api::resolve::{self, InstanceRef};
use crate::api::{prompt, Ctx};

pub fn str_of(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

/// What a project argument (slug, id, version id or link) points at.
#[derive(Debug, Clone, PartialEq)]
pub enum ModrinthTarget {
    Project {
        id: String,
        /// A version named by the link.
        version_id: Option<String>,
    },
    Version(Value),
}

pub fn is_modrinth_link(input: &str) -> bool {
    input.to_lowercase().contains("modrinth.com/")
}

/// Resolves a slug, project id, version id or Modrinth link through the core.
pub async fn resolve_target(conn: &Connection, input: &str) -> CliResult<ModrinthTarget> {
    let input = input.trim();
    if is_modrinth_link(input) {
        let target = conn
            .get_query(&["modrinth", "resolve"], &[("url", input.to_string())])
            .await?;
        return Ok(ModrinthTarget::Project {
            id: str_of(&target, "project_id"),
            version_id: target["version_id"].as_str().map(str::to_string),
        });
    }
    match conn.get(&["modrinth", "project", input]).await {
        Ok(project) => {
            return Ok(ModrinthTarget::Project {
                id: str_of(&project, "id"),
                version_id: None,
            })
        }
        Err(e) if e.kind == kind::NOT_FOUND => {}
        Err(e) => return Err(e),
    }
    match conn.get(&["modrinth", "version", input]).await {
        Ok(version) => Ok(ModrinthTarget::Version(version)),
        Err(e) if e.kind == kind::NOT_FOUND => Err(CliError::not_found(format!(
            "No Modrinth project or version `{input}`"
        ))),
        Err(e) => Err(e),
    }
}

/// Picks a version id from `versions` (newest first): the one whose id or version number is
/// `wanted`, else the newest release, else the newest version.
pub fn pick_version(versions: &[Value], wanted: Option<&str>) -> CliResult<String> {
    if let Some(wanted) = wanted {
        return versions
            .iter()
            .find(|v| str_of(v, "id") == wanted || str_of(v, "version_number") == wanted)
            .map(|v| str_of(v, "id"))
            .ok_or_else(|| {
                let known: Vec<String> = versions
                    .iter()
                    .take(10)
                    .map(|v| str_of(v, "version_number"))
                    .collect();
                CliError::not_found(format!(
                    "No version `{wanted}`. Recent versions: {}",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                ))
            });
    }
    versions
        .iter()
        .find(|v| v["version_type"] == "release")
        .or_else(|| versions.first())
        .map(|v| str_of(v, "id"))
        .ok_or_else(|| CliError::not_found("The project has no versions"))
}

/// The Modrinth loaders whose versions an instance loader accepts (Paper runs Bukkit and
/// Spigot plugins, as the core's install check does).
pub fn accepted_loaders(loader: &str) -> Vec<&'static str> {
    match loader {
        "paper" => vec!["paper", "spigot", "bukkit"],
        "spigot" => vec!["spigot", "bukkit"],
        "fabric" => vec!["fabric"],
        "forge" => vec!["forge"],
        _ => vec![],
    }
}

/// Whether a version fits an instance's loader and game version.
pub fn is_compatible(version: &Value, loader: &str, game_version: &str) -> bool {
    let has = |key: &str, wanted: &[&str]| {
        version[key].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|i| i.as_str().is_some_and(|i| wanted.contains(&i)))
        })
    };
    has("loaders", &accepted_loaders(loader)) && has("game_versions", &[game_version])
}

/* ------------------------------------------------------------ output */

fn mod_name(m: &Value) -> String {
    m["modrinth"]["title"]
        .as_str()
        .or_else(|| m["metadata"]["name"].as_str())
        .unwrap_or_else(|| m["file_name"].as_str().unwrap_or_default())
        .to_string()
}

fn mod_version(m: &Value) -> String {
    m["modrinth"]["version_number"]
        .as_str()
        .or_else(|| m["metadata"]["version"].as_str())
        .unwrap_or("-")
        .to_string()
}

fn print_mod_list(list: &Value, check_updates: bool) {
    let kind_name = if list["kind"] == "plugin" {
        "plugins"
    } else {
        "mods"
    };
    println!(
        "{} {} {kind_name} in {}/",
        cell(&list["loader"]),
        cell(&list["game_version"]),
        cell(&list["directory"])
    );
    let mut headers = vec!["NAME", "VERSION", "ENABLED", "FILE", "SIZE"];
    if check_updates {
        headers.push("UPDATE");
    }
    let mut table = Table::new(headers);
    for m in list["mods"].as_array().into_iter().flatten() {
        let mut row = vec![
            mod_name(m),
            mod_version(m),
            if m["enabled"].as_bool().unwrap_or(false) {
                "yes"
            } else {
                "no"
            }
            .to_string(),
            cell(&m["file_name"]),
            format_bytes(m["size"].as_u64().unwrap_or_default()),
        ];
        if check_updates {
            row.push(
                m["modrinth"]["update"]["version_number"]
                    .as_str()
                    .unwrap_or("-")
                    .to_string(),
            );
        }
        table.row(row);
    }
    table.print_or(&format!("No {kind_name} installed."));
    if list["restart_required"].as_bool().unwrap_or(false) {
        println!("Restart the server to apply {kind_name} changes.");
    }
}

fn print_search(results: &Value) {
    let mut table = Table::new(["TITLE", "SLUG", "AUTHOR", "DOWNLOADS", "DESCRIPTION"]);
    for hit in results["hits"].as_array().into_iter().flatten() {
        let description: String = str_of(hit, "description").chars().take(60).collect();
        table.row([
            str_of(hit, "title"),
            str_of(hit, "slug"),
            str_of(hit, "author"),
            cell(&hit["downloads"]),
            description,
        ]);
    }
    table.print_or("Nothing found.");
    let offset = results["offset"].as_u64().unwrap_or_default();
    let count = results["hits"].as_array().map_or(0, Vec::len) as u64;
    if count > 0 {
        println!(
            "Showing {}-{} of {}",
            offset + 1,
            offset + count,
            cell(&results["total_hits"])
        );
    }
}

fn summarize_list(value: &Value, max: usize) -> String {
    let items: Vec<String> = value
        .as_array()
        .map(|a| a.iter().map(cell).collect())
        .unwrap_or_default();
    if items.len() <= max {
        items.join(", ")
    } else {
        // game versions come oldest first: show the newest
        format!(
            "{} ... ({} in all)",
            items[items.len() - max..].join(", "),
            items.len()
        )
    }
}

fn print_project(project: &Value, body: bool) {
    let page = format!(
        "https://modrinth.com/{}/{}",
        str_of(project, "project_type"),
        str_of(project, "slug")
    );
    print_fields(&[
        ("Title", str_of(project, "title")),
        ("Slug", str_of(project, "slug")),
        ("Id", str_of(project, "id")),
        ("Type", str_of(project, "project_type")),
        ("Summary", str_of(project, "description")),
        ("Downloads", cell(&project["downloads"])),
        ("Followers", cell(&project["followers"])),
        ("Categories", cell(&project["categories"])),
        ("Loaders", cell(&project["loaders"])),
        (
            "Game versions",
            summarize_list(&project["game_versions"], 8),
        ),
        ("Server side", cell(&project["server_side"])),
        ("License", cell(&project["license_id"])),
        ("Page", page),
        ("Source", cell(&project["source_url"])),
        ("Issues", cell(&project["issues_url"])),
        ("Wiki", cell(&project["wiki_url"])),
        ("Discord", cell(&project["discord_url"])),
        ("Updated", cell(&project["updated"])),
    ]);
    if body {
        println!("\n{}", str_of(project, "body"));
    }
}

fn print_versions(versions: &[Value]) {
    let mut table = Table::new([
        "VERSION",
        "TYPE",
        "GAME VERSIONS",
        "LOADERS",
        "PUBLISHED",
        "ID",
    ]);
    for v in versions {
        let date: String = str_of(v, "date_published").chars().take(10).collect();
        table.row([
            str_of(v, "version_number"),
            str_of(v, "version_type"),
            summarize_list(&v["game_versions"], 4),
            cell(&v["loaders"]),
            date,
            str_of(v, "id"),
        ]);
    }
    table.print_or("No versions.");
}

fn search_query(page: &PageArgs, query: &str) -> Vec<(&'static str, String)> {
    let mut params = vec![
        ("index", page.sort.as_str().to_string()),
        ("limit", page.limit.to_string()),
        ("offset", page.offset.to_string()),
    ];
    if !query.is_empty() {
        params.push(("query", query.to_string()));
    }
    params
}

async fn show_project(ctx: &Ctx, conn: &Connection, input: &str, body: bool) -> CliResult<()> {
    let id = match resolve_target(conn, input).await? {
        ModrinthTarget::Project { id, .. } => id,
        ModrinthTarget::Version(v) => str_of(&v, "project_id"),
    };
    let project = conn.get(&["modrinth", "project", &id]).await?;
    ctx.out.show(&project, || print_project(&project, body));
    Ok(())
}

async fn project_id(conn: &Connection, input: &str) -> CliResult<String> {
    Ok(match resolve_target(conn, input).await? {
        ModrinthTarget::Project { id, .. } => id,
        ModrinthTarget::Version(v) => str_of(&v, "project_id"),
    })
}

pub async fn modrinth(ctx: &Ctx, cmd: ModrinthCmd) -> CliResult<()> {
    let conn = ctx.connect()?;
    match cmd {
        ModrinthCmd::Search {
            query,
            project_type,
            game_version,
            loader,
            page,
        } => {
            let query = query.join(" ");
            if is_modrinth_link(&query) {
                return show_project(ctx, &conn, &query, false).await;
            }
            let mut params = search_query(&page, &query);
            params.push(("project_type", project_type.as_str().to_string()));
            if let Some(v) = game_version {
                params.push(("game_version", v));
            }
            if let Some(l) = loader {
                params.push(("loader", l));
            }
            let results = conn.get_query(&["modrinth", "search"], &params).await?;
            ctx.out.show(&results, || print_search(&results));
            Ok(())
        }
        ModrinthCmd::Show { project, body } => show_project(ctx, &conn, &project, body).await,
        ModrinthCmd::Versions {
            project,
            game_version,
            loader,
        } => {
            let id = project_id(&conn, &project).await?;
            let mut params = Vec::new();
            if let Some(v) = game_version {
                params.push(("game_version", v));
            }
            if let Some(l) = loader {
                params.push(("loader", l));
            }
            let versions = conn
                .get_query(&["modrinth", "project", &id, "versions"], &params)
                .await?;
            ctx.out.show(&versions, || {
                print_versions(versions.as_array().map(Vec::as_slice).unwrap_or_default())
            });
            Ok(())
        }
        ModrinthCmd::Resolve { url } => {
            let target = conn
                .get_query(&["modrinth", "resolve"], &[("url", url)])
                .await?;
            ctx.out.show(&target, || {
                print_fields(&[
                    ("Project", str_of(&target, "project_id")),
                    ("Slug", str_of(&target, "slug")),
                    ("Type", str_of(&target, "project_type")),
                    ("Version", cell(&target["version_id"])),
                ])
            });
            Ok(())
        }
    }
}

/* ------------------------------------------------------- instance mods */

/// Finds installed mods by file name (with or without `.jar`/`.disabled`), Modrinth slug,
/// project id or title, or the name or id in the jar.
pub fn find_installed<'a>(mods: &'a [Value], query: &str) -> CliResult<&'a Value> {
    let q = query.trim().to_lowercase();
    let stem = |name: &str| {
        name.to_lowercase()
            .trim_end_matches(".disabled")
            .trim_end_matches(".jar")
            .to_string()
    };
    let passes: [&dyn Fn(&Value) -> bool; 3] = [
        &|m| str_of(m, "file_name") == query,
        &|m| {
            let name = str_of(m, "file_name");
            name.to_lowercase() == q || stem(&name) == stem(&q)
        },
        &|m| {
            [
                &m["modrinth"]["slug"],
                &m["modrinth"]["project_id"],
                &m["modrinth"]["title"],
                &m["metadata"]["id"],
                &m["metadata"]["name"],
            ]
            .iter()
            .any(|v| v.as_str().is_some_and(|s| s.to_lowercase() == q))
        },
    ];
    for pass in passes {
        let matches: Vec<&Value> = mods.iter().filter(|m| pass(m)).collect();
        match matches.as_slice() {
            [] => continue,
            [one] => return Ok(one),
            many => {
                return Err(CliError::new(
                    kind::AMBIGUOUS,
                    format!(
                        "`{query}` matches several files; use a file name: {}",
                        many.iter()
                            .map(|m| str_of(m, "file_name"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ))
            }
        }
    }
    Err(CliError::not_found(format!(
        "No installed mod or plugin `{query}` (see `mods <instance> list`)"
    )))
}

async fn mod_list(conn: &Connection, inst: &InstanceRef, check_updates: bool) -> CliResult<Value> {
    let query = if check_updates {
        vec![("check_updates", "true".to_string())]
    } else {
        vec![]
    };
    conn.get_query(&["instance", &inst.uuid, "mods"], &query)
        .await
}

fn mods_of(list: &Value) -> Vec<Value> {
    list["mods"].as_array().cloned().unwrap_or_default()
}

/// After a change: tells people to restart if the server runs.
async fn restart_note(ctx: &Ctx, conn: &Connection, inst: &InstanceRef) {
    if ctx.out.json {
        return;
    }
    if let Ok(list) = mod_list(conn, inst, false).await {
        if list["restart_required"].as_bool().unwrap_or(false) {
            ctx.out.note("Restart the server to apply the changes.");
        }
    }
}

fn print_install_result(result: &Value) {
    let names = |key: &str| -> Vec<String> {
        result[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| match m.as_str() {
                Some(s) => s.to_string(),
                None => format!("{} ({})", mod_name(m), str_of(m, "file_name")),
            })
            .collect()
    };
    let installed = names("installed");
    if installed.is_empty() {
        println!("Nothing was installed.");
    } else {
        println!("Installed: {}", installed.join(", "));
    }
    let removed = names("removed");
    if !removed.is_empty() {
        println!("Removed old versions: {}", removed.join(", "));
    }
    for skipped in result["skipped"].as_array().into_iter().flatten() {
        println!(
            "Skipped {}: {}",
            str_of(skipped, "project_id"),
            str_of(skipped, "reason")
        );
    }
}

/// The version to install for `target` on an instance with `loader` and `game_version`.
async fn version_to_install(
    conn: &Connection,
    target: &str,
    wanted: Option<&str>,
    loader: &str,
    game_version: &str,
) -> CliResult<String> {
    match resolve_target(conn, target).await? {
        ModrinthTarget::Version(version) => {
            if wanted.is_some() {
                return Err(CliError::usage(
                    "--version only applies to a project, not to a version id",
                ));
            }
            Ok(str_of(&version, "id"))
        }
        ModrinthTarget::Project {
            version_id: Some(v),
            ..
        } if wanted.is_none() => Ok(v),
        ModrinthTarget::Project { id, .. } => {
            let versions = conn.get(&["modrinth", "project", &id, "versions"]).await?;
            let versions = versions.as_array().cloned().unwrap_or_default();
            if wanted.is_some() {
                return pick_version(&versions, wanted);
            }
            let compatible: Vec<Value> = versions
                .into_iter()
                .filter(|v| is_compatible(v, loader, game_version))
                .collect();
            if compatible.is_empty() {
                return Err(CliError::not_found(format!(
                    "No version of `{target}` is for {loader} {game_version}. See `mods <instance> versions {target} --all`, then pass --version and --force"
                )));
            }
            pick_version(&compatible, None)
        }
    }
}

async fn install(
    conn: &Connection,
    inst: &InstanceRef,
    version_id: &str,
    deps: bool,
    force: bool,
) -> CliResult<Value> {
    conn.post(
        &["instance", &inst.uuid, "mods", "install"],
        Body::Json(json!({
            "version_id": version_id,
            "install_dependencies": deps,
            "force": force,
        })),
    )
    .await
}

pub async fn run(ctx: &Ctx, args: ModsArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    let inst = resolve::instance(&conn, &args.instance).await?;
    match args.action {
        ModsCmd::List { check_updates } => {
            let list = mod_list(&conn, &inst, check_updates).await?;
            ctx.out.show(&list, || print_mod_list(&list, check_updates));
            Ok(())
        }
        ModsCmd::Search {
            query,
            all_versions,
            page,
        } => {
            let query = query.join(" ");
            if is_modrinth_link(&query) {
                return show_project(ctx, &conn, &query, false).await;
            }
            let list = mod_list(&conn, &inst, false).await?;
            let mut params = search_query(&page, &query);
            params.push(("project_type", str_of(&list, "kind")));
            if !all_versions {
                params.push(("game_version", str_of(&list, "game_version")));
                params.push(("loader", str_of(&list, "loader")));
            }
            let mut results = conn.get_query(&["modrinth", "search"], &params).await?;
            // mark the projects that are installed, like the dashboard's badge
            let installed: Vec<String> = mods_of(&list)
                .iter()
                .filter_map(|m| m["modrinth"]["project_id"].as_str().map(str::to_string))
                .collect();
            for hit in results["hits"].as_array_mut().into_iter().flatten() {
                let is_installed = installed.contains(&str_of(hit, "project_id"));
                hit["installed"] = json!(is_installed);
            }
            ctx.out.show(&results, || print_search(&results));
            Ok(())
        }
        ModsCmd::Show { project, body } => show_project(ctx, &conn, &project, body).await,
        ModsCmd::Versions { project, all } => {
            let id = project_id(&conn, &project).await?;
            let versions = conn.get(&["modrinth", "project", &id, "versions"]).await?;
            let mut versions = versions.as_array().cloned().unwrap_or_default();
            if !all {
                let list = mod_list(&conn, &inst, false).await?;
                let (loader, game_version) =
                    (str_of(&list, "loader"), str_of(&list, "game_version"));
                versions.retain(|v| is_compatible(v, &loader, &game_version));
            }
            ctx.out.show(&json!(versions), || print_versions(&versions));
            Ok(())
        }
        ModsCmd::Install {
            target,
            version,
            no_deps,
            force,
        } => {
            let list = mod_list(&conn, &inst, false).await?;
            let version_id = version_to_install(
                &conn,
                &target,
                version.as_deref(),
                &str_of(&list, "loader"),
                &str_of(&list, "game_version"),
            )
            .await?;
            let result = install(&conn, &inst, &version_id, !no_deps, force).await?;
            ctx.out.show(&result, || print_install_result(&result));
            restart_note(ctx, &conn, &inst).await;
            Ok(())
        }
        ModsCmd::Upload { jars, overwrite } => {
            let parts = jars
                .iter()
                .map(|jar| multipart::Part::for_file(jar))
                .collect::<CliResult<Vec<_>>>()?;
            let query = if overwrite {
                vec![("overwrite", "true".to_string())]
            } else {
                vec![]
            };
            let result = conn
                .json(
                    Method::POST,
                    &["instance", &inst.uuid, "mods", "upload"],
                    &query,
                    multipart::body("file", parts, |_| {}),
                )
                .await
                .map_err(|mut e| {
                    if !overwrite
                        && (e.kind == kind::CONFLICT || e.message.to_lowercase().contains("exist"))
                    {
                        e.message.push_str(" (pass --overwrite to replace it)");
                    }
                    e
                })?;
            ctx.out.show(&result, || {
                for m in result.as_array().into_iter().flatten() {
                    println!("Uploaded {} ({})", mod_name(m), str_of(m, "file_name"));
                }
            });
            restart_note(ctx, &conn, &inst).await;
            Ok(())
        }
        ModsCmd::Enable { mods } => toggle(ctx, &conn, &inst, &mods, "enable").await,
        ModsCmd::Disable { mods } => toggle(ctx, &conn, &inst, &mods, "disable").await,
        ModsCmd::Remove { mods } => {
            let list = mod_list(&conn, &inst, false).await?;
            let installed = mods_of(&list);
            let files = mods
                .iter()
                .map(|q| find_installed(&installed, q).map(|m| str_of(m, "file_name")))
                .collect::<CliResult<Vec<_>>>()?;
            prompt::confirm(
                ctx.global.yes,
                &format!("Delete {} from {inst}?", files.join(", ")),
            )?;
            for file in &files {
                conn.delete(&["instance", &inst.uuid, "mods", file]).await?;
            }
            ctx.out.done(
                format!("Removed {}", files.join(", ")),
                json!({"uuid": inst.uuid, "removed": files}),
            );
            restart_note(ctx, &conn, &inst).await;
            Ok(())
        }
        ModsCmd::Update { r#mod, all } => {
            let list = mod_list(&conn, &inst, true).await?;
            let installed = mods_of(&list);
            let selected: Vec<&Value> = if all {
                installed.iter().collect()
            } else {
                vec![find_installed(
                    &installed,
                    r#mod.as_deref().expect("clap requires a mod without --all"),
                )?]
            };
            let mut results = Vec::new();
            for m in selected {
                let Some(update) = m["modrinth"]["update"].as_object() else {
                    if !all {
                        ctx.out.note(format!("{} is up to date", mod_name(m)));
                    }
                    continue;
                };
                let version_id = update
                    .get("version_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                ctx.out.note(format!(
                    "Updating {} {} -> {}",
                    mod_name(m),
                    mod_version(m),
                    update.get("version_number").map(cell).unwrap_or_default()
                ));
                let result = install(&conn, &inst, &version_id, true, false).await?;
                results.push(json!({
                    "file_name": m["file_name"],
                    "from": m["modrinth"]["version_number"],
                    "to": update.get("version_number"),
                    "result": result,
                }));
            }
            let doc = json!({"uuid": inst.uuid, "updated": results});
            ctx.out.show(&doc, || {
                if results.is_empty() {
                    println!("Everything is up to date.");
                } else {
                    println!("Updated {} mod(s).", results.len());
                }
            });
            if !results.is_empty() {
                restart_note(ctx, &conn, &inst).await;
            }
            Ok(())
        }
    }
}

async fn toggle(
    ctx: &Ctx,
    conn: &Connection,
    inst: &InstanceRef,
    queries: &[String],
    action: &str,
) -> CliResult<()> {
    let list = mod_list(conn, inst, false).await?;
    let installed = mods_of(&list);
    let mut changed = Vec::new();
    for query in queries {
        let file = str_of(find_installed(&installed, query)?, "file_name");
        let result = conn
            .put(
                &["instance", &inst.uuid, "mods", &file, action],
                Body::Empty,
            )
            .await?;
        changed.push(result);
    }
    let past = if action == "enable" {
        "Enabled"
    } else {
        "Disabled"
    };
    let doc = json!(changed);
    ctx.out.show(&doc, || {
        for m in &changed {
            println!("{past} {} ({})", mod_name(m), str_of(m, "file_name"));
        }
    });
    restart_note(ctx, conn, inst).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> Vec<Value> {
        vec![
            json!({"file_name": "fabric-api-0.92.jar", "enabled": true,
                   "metadata": {"id": "fabric-api", "name": "Fabric API"},
                   "modrinth": {"project_id": "P7dR8mSH", "slug": "fabric-api", "title": "Fabric API"}}),
            json!({"file_name": "sodium-0.5.jar.disabled", "enabled": false,
                   "metadata": {"id": "sodium", "name": "Sodium"}, "modrinth": null}),
            json!({"file_name": "custom.jar", "enabled": true, "metadata": null, "modrinth": null}),
        ]
    }

    #[test]
    fn find_installed_by_any_name() {
        let mods = installed();
        for q in [
            "fabric-api-0.92.jar",
            "fabric-api",
            "Fabric API",
            "P7dR8mSH",
        ] {
            assert_eq!(
                str_of(find_installed(&mods, q).unwrap(), "file_name"),
                "fabric-api-0.92.jar",
                "{q}"
            );
        }
        for q in [
            "sodium-0.5.jar.disabled",
            "sodium-0.5.jar",
            "sodium-0.5",
            "sodium",
        ] {
            assert_eq!(
                str_of(find_installed(&mods, q).unwrap(), "file_name"),
                "sodium-0.5.jar.disabled",
                "{q}"
            );
        }
        assert_eq!(
            str_of(find_installed(&mods, "custom").unwrap(), "file_name"),
            "custom.jar"
        );
        assert_eq!(
            find_installed(&mods, "lithium").unwrap_err().kind,
            kind::NOT_FOUND
        );
    }

    #[test]
    fn version_picking() {
        let versions = vec![
            json!({"id": "c", "version_number": "3.0-beta", "version_type": "beta"}),
            json!({"id": "b", "version_number": "2.0", "version_type": "release"}),
            json!({"id": "a", "version_number": "1.0", "version_type": "release"}),
        ];
        assert_eq!(pick_version(&versions, None).unwrap(), "b");
        assert_eq!(pick_version(&versions, Some("1.0")).unwrap(), "a");
        assert_eq!(pick_version(&versions, Some("c")).unwrap(), "c");
        assert_eq!(
            pick_version(&versions, Some("9")).unwrap_err().kind,
            kind::NOT_FOUND
        );
        assert_eq!(pick_version(&versions[..1], None).unwrap(), "c");
        assert!(pick_version(&[], None).is_err());
    }

    #[test]
    fn compatibility() {
        let v = json!({"loaders": ["bukkit", "spigot"], "game_versions": ["1.20.1", "1.20.4"]});
        assert!(is_compatible(&v, "paper", "1.20.1"));
        assert!(is_compatible(&v, "spigot", "1.20.4"));
        assert!(!is_compatible(&v, "paper", "1.21"));
        assert!(!is_compatible(&v, "fabric", "1.20.1"));
        let f = json!({"loaders": ["fabric", "quilt"], "game_versions": ["1.21.1"]});
        assert!(is_compatible(&f, "fabric", "1.21.1"));
        assert!(!is_compatible(&f, "forge", "1.21.1"));
    }

    #[test]
    fn links() {
        assert!(is_modrinth_link("https://modrinth.com/mod/sodium"));
        assert!(is_modrinth_link("modrinth.com/mod/sodium/version/1.0"));
        assert!(!is_modrinth_link("sodium"));
    }
}
