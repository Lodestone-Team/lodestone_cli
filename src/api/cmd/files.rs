//! `files`: an instance's files (paths relative to its folder), or with `--global` the
//! core machine's file system (absolute paths). The core takes paths base64-encoded
//! (URL-safe, no padding) in its routes, as the dashboard sends them.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use reqwest::Method;
use serde_json::json;

use crate::api::b64::encode_path;
use crate::api::cli::{FilesArgs, FilesCmd};
use crate::api::error::{kind, CliError, CliResult};
use crate::api::events::{Target, TaskWaiter};
use crate::api::http::{Body, Connection};
use crate::api::multipart;
use crate::api::output::{cell, format_bytes, format_unix_seconds, Table};
use crate::api::resolve::{self, InstanceRef};
use crate::api::{prompt, Ctx};

/// The core refuses bigger write bodies.
const MAX_WRITE_BYTES: usize = 64 * 1024 * 1024;

/// Whose files: an instance's, or the machine's.
enum Scope {
    Instance(InstanceRef),
    Global,
}

impl Scope {
    /// The route for `op` on `path`.
    fn segments(&self, path: &str, op: &str) -> Vec<String> {
        match self {
            Scope::Instance(inst) => vec![
                "instance".into(),
                inst.uuid.clone(),
                "fs".into(),
                encode_path(path),
                op.into(),
            ],
            Scope::Global => vec!["fs".into(), encode_path(path), op.into()],
        }
    }

    fn instance(&self, what: &str) -> CliResult<&InstanceRef> {
        match self {
            Scope::Instance(inst) => Ok(inst),
            Scope::Global => Err(CliError::new(
                kind::UNSUPPORTED,
                format!(
                    "`files {what}` works on instances only; the core has no global route for it"
                ),
            )),
        }
    }

    fn default_dir(&self) -> CliResult<String> {
        match self {
            Scope::Instance(_) => Ok(".".to_string()),
            Scope::Global => Err(CliError::usage("With --global, give an absolute path")),
        }
    }
}

fn refs(segments: &[String]) -> Vec<&str> {
    segments.iter().map(String::as_str).collect()
}

