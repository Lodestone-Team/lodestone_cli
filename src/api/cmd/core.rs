//! `core`: the core's info, global settings and machine.

use serde_json::{json, Value};

use crate::api::cli::{CoreCmd, CoreSettingKey, CoreSettingsCmd};
use crate::api::error::CliResult;
use crate::api::http::Body;
use crate::api::output::{cell, format_bytes, format_duration, format_unix_seconds, print_fields};
use crate::api::Ctx;

use super::auth;
use super::values::parse_bool;

fn num(value: &Value) -> u64 {
    value.as_u64().unwrap_or_default()
}

pub async fn run(ctx: &Ctx, cmd: CoreCmd) -> CliResult<()> {
    match cmd {
        CoreCmd::Info => {
            let conn = ctx.session(false)?.conn;
            let info = conn.get(&["info"]).await?;
            ctx.out.show(&info, || {
                print_fields(&[
                    ("Name", cell(&info["core_name"])),
                    ("Version", cell(&info["version"])),
                    ("Set up", cell(&info["is_setup"])),
                    ("Host", cell(&info["host_name"])),
                    (
                        "OS",
                        format!("{} {}", cell(&info["os"]), cell(&info["arch"])),
                    ),
                    (
                        "CPU",
                        format!(
                            "{} ({} cores)",
                            cell(&info["cpu"]),
                            cell(&info["cpu_count"])
                        ),
                    ),
                    ("RAM", format_bytes(num(&info["total_ram"]))),
                    ("Disk", format_bytes(num(&info["total_disk"]))),
                    (
                        "Up since",
                        format_unix_seconds(info["up_since"].as_i64().unwrap_or_default()),
                    ),
                    ("TLS", cell(&info["tls"]["mode"])),
                    ("Fingerprint", cell(&info["tls"]["fingerprint_sha256"])),
                    ("UUID", cell(&info["uuid"])),
                ]);
            });
            Ok(())
        }
        CoreCmd::Settings(CoreSettingsCmd::Get) => {
            let conn = ctx.connect()?;
            let settings = conn.get(&["global_settings"]).await?;
            ctx.out.show(&settings, || {
                print_fields(&[
                    ("name", cell(&settings["core_name"])),
                    ("safe-mode", cell(&settings["safe_mode"])),
                    ("code-execution", cell(&settings["code_execution_enabled"])),
                    (
                        "code-execution-locked",
                        cell(&settings["code_execution_locked"]),
                    ),
                    ("domain", cell(&settings["domain"])),
                    ("allow-lan-http", cell(&settings["allow_lan_http"])),
                    ("playit", cell(&settings["playit_enabled"])),
                ]);
            });
            Ok(())
        }
        CoreCmd::Settings(CoreSettingsCmd::Set { key, value }) => {
            let conn = ctx.connect()?;
            let (route, body) = match key {
                CoreSettingKey::Name => ("name", json!(value)),
                CoreSettingKey::Domain => ("domain", json!(value)),
                CoreSettingKey::SafeMode => ("safe_mode", json!(parse_bool(&value)?)),
                CoreSettingKey::CodeExecution => ("code_execution", json!(parse_bool(&value)?)),
                CoreSettingKey::AllowLanHttp => ("allow_lan_http", json!(parse_bool(&value)?)),
                CoreSettingKey::Playit => ("playit_enabled", json!(parse_bool(&value)?)),
            };
            conn.put(&["global_settings", route], Body::Json(body.clone()))
                .await?;
            ctx.out.done(
                format!("Set {route} to {}", cell(&body)),
                json!({"setting": route, "value": body}),
            );
            Ok(())
        }
        CoreCmd::System => {
            let conn = ctx.connect()?;
            let (info, ram, disk, cpu) = tokio::try_join!(
                conn.get(&["info"]),
                conn.get(&["system", "ram"]),
                conn.get(&["system", "disk"]),
                conn.get(&["system", "cpu"]),
            )?;
            let up_since = info["up_since"].as_i64().unwrap_or_default();
            let uptime = (chrono::Utc::now().timestamp() - up_since).max(0) as u64;
            let doc = json!({
                "os": info["os"],
                "arch": info["arch"],
                "host_name": info["host_name"],
                "cpu": {"model": info["cpu"], "count": info["cpu_count"], "speed_mhz": cpu["cpu_speed"], "load_percent": cpu["cpu_load"]},
                "ram": ram,
                "disk": disk,
                "up_since": up_since,
                "uptime_seconds": uptime,
            });
            ctx.out.show(&doc, || {
                let used = |v: &Value| num(&v["total"]).saturating_sub(num(&v["free"]));
                print_fields(&[
                    (
                        "OS",
                        format!("{} {}", cell(&info["os"]), cell(&info["arch"])),
                    ),
                    ("Host", cell(&info["host_name"])),
                    (
                        "CPU",
                        format!(
                            "{} x{}, {} MHz, {:.1}% load",
                            cell(&info["cpu"]),
                            cell(&info["cpu_count"]),
                            cell(&cpu["cpu_speed"]),
                            cpu["cpu_load"].as_f64().unwrap_or_default()
                        ),
                    ),
                    (
                        "RAM",
                        format!(
                            "{} used of {}",
                            format_bytes(used(&ram)),
                            format_bytes(num(&ram["total"]))
                        ),
                    ),
                    (
                        "Disk",
                        format!(
                            "{} used of {}",
                            format_bytes(used(&disk)),
                            format_bytes(num(&disk["total"]))
                        ),
                    ),
                    ("Uptime", format_duration(uptime)),
                ]);
            });
            Ok(())
        }
        CoreCmd::Setup(args) => auth::setup(ctx, args).await,
        CoreCmd::Games => {
            let conn = ctx.connect()?;
            let games = conn.get(&["games"]).await?;
            ctx.out.show(&games, || {
                for game in games.as_array().into_iter().flatten() {
                    println!("{}", cell(game));
                }
            });
            Ok(())
        }
        CoreCmd::CheckPort { port } => {
            let conn = ctx.connect()?;
            let status = conn.get(&["check", "port", &port.to_string()]).await?;
            ctx.out.show(&status, || {
                let in_use = status["is_in_use"].as_bool().unwrap_or(true);
                let allocated = status["is_allocated"].as_bool().unwrap_or(true);
                match (in_use, allocated) {
                    (false, false) => println!("Port {port} is free"),
                    (true, _) => println!("Port {port} is in use"),
                    (false, true) => println!("Port {port} is assigned to a server"),
                }
            });
            Ok(())
        }
        CoreCmd::CheckName { name } => {
            let conn = ctx.connect()?;
            let used = conn.get(&["check", "name", &name]).await?;
            let doc = json!({"name": name, "in_use": used});
            ctx.out.show(&doc, || {
                if used.as_bool().unwrap_or(false) {
                    println!("The name `{name}` is used by a server");
                } else {
                    println!("The name `{name}` is free");
                }
            });
            Ok(())
        }
        CoreCmd::OpenPort { port } => {
            let conn = ctx.connect()?;
            conn.put(&["gateway", "open_port", &port.to_string()], Body::Empty)
                .await?;
            ctx.out.done(
                format!("Opened port {port} on the router"),
                json!({"port": port}),
            );
            Ok(())
        }
    }
}
