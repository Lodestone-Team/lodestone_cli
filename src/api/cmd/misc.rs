//! `events`, `tasks`, `playit` and `extension`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::api::cli::{EventsArgs, ExtensionCmd, PlayitCmd, TasksArgs};
use crate::api::error::{CliError, CliResult};
use crate::api::events::{self, EventFilter, Target, TaskWaiter, Tasks};
use crate::api::http::{Body, Connection};
use crate::api::output::{cell, format_unix_millis, print_fields, Table};
use crate::api::ws::{self, Backoff, Next};
use crate::api::{resolve, Ctx};

/// Parses an age such as `90s`, `30m`, `12h`, `7d` or `2w`.
pub fn parse_age(input: &str) -> CliResult<Duration> {
    let input = input.trim();
    let split = input
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(input.len());
    let (number, unit) = input.split_at(split);
    let number: u64 = number
        .parse()
        .map_err(|_| CliError::usage(format!("`{input}` is not an age such as 30m, 12h or 7d")))?;
    let seconds = match unit {
        "s" => 1,
        "m" | "" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 7 * 86400,
        _ => {
            return Err(CliError::usage(format!(
                "`{input}`: use s, m, h, d or w as the unit"
            )))
        }
    };
    Ok(Duration::from_secs(number * seconds))
}

