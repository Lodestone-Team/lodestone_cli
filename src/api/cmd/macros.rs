//! `macros`: an instance's macros. The core only lets the owner use them, and only while
//! code execution is on.

use serde_json::{json, Value};

use crate::api::cli::{MacroConfigCmd, MacrosArgs, MacrosCmd};
use crate::api::error::{CliError, CliResult};
use crate::api::http::Body;
use crate::api::output::{cell, format_unix_seconds, Table};
use crate::api::{prompt, resolve, Ctx};

use super::values::{configurable_value, describe_type, display_value};

fn time_cell(value: &Value) -> String {
    value
        .as_i64()
        .map(format_unix_seconds)
        .unwrap_or_else(|| "-".into())
}

fn exit_cell(status: &Value) -> String {
    match status["type"].as_str().unwrap_or_default() {
        "Error" => format!("error: {}", cell(&status["error_msg"])),
        other => other.to_lowercase(),
    }
}

pub async fn run(ctx: &Ctx, args: MacrosArgs) -> CliResult<()> {
    let conn = ctx.connect()?;
    let inst = resolve::instance(&conn, &args.instance).await?;
    let uuid = inst.uuid.as_str();
    match args.action {
        MacrosCmd::List => {
            let list = conn.get(&["instance", uuid, "macro", "list"]).await?;
            ctx.out.show(&list, || {
                let mut table = Table::new(["NAME", "LAST RUN", "PATH"]);
                for m in list.as_array().into_iter().flatten() {
                    table.row([
                        cell(&m["name"]),
                        time_cell(&m["last_run"]),
                        cell(&m["path"]),
                    ]);
                }
                table.print_or("No macros.");
            });
        }
        MacrosCmd::Run { name, args } => {
            conn.put(
                &["instance", uuid, "macro", "run", &name],
                Body::Json(json!(args)),
            )
            .await?;
            ctx.out.done(
                format!("Started macro {name} on {inst}"),
                json!({"uuid": uuid, "macro": name, "args": args}),
            );
        }
        MacrosCmd::Kill { pid } => {
            prompt::confirm(ctx.global.yes, &format!("Stop macro {pid} on {inst}?"))?;
            conn.put(
                &["instance", uuid, "macro", "kill", &pid.to_string()],
                Body::Empty,
            )
            .await?;
            ctx.out.done(
                format!("Stopped macro {pid}"),
                json!({"uuid": uuid, "pid": pid}),
            );
        }
        MacrosCmd::Tasks => {
            let list = conn.get(&["instance", uuid, "task", "list"]).await?;
            ctx.out.show(&list, || {
                let mut table = Table::new(["PID", "NAME", "STARTED"]);
                for t in list.as_array().into_iter().flatten() {
                    table.row([
                        cell(&t["pid"]),
                        cell(&t["name"]),
                        time_cell(&t["creation_time"]),
                    ]);
                }
                table.print_or("No macros are running.");
            });
        }
        MacrosCmd::History => {
            let list = conn.get(&["instance", uuid, "history", "list"]).await?;
            ctx.out.show(&list, || {
                let mut table = Table::new(["PID", "NAME", "STARTED", "EXIT"]);
                for h in list.as_array().into_iter().flatten() {
                    table.row([
                        cell(&h["task"]["pid"]),
                        cell(&h["task"]["name"]),
                        time_cell(&h["task"]["creation_time"]),
                        exit_cell(&h["exit_status"]),
                    ]);
                }
                table.print_or("No macro has run yet.");
            });
        }
        MacrosCmd::Config(MacroConfigCmd::Get { name }) => {
            let config = conn
                .get(&["instance", uuid, "macro", "config", "get", &name])
                .await?;
            ctx.out.show(&config, || {
                if let Some(message) = config["message"].as_str() {
                    println!("{message}");
                }
                let mut table = Table::new(["SETTING", "VALUE", "TYPE", "NAME"]);
                for (id, s) in config["config"].as_object().into_iter().flatten() {
                    table.row([
                        id.clone(),
                        display_value(&s["value"]),
                        describe_type(&s["value_type"]),
                        cell(&s["name"]),
                    ]);
                }
                table.print_or("This macro has no settings.");
            });
        }
        MacrosCmd::Config(MacroConfigCmd::Set { name, key, value }) => {
            let response = conn
                .get(&["instance", uuid, "macro", "config", "get", &name])
                .await?;
            let mut config = response["config"].clone();
            let setting = config.get_mut(&key).ok_or_else(|| {
                CliError::not_found(format!("Macro {name} has no setting `{key}`"))
            })?;
            let new_value = configurable_value(&setting["value_type"], &value)?;
            setting["value"] = new_value.clone();
            conn.post(
                &["instance", uuid, "macro", "config", "store", &name],
                Body::Json(config),
            )
            .await?;
            ctx.out.done(
                format!("Set {key} of macro {name} to {}", display_value(&new_value)),
                json!({"uuid": uuid, "macro": name, "setting": key, "value": new_value}),
            );
        }
    }
    Ok(())
}
