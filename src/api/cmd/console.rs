//! `console`, `players` and `monitor`: an instance's output, players and resource usage.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::Instant;

use crate::api::cli::{ConsoleArgs, ConsoleCmd, ConsoleShowArgs, MonitorArgs, PlayersArgs};
use crate::api::error::{kind, CliError, CliResult};
use crate::api::http::{Body, Connection};
use crate::api::output::{cell, format_bytes, format_duration, Table};
use crate::api::resolve::{self, InstanceRef};
use crate::api::ws::{self, Backoff, Next};
use crate::api::Ctx;

fn snowflake(event: &Value) -> i64 {
    event["snowflake"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default()
}

/// The text of a console line, or `None` for events that are not console lines.
pub fn console_line(event: &Value) -> Option<String> {
    let inner = &event["event_inner"]["instance_event_inner"];
    let message = inner["message"].as_str().unwrap_or_default();
    match inner["type"].as_str()? {
        "InstanceOutput" => Some(message.to_string()),
        "InstanceInput" => Some(format!("> {message}")),
        "SystemMessage" => Some(format!("[Lodestone] {message}")),
        _ => None,
    }
}

pub async fn console(ctx: &Ctx, args: ConsoleArgs) -> CliResult<()> {
    match args.action {
        Some(ConsoleCmd::Send { instance, command }) => {
            let conn = ctx.connect()?;
            let inst = resolve::instance(&conn, &instance).await?;
            let command = command.join(" ");
            conn.post(
                &["instance", &inst.uuid, "console"],
                Body::Json(json!(command)),
            )
            .await?;
            ctx.out.done(
                format!("Sent to {inst}: {command}"),
                json!({"uuid": inst.uuid, "command": command}),
            );
            Ok(())
        }
        Some(ConsoleCmd::Show(show)) => show_console(ctx, show).await,
        None => show_console(ctx, args.show).await,
    }
}

struct Printer<'a> {
    ctx: &'a Ctx,
    last: i64,
}

impl Printer<'_> {
    /// Prints a console event once. Returns its text.
    fn print(&mut self, event: &Value) -> Option<String> {
        let id = snowflake(event);
        if id != 0 && id <= self.last {
            return None;
        }
        let line = console_line(event)?;
        self.last = self.last.max(id);
        if self.ctx.out.json {
            self.ctx.out.json_line(event);
        } else {
            println!("{line}");
        }
        Some(line)
    }
}

async fn buffer(conn: &Connection, inst: &InstanceRef) -> CliResult<Vec<Value>> {
    let events = conn
        .get(&["instance", &inst.uuid, "console", "buffer"])
        .await?;
    let mut events: Vec<Value> = events
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| console_line(e).is_some())
        .collect();
    events.sort_by_key(snowflake);
    Ok(events)
}

fn tail(events: Vec<Value>, lines: Option<usize>) -> Vec<Value> {
    match lines {
        Some(n) if events.len() > n => events[events.len() - n..].to_vec(),
        _ => events,
    }
}

async fn show_console(ctx: &Ctx, args: ConsoleShowArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    let query = args
        .instance
        .clone()
        .ok_or_else(|| CliError::usage("Which instance?"))?;
    let inst = resolve::instance(&conn, &query).await?;
    if !args.follow {
        let events = tail(buffer(&conn, &inst).await?, args.lines);
        ctx.out.show(&json!(events), || {
            for event in &events {
                if let Some(line) = console_line(event) {
                    println!("{line}");
                }
            }
        });
        return Ok(());
    }
    let deadline = args
        .timeout
        .map(|t| Instant::now() + Duration::from_secs(t));
    let token = ws::bearer_query(&conn)?;
    let mut printer = Printer { ctx, last: 0 };
    let mut backoff = Backoff::new();
    let mut first = true;
    loop {
        // subscribe before reading the buffer, so no line falls in between
        let mut socket = match ws::connect(
            &conn,
            &["instance", &inst.uuid, "console", "stream"],
            std::slice::from_ref(&token),
        )
        .await
        {
            Ok(socket) => socket,
            Err(e) if !first && ws::is_retryable(&e) => {
                ctx.out.note(format!("Reconnecting: {}", e.message));
                sleep_until_or_timeout(backoff.next_delay(), deadline).await?;
                continue;
            }
            Err(e) => return Err(e),
        };
        backoff.reset();
        let recent = buffer(&conn, &inst).await?;
        let recent = if first {
            tail(recent, args.lines)
        } else {
            recent
        };
        for event in &recent {
            // old lines never end the wait: they may be from an earlier run
            printer.print(event);
        }
        first = false;
        loop {
            let next = match deadline {
                Some(d) => match tokio::time::timeout_at(d, ws::next_json(&mut socket)).await {
                    Ok(next) => next,
                    Err(_) => return Err(until_timeout(&args)),
                },
                None => ws::next_json(&mut socket).await,
            };
            match next {
                Next::Json(event) => {
                    if let Some(line) = printer.print(&event) {
                        if args
                            .until
                            .as_ref()
                            .is_some_and(|u| line.contains(u.as_str()))
                        {
                            return Ok(());
                        }
                    }
                }
                Next::Closed(reason) => {
                    ctx.out.note(format!(
                        "Console stream closed{}; reconnecting",
                        reason.map(|r| format!(" ({r})")).unwrap_or_default()
                    ));
                    sleep_until_or_timeout(backoff.next_delay(), deadline).await?;
                    break;
                }
            }
        }
    }
}

fn until_timeout(args: &ConsoleShowArgs) -> CliError {
    match &args.until {
        Some(text) => CliError::new(
            kind::TIMEOUT,
            format!("No console line contained `{text}` in time"),
        ),
        None => CliError::new(kind::TIMEOUT, "Stopped following after the timeout"),
    }
}