fn snowflake(event: &Value) -> i64 {
    event["snowflake"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default()
}

async fn build_filter(conn: &Connection, args: &EventsArgs) -> CliResult<EventFilter> {
    let mut filter = EventFilter {
        levels: args.level.iter().map(|l| l.as_str().to_string()).collect(),
        types: args
            .event_type
            .iter()
            .map(|t| t.as_str().to_string())
            .collect(),
        ..EventFilter::default()
    };
    for instance in &args.instance {
        filter
            .instance_ids
            .push(resolve::instance(conn, instance).await?.uuid);
    }
    for user in &args.user {
        filter.user_ids.push(resolve::user(conn, user).await?.uid);
    }
    if let Some(since) = &args.since {
        let now = chrono::Utc::now().timestamp_millis();
        let age = parse_age(since)?.as_millis() as i64;
        filter.time_range = Some((now - age, now));
    }
    Ok(filter)
}

fn print_event(ctx: &Ctx, event: &Value) -> CliResult<()> {
    if ctx.out.json {
        ctx.out.json_line(event)
    } else {
        ctx.out.line(events::format_event_line(event))
    }
}

pub async fn events(ctx: &Ctx, args: EventsArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    let filter = build_filter(&conn, &args).await?;
    let fetch = |filter: &EventFilter| {
        let query = filter.to_query(None).to_string();
        let segments: &[&str] = if filter.time_range.is_some() {
            &["events", "search"]
        } else {
            &["events", "all", "buffer"]
        };
        let conn = &conn;
        async move {
            let events = conn.get_query(segments, &[("filter", query)]).await?;
            let mut events = events.as_array().cloned().unwrap_or_default();
            if !args.include_console {
                events.retain(|e| !events::is_console_event(e));
            }
            events.sort_by_key(snowflake);
            if let Some(limit) = args.limit {
                if events.len() > limit {
                    events.drain(..events.len() - limit);
                }
            }
            Ok::<_, CliError>(events)
        }
    };
    if !args.follow {
        let events = fetch(&filter).await?;
        ctx.out.show(&json!(events), || {
            for event in &events {
                println!("{}", events::format_event_line(event));
            }
            if events.is_empty() {
                println!("No events.");
            }
        });
        return Ok(());
    }
    let token = conn
        .token()
        .map(str::to_string)
        .ok_or_else(|| CliError::new(crate::api::error::kind::NOT_LOGGED_IN, "Not logged in"))?;
    let stream_filter = EventFilter {
        time_range: None,
        ..filter.clone()
    };
    let mut last = 0i64;
    let mut backoff = Backoff::new();
    let mut first = true;
    loop {
        let query = stream_filter.to_query(Some(&token)).to_string();
        let mut socket =
            match ws::connect(&conn, &["events", "all", "stream"], &[("filter", query)]).await {
                Ok(socket) => socket,
                Err(e) if !first && ws::is_retryable(&e) => {
                    ctx.out.note(format!("Reconnecting: {}", e.message));
                    tokio::time::sleep(backoff.next_delay()).await;
                    continue;
                }
                Err(e) => return Err(e),
            };
        backoff.reset();
        let recent = if first {
            fetch(&filter).await?
        } else {
            fetch(&stream_filter).await?
        };
        first = false;
        for event in recent {
            if snowflake(&event) > last {
                last = snowflake(&event);
                print_event(ctx, &event)?;
            }
        }
        loop {
            match ws::next_json(&mut socket).await {
                Next::Json(event) => {
                    if (!args.include_console && events::is_console_event(&event))
                        || snowflake(&event) <= last
                    {
                        continue;
                    }
                    last = snowflake(&event);
                    print_event(ctx, &event)?;
                }
                Next::Closed(reason) => {
                    ctx.out.note(format!(
                        "Event stream closed{}; reconnecting",
                        reason.map(|r| format!(" ({r})")).unwrap_or_default()
                    ));
                    tokio::time::sleep(backoff.next_delay()).await;
                    break;
                }
            }
        }
    }
}

pub async fn tasks(ctx: &Ctx, args: TasksArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    if let Some(id) = args.wait {
        let target_id = if resolve::is_instance_uuid(&id) || id.chars().all(|c| c.is_ascii_digit())
        {
            id
        } else {
            // an instance name: the task that creates or deletes it
            resolve::instance(&conn, &id).await?.uuid
        };
        let waiter = TaskWaiter {
            conn: &conn,
            out: &ctx.out,
            timeout: args.timeout.map(Duration::from_secs),
        };
        let ((), task) = waiter
            .wait(Target::Id(target_id), || async { Ok(()) })
            .await?;
        let doc = task.to_json();
        ctx.out.show(&doc, || {
            println!(
                "{}: {}",
                if task.name.is_empty() {
                    &task.id
                } else {
                    &task.name
                },
                task.end_message.as_deref().unwrap_or("finished")
            )
        });
        return Ok(());
    }
    let filter = EventFilter {
        types: vec!["ProgressionEvent".to_string()],
        ..EventFilter::default()
    };
    let buffer = conn
        .get_query(
            &["events", "all", "buffer"],
            &[("filter", filter.to_query(None).to_string())],
        )
        .await?;
    let mut tasks = Tasks::default();
    let mut events = buffer.as_array().cloned().unwrap_or_default();
    events.sort_by_key(snowflake);
    for event in &events {
        tasks.apply(event);
    }
    let shown: Vec<_> = tasks
        .in_order()
        .filter(|t| args.all || t.ended.is_none())
        .cloned()
        .collect();
    let doc = json!(shown.iter().map(|t| t.to_json()).collect::<Vec<_>>());
    ctx.out.show(&doc, || {
        let mut table = Table::new(["ID", "STATE", "PROGRESS", "STARTED", "NAME", "MESSAGE"]);
        for t in &shown {
            let state = match t.ended {
                None => "running",
                Some(true) => "done",
                Some(false) => "failed",
            };
            let message = t
                .end_message
                .clone()
                .filter(|_| t.ended.is_some())
                .unwrap_or_else(|| t.message.clone());
            table.row([
                t.id.clone(),
                state.to_string(),
                t.percent()
                    .map(|p| format!("{p:.0}%"))
                    .unwrap_or_else(|| "-".into()),
                t.started_millis
                    .map(format_unix_millis)
                    .unwrap_or_else(|| "-".into()),
                t.name.clone(),
                message,
            ]);
        }
        table.print_or(if args.all {
            "No recent tasks."
        } else {
            "No running tasks (--all shows finished ones)."
        });
    });
    Ok(())
}

pub async fn playit(ctx: &Ctx, cmd: PlayitCmd) -> CliResult<()> {
    let conn = ctx.connect()?;
    match cmd {
        PlayitCmd::Status => {
            let running = conn.get(&["playitgg", "cli_is_running"]).await?;
            let key_valid = conn
                .post(&["playitgg", "verify_key"], Body::Empty)
                .await
                .unwrap_or(Value::Bool(false));
            let doc = json!({"running": running, "key_valid": key_valid});
            ctx.out.show(&doc, || {
                print_fields(&[
                    ("Agent running", cell(&running)),
                    ("Key valid", cell(&key_valid)),
                ])
            });
        }
        PlayitCmd::Signup => {
            let data = conn.get(&["playitgg", "generate_signup_link"]).await?;
            ctx.out.show(&data, || {
                println!(
                    "Open this link to link a playit.gg account to the core:\n  {}\nClaim code: {}",
                    cell(&data["url"]),
                    cell(&data["claim_code"])
                )
            });
        }
        PlayitCmd::Verify => {
            let valid = conn.post(&["playitgg", "verify_key"], Body::Empty).await?;
            ctx.out.show(&json!({"key_valid": valid}), || {
                if valid.as_bool().unwrap_or(false) {
                    println!("The playit.gg key is valid");
                } else {
                    println!("There is no valid playit.gg key; run `lodestone_cli playit signup`");
                }
            });
        }
        PlayitCmd::Start => {
            conn.post(&["playitgg", "start_cli"], Body::Empty).await?;
            ctx.out.done("Started the playit.gg agent", json!({}));
        }
        PlayitCmd::Stop => {
            conn.post(&["playitgg", "stop_cli"], Body::Empty).await?;
            ctx.out.done("Stopped the playit.gg agent", json!({}));
        }
        PlayitCmd::Tunnels => {
            let tunnels = conn.get(&["playitgg", "get_tunnels"]).await?;
            ctx.out.show(&tunnels, || {
                let mut table = Table::new(["NAME", "ADDRESS", "LOCAL", "ACTIVE", "ID"]);
                for t in tunnels.as_array().into_iter().flatten() {
                    table.row([
                        cell(&t["name"]),
                        cell(&t["server_address"]),
                        format!("{}:{}", cell(&t["local_ip"]), cell(&t["local_port"])),
                        cell(&t["active"]),
                        cell(&t["tunnel_id"]),
                    ]);
                }
                table.print_or("No tunnels.");
            });
        }
    }
    Ok(())
}

pub async fn extension(ctx: &Ctx, cmd: ExtensionCmd) -> CliResult<()> {
    let conn = ctx.connect()?;
    match cmd {
        ExtensionCmd::GitStatus => {
            let installed = conn.get(&["extension", "gitstatus"]).await?;
            ctx.out.show(&json!({"git_installed": installed}), || {
                println!(
                    "git is {}installed on the core's machine",
                    if installed.as_bool().unwrap_or(false) {
                        ""
                    } else {
                        "not "
                    }
                )
            });
        }
        ExtensionCmd::Manifest { url } => {
            let manifest = conn
                .get_query(&["extension", "fetchmanifest"], &[("url", url)])
                .await?;
            ctx.out.show(&manifest, || {
                let m = &manifest["manifest"];
                print_fields(&[
                    ("Name", cell(&m["name"])),
                    ("Type", cell(&m["type"])),
                    ("Version", cell(&m["version"])),
                    ("Author", cell(&m["author"])),
                    ("Description", cell(&m["description"])),
                    ("Repository owner", cell(&manifest["username"])),
                ])
            });
        }
        ExtensionCmd::Install { url } => {
            conn.put(&["extension", "install"], Body::Json(json!({"url": url})))
                .await?;
            ctx.out.done(
                format!("Installed the extension from {url}"),
                json!({"url": url}),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(parse_age("90s").unwrap().as_secs(), 90);
        assert_eq!(parse_age("30m").unwrap().as_secs(), 1800);
        assert_eq!(parse_age("12h").unwrap().as_secs(), 43200);
        assert_eq!(parse_age("7d").unwrap().as_secs(), 604800);
        assert_eq!(parse_age("2w").unwrap().as_secs(), 1209600);
        assert_eq!(parse_age("5").unwrap().as_secs(), 300);
        assert!(parse_age("soon").is_err());
        assert!(parse_age("5y").is_err());
    }
}
