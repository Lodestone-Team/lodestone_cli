//! `api-keys`: list, create and revoke API keys. The core only allows these with a
//! password login (a session), never with an API key.

use serde_json::{json, Map, Value};

use crate::api::cli::{ApiKeysCmd, CreateKeyArgs};
use crate::api::error::{CliError, CliResult};
use crate::api::http::{Body, Connection};
use crate::api::keys;
use crate::api::output::{cell, format_unix_seconds, Table};
use crate::api::{prompt, resolve, Ctx};

use super::users::{
    apply_permission, instance_uuids, is_instance_permission, permission_field, PermissionChange,
    GLOBAL_PERMISSIONS, INSTANCE_PERMISSIONS,
};
use super::values::parse_bool;

/// A `UserPermission` that allows nothing.
pub fn no_permissions() -> Value {
    let mut permissions = Map::new();
    for field in INSTANCE_PERMISSIONS {
        permissions.insert(field.to_string(), json!([]));
    }
    for field in GLOBAL_PERMISSIONS {
        permissions.insert(field.to_string(), json!(false));
    }
    Value::Object(permissions)
}

/// Splits an `--allow PERMISSION=VALUE` into the permission field and its value.
pub fn parse_allow(input: &str) -> CliResult<(&'static str, String)> {
    let (permission, value) = input.split_once('=').ok_or_else(|| {
        CliError::usage(format!(
            "`--allow {input}`: use PERMISSION=VALUE, such as view_instance=survival or create_instance=true"
        ))
    })?;
    let field = permission_field(permission)?;
    let value = value.trim().to_string();
    if is_instance_permission(field) && value.is_empty() {
        return Err(CliError::usage(format!(
            "`--allow {input}`: name the instances, such as {permission}=survival,creative"
        )));
    }
    Ok((field, value))
}

/// The custom scope for `--allow` options: only the permissions listed.
async fn custom_scope(conn: &Connection, allows: &[String]) -> CliResult<Value> {
    let mut permissions = no_permissions();
    for allow in allows {
        let (field, value) = parse_allow(allow)?;
        let change = if is_instance_permission(field) {
            PermissionChange::Add(instance_uuids(conn, &[value]).await?)
        } else {
            PermissionChange::Flag(parse_bool(&value)?)
        };
        apply_permission(&mut permissions, field, change)?;
    }
    Ok(json!({"type": "custom", "permissions": permissions}))
}

/// "Full access", "Launcher: view, start and stop instances", or "Custom: 2 instances,
/// 3 permissions". A scope type this CLI doesn't know is named as such, never shown as
/// full access.
pub fn scope_summary(scope: &Value) -> String {
    match scope["type"].as_str() {
        Some("full") => return "Full access".to_string(),
        Some("launcher") => return launcher_summary(scope),
        Some("custom") => {}
        Some(other) => return format!("Other ({other})"),
        None => return "Unknown".to_string(),
    }
    let permissions = &scope["permissions"];
    let mut instances: Vec<&str> = Vec::new();
    let mut granted = 0;
    for field in INSTANCE_PERMISSIONS {
        let list: Vec<&str> = permissions[field]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !list.is_empty() {
            granted += 1;
        }
        for uuid in list {
            if !instances.contains(&uuid) {
                instances.push(uuid);
            }
        }
    }
    granted += GLOBAL_PERMISSIONS
        .iter()
        .filter(|f| permissions[**f] == true)
        .count();
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    format!(
        "Custom: {}, {}",
        plural(instances.len(), "instance"),
        plural(granted, "permission")
    )
}

/// A Lodestone Launcher key: view, start and stop, plus any console access it was
/// given (`console_read` / `console_write`, when the core offers them).
fn launcher_summary(scope: &Value) -> String {
    let mut can = vec!["view", "start and stop instances"];
    if scope["console_write"] == true {
        can.push("read and write the console");
    } else if scope["console_read"] == true {
        can.push("read the console");
    }
    format!("Launcher: {}", can.join(", "))
}

fn time_or(value: &Value, none: &str) -> String {
    value
        .as_i64()
        .map(format_unix_seconds)
        .unwrap_or_else(|| none.to_string())
}

/// The route of a user's keys: your own, or another user's.
async fn keys_route(conn: &Connection, user: Option<&str>) -> CliResult<(Vec<String>, String)> {
    match user {
        None => Ok((vec!["user".into(), "api_keys".into()], "you".to_string())),
        Some(query) => {
            let user = resolve::user(conn, query).await?;
            Ok((
                vec!["user".into(), user.uid, "api_keys".into()],
                user.username,
            ))
        }
    }
}

fn refs(segments: &[String]) -> Vec<&str> {
    segments.iter().map(String::as_str).collect()
}