async fn sleep_until_or_timeout(delay: Duration, deadline: Option<Instant>) -> CliResult<()> {
    if deadline.is_some_and(|d| Instant::now() + delay > d) {
        return Err(CliError::new(kind::TIMEOUT, "Timed out while reconnecting"));
    }
    tokio::time::sleep(delay).await;
    Ok(())
}

pub async fn players(ctx: &Ctx, args: PlayersArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    let inst = resolve::instance(&conn, &args.instance).await?;
    if let Some(max) = args.set_max {
        conn.put(
            &["instance", &inst.uuid, "players", "max"],
            Body::Json(json!(max)),
        )
        .await?;
        ctx.out.done(
            format!("Set the maximum players of {inst} to {max}"),
            json!({"uuid": inst.uuid, "max_players": max}),
        );
        return Ok(());
    }
    let uuid = inst.uuid.as_str();
    let (list_route, count_route, max_route) = (
        ["instance", uuid, "players"],
        ["instance", uuid, "players", "count"],
        ["instance", uuid, "players", "max"],
    );
    let (players, count, max) = tokio::join!(
        conn.get(&list_route),
        conn.get(&count_route),
        conn.get(&max_route),
    );
    let players = players?;
    // the counts are only known while the server runs
    let count = count.unwrap_or(Value::Null);
    let max = max.unwrap_or(Value::Null);
    let doc = json!({"uuid": inst.uuid, "players": players, "count": count, "max": max});
    ctx.out.show(&doc, || {
        let list = players.as_array().cloned().unwrap_or_default();
        let online = count.as_u64().unwrap_or(list.len() as u64);
        match max.as_u64() {
            Some(max) => println!("{online}/{max} players online on {inst}"),
            None => println!("{online} player(s) online on {inst}"),
        }
        let mut table = Table::new(["NAME", "UUID"]);
        for p in &list {
            table.row([cell(&p["name"]), cell(&p["uuid"])]);
        }
        if !table.is_empty() {
            table.print();
        }
    });
    Ok(())
}

fn print_report(report: &Value) {
    let cpu = report["cpu_usage"]
        .as_f64()
        .map(|c| format!("{c:.1}%"))
        .unwrap_or_else(|| "-".into());
    let memory = report["memory_usage"]
        .as_u64()
        .map(format_bytes)
        .unwrap_or_else(|| "-".into());
    let uptime = report["start_time"]
        .as_i64()
        .map(|start| format_duration((chrono::Utc::now().timestamp() - start).max(0) as u64))
        .unwrap_or_else(|| "-".into());
    println!("CPU {cpu:>7}   Memory {memory:>10}   Uptime {uptime}");
}

pub async fn monitor(ctx: &Ctx, args: MonitorArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    let inst = resolve::instance(&conn, &args.instance).await?;
    let token = ws::bearer_query(&conn)?;
    let mut backoff = Backoff::new();
    let mut first = true;
    loop {
        let mut socket = match ws::connect(
            &conn,
            &["monitor", &inst.uuid],
            std::slice::from_ref(&token),
        )
        .await
        {
            Ok(socket) => socket,
            Err(e) if args.follow && !first && ws::is_retryable(&e) => {
                ctx.out.note(format!("Reconnecting: {}", e.message));
                tokio::time::sleep(backoff.next_delay()).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        backoff.reset();
        if !args.follow {
            // the core sends its recent history, then a fresh report at once: keep the last
            // report of that first burst
            let mut latest = None;
            loop {
                let wait = if latest.is_some() { 400 } else { 5000 };
                match tokio::time::timeout(Duration::from_millis(wait), ws::next_json(&mut socket))
                    .await
                {
                    Ok(Next::Json(report)) => latest = Some(report),
                    Ok(Next::Closed(_)) | Err(_) => break,
                }
            }
            let report = latest
                .ok_or_else(|| CliError::new(kind::NETWORK, "The core sent no monitor report"))?;
            ctx.out.show(&report, || print_report(&report));
            return Ok(());
        }
        first = false;
        loop {
            match ws::next_json(&mut socket).await {
                Next::Json(report) => {
                    if ctx.out.json {
                        ctx.out.json_line(&report);
                    } else {
                        print_report(&report);
                    }
                }
                Next::Closed(_) => {
                    ctx.out.note("Monitor stream closed; reconnecting");
                    tokio::time::sleep(backoff.next_delay()).await;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, message: &str, snowflake: &str) -> Value {
        json!({"event_inner": {"type": "InstanceEvent", "instance_uuid": "I", "instance_name": "x",
                "instance_event_inner": {"type": kind, "message": message}},
               "snowflake": snowflake, "details": "", "level": "Info", "caused_by": {"type": "System"}})
    }

    #[test]
    fn console_lines() {
        assert_eq!(
            console_line(&event("InstanceOutput", "Done (1.0s)!", "1")).unwrap(),
            "Done (1.0s)!"
        );
        assert_eq!(
            console_line(&event("InstanceInput", "say hi", "1")).unwrap(),
            "> say hi"
        );
        assert_eq!(
            console_line(&event("SystemMessage", "Starting", "1")).unwrap(),
            "[Lodestone] Starting"
        );
        assert!(console_line(&event("StateTransition", "", "1")).is_none());
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let events: Vec<Value> = (1..=5)
            .map(|i| event("InstanceOutput", &i.to_string(), &i.to_string()))
            .collect();
        let last = tail(events.clone(), Some(2));
        assert_eq!(last.len(), 2);
        assert_eq!(last[0]["snowflake"], "4");
        assert_eq!(tail(events, None).len(), 5);
    }
}