pub async fn run(ctx: &Ctx, args: FilesArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    let scope = match &args.instance {
        Some(instance) if !args.global => {
            Scope::Instance(resolve::instance(&conn, instance).await?)
        }
        _ => Scope::Global,
    };
    match args.action {
        FilesCmd::Ls { path } => {
            let path = match path {
                Some(p) => p,
                None => scope.default_dir()?,
            };
            let mut list = conn.get(&refs(&scope.segments(&path, "ls"))).await?;
            if let Some(items) = list.as_array_mut() {
                items.sort_by_key(|f| {
                    (
                        f["file_type"] != "Directory",
                        f["name"].as_str().unwrap_or_default().to_lowercase(),
                    )
                });
            }
            ctx.out.show(&list, || {
                let mut table = Table::new(["TYPE", "SIZE", "MODIFIED", "NAME"]);
                for f in list.as_array().into_iter().flatten() {
                    let is_dir = f["file_type"] == "Directory";
                    table.row([
                        if is_dir { "dir" } else { "file" }.to_string(),
                        f["size"]
                            .as_u64()
                            .map(format_bytes)
                            .unwrap_or_else(|| "-".into()),
                        f["modification_time"]
                            .as_i64()
                            .map(format_unix_seconds)
                            .unwrap_or_else(|| "-".into()),
                        if is_dir {
                            format!("{}/", cell(&f["name"]))
                        } else {
                            cell(&f["name"])
                        },
                    ]);
                }
                table.print_or("(empty)");
            });
            Ok(())
        }
        FilesCmd::Cat { path } => {
            let content = conn
                .text(
                    Method::GET,
                    &refs(&scope.segments(&path, "read")),
                    &[],
                    Body::Empty,
                )
                .await?;
            if ctx.out.json {
                ctx.out.json(&json!({"path": path, "content": content}));
            } else {
                print!("{content}");
                let _ = std::io::stdout().flush();
            }
            Ok(())
        }
        FilesCmd::Download { path, dest } => download(ctx, &conn, &scope, &path, dest).await,
        FilesCmd::Upload { files, to } => {
            let dir = match to {
                Some(d) => d,
                None => scope.default_dir()?,
            };
            let parts = files
                .iter()
                .map(|f| multipart::Part::for_file(f))
                .collect::<CliResult<Vec<_>>>()?;
            let names: Vec<String> = parts.iter().map(|p| p.file_name.clone()).collect();
            let total: u64 = parts.iter().map(|p| p.size).sum();
            let bar = progress_bar(ctx, Some(total));
            let counter = bar.clone();
            let body = multipart::body("file", parts, move |n| {
                if let Some(bar) = &counter {
                    bar.inc(n);
                }
            });
            conn.put(&refs(&scope.segments(&dir, "upload")), body)
                .await?;
            if let Some(bar) = bar {
                bar.finish_and_clear();
            }
            ctx.out.done(
                format!("Uploaded {} to {dir}", names.join(", ")),
                json!({"uploaded": names, "to": dir}),
            );
            Ok(())
        }
        FilesCmd::Write { path, from } => {
            let content = match &from {
                Some(file) => std::fs::read(file)
                    .map_err(|e| CliError::io(format!("Cannot read {}", file.display()), e))?,
                None => {
                    if std::io::stdin().is_terminal() {
                        ctx.out
                            .note("Reading the new contents from stdin; end with Ctrl-D");
                    }
                    let mut buffer = Vec::new();
                    std::io::stdin()
                        .take(MAX_WRITE_BYTES as u64 + 1)
                        .read_to_end(&mut buffer)
                        .map_err(|e| CliError::io("Failed to read stdin", e))?;
                    buffer
                }
            };
            if content.len() > MAX_WRITE_BYTES {
                return Err(CliError::usage(
                    "The core accepts at most 64 MiB per write; use `files upload` for big files",
                ));
            }
            let size = content.len();
            conn.put(&refs(&scope.segments(&path, "write")), Body::Bytes(content))
                .await?;
            ctx.out.done(
                format!("Wrote {} to {path}", format_bytes(size as u64)),
                json!({"path": path, "bytes": size}),
            );
            Ok(())
        }
        FilesCmd::Mkdir { path } => {
            conn.put(&refs(&scope.segments(&path, "mkdir")), Body::Empty)
                .await?;
            ctx.out
                .done(format!("Created folder {path}"), json!({"path": path}));
            Ok(())
        }
        FilesCmd::Touch { path } => {
            conn.put(&refs(&scope.segments(&path, "new")), Body::Empty)
                .await?;
            ctx.out
                .done(format!("Created file {path}"), json!({"path": path}));
            Ok(())
        }
        FilesCmd::Rm { paths, recursive } => {
            let what = if recursive {
                " and everything in them"
            } else {
                ""
            };
            prompt::confirm(
                ctx.global.yes,
                &format!("Delete {}{what}?", paths.join(", ")),
            )?;
            let op = if recursive { "rmdir" } else { "rm" };
            for path in &paths {
                conn.delete(&refs(&scope.segments(path, op))).await?;
            }
            ctx.out.done(
                format!("Deleted {}", paths.join(", ")),
                json!({"deleted": paths}),
            );
            Ok(())
        }
        FilesCmd::Mv { from, to } => {
            let mut segments = scope.segments(&from, "move");
            segments.push(encode_path(&to));
            conn.put(&refs(&segments), Body::Empty).await?;
            ctx.out.done(
                format!("Moved {from} to {to}"),
                json!({"from": from, "to": to}),
            );
            Ok(())
        }
        FilesCmd::Cp { sources, dest } => {
            let inst = scope.instance("cp")?;
            conn.put(
                &["instance", &inst.uuid, "fs", "cpr"],
                Body::Json(json!({"relative_paths_source": sources, "relative_path_dest": dest})),
            )
            .await?;
            ctx.out.done(
                format!("Copied {} into {dest}", sources.join(", ")),
                json!({"sources": sources, "dest": dest}),
            );
            Ok(())
        }
        FilesCmd::Zip {
            paths,
            output,
            wait,
        } => {
            let inst = scope.instance("zip")?.clone();
            let body = json!({"target_relative_paths": paths, "destination_relative_path": output});
            let request = || async {
                conn.put(
                    &["instance", &inst.uuid, "fs", "zip"],
                    Body::Json(body.clone()),
                )
                .await
            };
            if wait {
                let waiter = TaskWaiter {
                    conn: &conn,
                    out: &ctx.out,
                    timeout: None,
                };
                let (_, task) = waiter
                    .wait(Target::NewTaskWithPrefix("Zipping ".into()), request)
                    .await?;
                ctx.out.done(
                    format!("Zipped {} into {output}", paths.join(", ")),
                    json!({"output": output, "task": task.to_json()}),
                );
            } else {
                request().await?;
                ctx.out.done(
                    format!(
                        "Zipping {} into {output} in the background (see `lodestone_cli tasks`)",
                        paths.join(", ")
                    ),
                    json!({"output": output, "state": "started"}),
                );
            }
            Ok(())
        }
        FilesCmd::Unzip {
            path,
            to,
            smart,
            into_folder,
            wait,
        } => {
            scope.instance("unzip")?;
            let option = match (to, smart, into_folder) {
                (Some(dir), _, _) => json!({ "ToDir": dir }),
                (None, true, _) => json!("Smart"),
                (None, false, true) => json!("ToDirectoryWithFileName"),
                (None, false, false) => json!("Normal"),
            };
            let segments = scope.segments(&path, "unzip");
            let request = || async { conn.put(&refs(&segments), Body::Json(option.clone())).await };
            if wait {
                let waiter = TaskWaiter {
                    conn: &conn,
                    out: &ctx.out,
                    timeout: None,
                };
                let (_, task) = waiter
                    .wait(Target::NewTaskWithPrefix("Unzipping ".into()), request)
                    .await?;
                ctx.out.done(
                    format!("Extracted {path}"),
                    json!({"path": path, "task": task.to_json()}),
                );
            } else {
                request().await?;
                ctx.out.done(
                    format!("Extracting {path} in the background (see `lodestone_cli tasks`)"),
                    json!({"path": path, "state": "started"}),
                );
            }
            Ok(())
        }
    }
}

