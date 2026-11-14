//! `trace`: the core's debug traces, for its owner. See `core/src/trace.rs` in the
//! Lodestone repository for what is traced. The traces stay on the core: this command
//! reads them over the owner's own connection.

use serde_json::Value;

use crate::api::cli::{TraceCmd, TraceWatchArgs};
use crate::api::error::CliResult;
use crate::api::ws::{self, Backoff, Next};
use crate::api::Ctx;

pub async fn run(ctx: &Ctx, cmd: TraceCmd) -> CliResult<()> {
    match cmd {
        TraceCmd::Topics => topics(ctx).await,
        TraceCmd::Snapshot => snapshot(ctx).await,
        TraceCmd::Watch(args) => watch(ctx, args).await,
    }
}

async fn topics(ctx: &Ctx) -> CliResult<()> {
    let conn = ctx.connect()?;
    let reply = conn.get(&["trace", "topics"]).await?;
    ctx.out.show(&reply, || {
        for topic in reply["topics"].as_array().into_iter().flatten() {
            println!(
                "{:<20} {:>6}  {}{}",
                topic["topic"].as_str().unwrap_or("?"),
                topic["events"],
                topic["about"].as_str().unwrap_or(""),
                if topic["live_only"] == true {
                    " (live only)"
                } else {
                    ""
                },
            );
        }
    });
    Ok(())
}

async fn snapshot(ctx: &Ctx) -> CliResult<()> {
    let conn = ctx.connect()?;
    let reply = conn.get(&["trace", "snapshot"]).await?;
    ctx.out.show(&reply, || {
        println!(
            "Lodestone Core {}, up {} s, safe mode {}, code execution {}",
            reply["version"].as_str().unwrap_or("?"),
            reply["uptime_secs"],
            reply["safe_mode"],
            reply["code_execution"],
        );
        for instance in reply["instances"].as_array().into_iter().flatten() {
            println!(
                "  {:<24} {}",
                instance["name"].as_str().unwrap_or("?"),
                instance["state"].as_str().unwrap_or("?")
            );
        }
    });
    Ok(())
}

/// `14:02:07.120  log.warn  {...}`, the time in this machine's zone.
pub fn trace_line(event: &Value) -> String {
    let at = event["at_ms"].as_i64().unwrap_or(0);
    let time = chrono::DateTime::from_timestamp_millis(at)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S%.3f")
                .to_string()
        })
        .unwrap_or_default();
    let topic = event["topic"].as_str().unwrap_or("?");
    format!("{time}  {topic:<18} {}", event["data"])
}

async fn watch(ctx: &Ctx, args: TraceWatchArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    let token = ws::bearer_query(&conn)?;
    let mut backoff = Backoff::new();
    let mut seen = 0usize;
    // after a reconnect, only what came after the last event shown
    let mut since = args.since;
    let mut backlog = args.backlog;
    let mut first = true;
    loop {
        let mut query = vec![token.clone(), ("backlog", backlog.to_string())];
        query.push(("topics", args.topics.join(",")));
        if let Some(since) = since {
            query.push(("since", since.to_string()));
        }
        let mut socket = match ws::connect(&conn, &["trace", "stream"], &query).await {
            Ok(socket) => socket,
            Err(e) if !first && ws::is_retryable(&e) => {
                ctx.out.note(format!("Reconnecting: {}", e.message));
                tokio::time::sleep(backoff.next_delay()).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        first = false;
        backoff.reset();
        loop {
            match ws::next_json(&mut socket).await {
                Next::Json(message) => match message["op"].as_str() {
                    Some("event") => {
                        let event = &message["event"];
                        since = event["seq"].as_u64().or(since);
                        if ctx.out.json {
                            ctx.out.json_line(event)?;
                        } else {
                            ctx.out.line(trace_line(event))?;
                        }
                        seen += 1;
                        if args.count.is_some_and(|count| seen >= count) {
                            return Ok(());
                        }
                    }
                    Some("lagged") => ctx.out.note(format!(
                        "{} events were dropped: the core sent faster than this read",
                        message["dropped"]
                    )),
                    Some("error") => ctx.out.note(message["message"].as_str().unwrap_or("error")),
                    _ => {}
                },
                Next::Closed(reason) => {
                    ctx.out.note(format!(
                        "The trace stream closed{}; reconnecting",
                        reason.map(|r| format!(" ({r})")).unwrap_or_default()
                    ));
                    // everything after the last event shown, whatever the backlog
                    backlog = usize::MAX;
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
    use serde_json::json;

    #[test]
    fn trace_lines_have_time_topic_and_data() {
        let line = trace_line(
            &json!({ "seq": 1, "at_ms": 1_700_000_000_000i64, "topic": "log.warn", "data": { "message": "slow" } }),
        );
        assert!(line.contains("log.warn"), "{line}");
        assert!(line.ends_with(r#"{"message":"slow"}"#), "{line}");
    }
}