pub async fn run(ctx: &Ctx, cmd: ApiKeysCmd) -> CliResult<()> {
    let conn = ctx.connect()?;
    match cmd {
        ApiKeysCmd::List { user } => {
            let (route, _) = keys_route(&conn, user.as_deref()).await?;
            let list = conn.get(&refs(&route)).await?;
            ctx.out.show(&list, || {
                let mut table =
                    Table::new(["KEY", "NAME", "SCOPE", "CREATED", "EXPIRES", "LAST USED"]);
                for key in list.as_array().into_iter().flatten() {
                    table.row([
                        cell(&key["display"]),
                        cell(&key["name"]),
                        scope_summary(&key["scope"]),
                        time_or(&key["created_at"], "-"),
                        time_or(&key["expires_at"], "Never"),
                        time_or(&key["last_used_at"], "Never"),
                    ]);
                }
                table.print_or("No API keys.");
            });
            Ok(())
        }
        ApiKeysCmd::Create(args) => create(ctx, &conn, args).await,
        ApiKeysCmd::Revoke { user, id } => {
            let id = keys::key_id(&id).ok_or_else(|| {
                CliError::usage(format!(
                    "`{}` is not an API key id (12 lowercase letters and digits, or lsk_<id>_…)",
                    keys::redact(&id)
                ))
            })?;
            let (mut route, whose) = keys_route(&conn, user.as_deref()).await?;
            let list = conn.get(&refs(&route)).await?;
            let key = list
                .as_array()
                .into_iter()
                .flatten()
                .find(|k| k["id"] == id.as_str())
                .ok_or_else(|| {
                    CliError::not_found(format!("No API key lsk_{id}_… belongs to {whose}"))
                })?;
            let name = cell(&key["name"]);
            prompt::confirm(
                ctx.global.yes,
                &format!(
                    "Revoke API key `{name}` (lsk_{id}_…)? Anything that uses it stops working."
                ),
            )?;
            route.push(id.clone());
            conn.delete(&refs(&route)).await?;
            ctx.out.done(
                format!("Revoked API key `{name}` (lsk_{id}_…)"),
                json!({"id": id, "name": name}),
            );
            Ok(())
        }
    }
}

async fn create(ctx: &Ctx, conn: &Connection, args: CreateKeyArgs) -> CliResult<()> {
    let name = args.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(CliError::usage("--name must be 1 to 64 characters"));
    }
    let scope = if args.full {
        json!({"type": "full"})
    } else {
        custom_scope(conn, &args.allow).await?
    };
    let expires_at = args
        .expires
        .seconds()
        .map(|s| chrono::Utc::now().timestamp() + s);
    let created = conn
        .post(
            &["user", "api_keys"],
            Body::Json(json!({"name": name, "expires_at": expires_at, "scope": scope})),
        )
        .await?;
    let key = created["key"]
        .as_str()
        .ok_or_else(|| CliError::bad_response("The core did not return the new key"))?;
    let info = &created["info"];
    // the one place the key is printed: stdout, once
    ctx.out.show(&created, || {
        println!(
            "Created API key `{}` ({}), {}, expires {}:\n\n{key}\n",
            cell(&info["name"]),
            keys::display(key),
            scope_summary(&info["scope"]),
            time_or(&info["expires_at"], "never"),
        );
    });
    ctx.out.warn(
        "Copy this key now and store it safely (a password manager or a CI secret): it is shown only once. Use it as LODESTONE_TOKEN, or with `lodestone_cli login <url> --api-key`.",
    );
    if args.expires.seconds().is_none() {
        ctx.out
            .warn("This key never expires. Revoke it with `lodestone_cli api-keys revoke` when it is no longer needed.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::kind;

    #[test]
    fn allow_options() {
        assert_eq!(
            parse_allow("view_instance=survival,creative").unwrap(),
            ("can_view_instance", "survival,creative".to_string())
        );
        assert_eq!(
            parse_allow("can_create_instance=true").unwrap(),
            ("can_create_instance", "true".to_string())
        );
        for bad in ["view_instance", "view_instance=", "fly=true"] {
            assert_eq!(parse_allow(bad).unwrap_err().kind, kind::USAGE, "{bad}");
        }
    }

    #[test]
    fn empty_permissions_allow_nothing() {
        let p = no_permissions();
        assert_eq!(p["can_start_instance"], json!([]));
        assert_eq!(p["can_create_instance"], json!(false));
        assert_eq!(
            p.as_object().unwrap().len(),
            INSTANCE_PERMISSIONS.len() + GLOBAL_PERMISSIONS.len()
        );
    }

    #[test]
    fn scope_summaries() {
        assert_eq!(scope_summary(&json!({"type": "full"})), "Full access");
        // A launcher key is not full access, and an unknown scope is never shown as one.
        assert_eq!(
            scope_summary(&json!({"type": "launcher"})),
            "Launcher: view, start and stop instances"
        );
        assert_eq!(
            scope_summary(&json!({"type": "launcher", "console_read": true})),
            "Launcher: view, start and stop instances, read the console"
        );
        assert_eq!(
            scope_summary(
                &json!({"type": "launcher", "console_read": true, "console_write": true})
            ),
            "Launcher: view, start and stop instances, read and write the console"
        );
        assert_eq!(
            scope_summary(&json!({"type": "future_kind"})),
            "Other (future_kind)"
        );
        assert_eq!(scope_summary(&json!({})), "Unknown");
        let mut p = no_permissions();
        p["can_view_instance"] = json!(["A", "B"]);
        p["can_start_instance"] = json!(["A"]);
        p["can_create_instance"] = json!(true);
        assert_eq!(
            scope_summary(&json!({"type": "custom", "permissions": p})),
            "Custom: 2 instances, 3 permissions"
        );
        assert_eq!(
            scope_summary(&json!({"type": "custom", "permissions": no_permissions()})),
            "Custom: 0 instances, 0 permissions"
        );
    }
}