fn progress_bar(ctx: &Ctx, total: Option<u64>) -> Option<indicatif::ProgressBar> {
    if ctx.out.json || !std::io::stderr().is_terminal() {
        return None;
    }
    let bar = match total {
        Some(total) => indicatif::ProgressBar::new(total),
        None => indicatif::ProgressBar::new_spinner(),
    };
    bar.set_draw_target(indicatif::ProgressDrawTarget::stderr());
    if let Ok(style) = indicatif::ProgressStyle::with_template(
        "{bar:40} {bytes}/{total_bytes} {bytes_per_sec} {eta}",
    ) {
        bar.set_style(style);
    }
    Some(bar)
}

/// The file name from a `Content-Disposition: attachment; filename="..."` header, made
/// safe as a plain file name.
pub fn attachment_file_name(header: &str) -> Option<String> {
    let start = header.find("filename=")? + "filename=".len();
    let rest = &header[start..];
    let name = if let Some(quoted) = rest.strip_prefix('"') {
        quoted.split('"').next()?
    } else {
        rest.split(';').next()?.trim()
    };
    safe_file_name(name)
}

fn safe_file_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next()?.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(name.chars().filter(|c| !c.is_control()).collect())
}

async fn download(
    ctx: &Ctx,
    conn: &Connection,
    scope: &Scope,
    path: &str,
    dest: Option<PathBuf>,
) -> CliResult<()> {
    let op = match scope {
        Scope::Instance(_) => "url",
        Scope::Global => "download",
    };
    ctx.out.note(format!("Preparing {path}..."));
    let key = conn
        .text(
            Method::GET,
            &refs(&scope.segments(path, op)),
            &[],
            Body::Empty,
        )
        .await?;
    let key = key.trim().trim_matches('"').to_string();
    let response = conn
        .send(Method::GET, &["file", &key], &[], Body::Empty)
        .await?;
    let total = response.content_length();
    let header_name = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(attachment_file_name);
    let file_name = header_name
        .or_else(|| safe_file_name(path))
        .unwrap_or_else(|| "download".to_string());
    let to_stdout = dest.as_deref() == Some(Path::new("-"));
    let target = match dest {
        _ if to_stdout => None,
        None => Some(PathBuf::from(&file_name)),
        Some(d) if d.is_dir() => Some(d.join(&file_name)),
        Some(d) => Some(d),
    };
    let mut stream = response.bytes_stream();
    let mut written = 0u64;
    let Some(target) = target else {
        let mut stdout = std::io::stdout().lock();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| CliError::new(kind::NETWORK, format!("Download failed: {e}")))?;
            stdout
                .write_all(&chunk)
                .map_err(|e| CliError::io("Failed to write to stdout", e))?;
        }
        let _ = stdout.flush();
        return Ok(());
    };
    if target.exists() {
        prompt::confirm(
            ctx.global.yes,
            &format!("{} exists. Replace it?", target.display()),
        )?;
    }
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| CliError::io(format!("Cannot write in {}", dir.display()), e))?;
    let bar = progress_bar(ctx, total);
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| CliError::new(kind::NETWORK, format!("Download failed: {e}")))?;
        temp.write_all(&chunk)
            .map_err(|e| CliError::io(format!("Failed to write {}", target.display()), e))?;
        written += chunk.len() as u64;
        if let Some(bar) = &bar {
            bar.set_position(written);
        }
    }
    if let Some(bar) = bar {
        bar.finish_and_clear();
    }
    if total.is_some_and(|t| t != written) {
        return Err(CliError::new(
            kind::NETWORK,
            format!(
                "The download ended early ({written} of {} bytes)",
                total.unwrap_or_default()
            ),
        ));
    }
    temp.persist(&target)
        .map_err(|e| CliError::io(format!("Failed to save {}", target.display()), e.error))?;
    ctx.out.done(
        format!(
            "Saved {path} to {} ({})",
            target.display(),
            format_bytes(written)
        ),
        json!({"path": path, "saved_to": target.display().to_string(), "bytes": written}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names() {
        assert_eq!(
            attachment_file_name("attachment; filename=\"world.zip\"").unwrap(),
            "world.zip"
        );
        assert_eq!(
            attachment_file_name("attachment; filename=server.properties").unwrap(),
            "server.properties"
        );
        assert_eq!(
            attachment_file_name("attachment; filename=\"../../etc/passwd\"").unwrap(),
            "passwd"
        );
        assert_eq!(
            attachment_file_name("attachment; filename=\"a\\\\b.txt\"").unwrap(),
            "b.txt"
        );
        assert!(attachment_file_name("attachment; filename=\"..\"").is_none());
        assert!(attachment_file_name("inline").is_none());
    }

    #[test]
    fn routes_encode_paths() {
        let scope = Scope::Instance(InstanceRef {
            uuid: "INSTANCE_1".into(),
            name: "x".into(),
        });
        assert_eq!(
            scope.segments("world", "ls"),
            ["instance", "INSTANCE_1", "fs", "d29ybGQ", "ls"]
        );
        assert_eq!(
            Scope::Global.segments("/tmp", "read"),
            ["fs", "L3RtcA", "read"]
        );
        assert_eq!(
            Scope::Global.instance("zip").unwrap_err().kind,
            kind::UNSUPPORTED
        );
    }
}
