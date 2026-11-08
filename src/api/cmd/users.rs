//! `users`: accounts and the permission editor.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::api::cli::{PermsCmd, UsersCmd};
use crate::api::config::Profiles;
use crate::api::error::{CliError, CliResult};
use crate::api::http::{Body, Connection};
use crate::api::output::{cell, print_fields, Table};
use crate::api::resolve::{self, UserRef};
use crate::api::{prompt, Ctx, ENV_PASSWORD};

/// Permissions that list the instances they apply to.
pub const INSTANCE_PERMISSIONS: [&str; 10] = [
    "can_view_instance",
    "can_start_instance",
    "can_stop_instance",
    "can_access_instance_console",
    "can_access_instance_setting",
    "can_read_instance_resource",
    "can_write_instance_resource",
    "can_access_instance_macro",
    "can_read_instance_file",
    "can_write_instance_file",
];

/// Permissions that are on or off for the whole core.
pub const GLOBAL_PERMISSIONS: [&str; 6] = [
    "can_create_instance",
    "can_delete_instance",
    "can_read_global_file",
    "can_write_global_file",
    "can_manage_permission",
    "can_install_extension",
];

/// The permission field a user typed: the field name, with or without `can_`, with `-` or
/// `_`, in any case.
pub fn permission_field(input: &str) -> CliResult<&'static str> {
    let normalized = input.trim().to_lowercase().replace('-', "_");
    let normalized = normalized.strip_prefix("can_").unwrap_or(&normalized);
    INSTANCE_PERMISSIONS
        .iter()
        .chain(GLOBAL_PERMISSIONS.iter())
        .find(|field| field.strip_prefix("can_") == Some(normalized))
        .copied()
        .ok_or_else(|| {
            CliError::usage(format!(
                "`{input}` is not a permission. Per instance: {}. For the core: {}",
                INSTANCE_PERMISSIONS.join(", "),
                GLOBAL_PERMISSIONS.join(", ")
            ))
        })
}

pub(super) fn is_instance_permission(field: &str) -> bool {
    INSTANCE_PERMISSIONS.contains(&field)
}

/// UUIDs for instance names or UUIDs, against the instance list.
pub(super) async fn instance_uuids(conn: &Connection, names: &[String]) -> CliResult<Vec<String>> {
    let names: Vec<&str> = names
        .iter()
        .flat_map(|n| n.split(','))
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let list = conn.get(&["instance", "list"]).await?;
    let list = list.as_array().cloned().unwrap_or_default();
    names
        .iter()
        .map(|name| {
            if resolve::is_instance_uuid(name) {
                Ok(name.to_string())
            } else {
                resolve::match_instance(&list, name).map(|i| i.uuid)
            }
        })
        .collect()
}

/// Changes one field of a `UserPermission` object: a per-instance list, or the flag of a
/// permission for the whole core.
pub fn apply_permission(
    permissions: &mut Value,
    field: &str,
    change: PermissionChange,
) -> CliResult<()> {
    match change {
        PermissionChange::Flag(flag) => {
            if is_instance_permission(field) {
                return Err(CliError::usage(format!(
                    "{field} is per instance: give instance names, or use `users perms grant`"
                )));
            }
            permissions[field] = json!(flag);
        }
        PermissionChange::Set(_) | PermissionChange::Add(_) | PermissionChange::Remove(_)
            if !is_instance_permission(field) =>
        {
            return Err(CliError::usage(format!(
                "{field} is for the whole core: set it to true or false"
            )));
        }
        PermissionChange::Set(uuids) => permissions[field] = json!(uuids),
        PermissionChange::Add(uuids) => {
            let mut current: Vec<String> = string_list(&permissions[field]);
            for uuid in uuids {
                if !current.contains(&uuid) {
                    current.push(uuid);
                }
            }
            permissions[field] = json!(current);
        }
        PermissionChange::Remove(uuids) => {
            let current: Vec<String> = string_list(&permissions[field])
                .into_iter()
                .filter(|u| !uuids.contains(u))
                .collect();
            permissions[field] = json!(current);
        }
    }
    Ok(())
}

pub enum PermissionChange {
    Flag(bool),
    Set(Vec<String>),
    Add(Vec<String>),
    Remove(Vec<String>),
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn new_password(password_stdin: bool, question: &str) -> CliResult<String> {
    if password_stdin {
        return prompt::read_secret_from_stdin();
    }
    let first = prompt::ask_secret(question)?;
    let second = prompt::ask_secret("Again")?;
    if first != second {
        return Err(CliError::usage("The passwords do not match"));
    }
    if first.is_empty() {
        return Err(CliError::usage("The password cannot be empty"));
    }
    Ok(first)
}

async fn user_ref(conn: &Connection, query: &str) -> CliResult<UserRef> {
    resolve::user(conn, query).await
}

async fn print_permissions(conn: &Connection, user: &Value) {
    let perms = &user["permissions"];
    let names: BTreeMap<String, String> = match conn.get(&["instance", "list"]).await {
        Ok(list) => list
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| (cell(&i["uuid"]), cell(&i["name"])))
            .collect(),
        Err(_) => BTreeMap::new(),
    };
    println!(
        "{} ({}){}{}",
        cell(&user["username"]),
        cell(&user["uid"]),
        if user["is_owner"] == true {
            ", owner: may do everything"
        } else {
            ""
        },
        if user["is_admin"] == true {
            ", admin"
        } else {
            ""
        }
    );
    let mut table = Table::new(["PERMISSION", "VALUE"]);
    for field in GLOBAL_PERMISSIONS {
        table.row([field.to_string(), cell(&perms[field])]);
    }
    for field in INSTANCE_PERMISSIONS {
        let instances: Vec<String> = string_list(&perms[field])
            .into_iter()
            .map(|uuid| names.get(&uuid).cloned().unwrap_or(uuid))
            .collect();
        table.row([
            field.to_string(),
            if instances.is_empty() {
                "-".to_string()
            } else {
                instances.join(", ")
            },
        ]);
    }
    table.print();
}

pub async fn run(ctx: &Ctx, cmd: UsersCmd) -> CliResult<()> {
    let session = ctx.session(true)?;
    let conn = &session.conn;
    match cmd {
        UsersCmd::List => {
            let list = conn.get(&["user", "list"]).await?;
            ctx.out.show(&list, || {
                let mut table = Table::new(["USERNAME", "OWNER", "ADMIN", "UID"]);
                for u in list.as_array().into_iter().flatten() {
                    table.row([
                        cell(&u["username"]),
                        cell(&u["is_owner"]),
                        cell(&u["is_admin"]),
                        cell(&u["uid"]),
                    ]);
                }
                table.print_or("No users.");
            });
        }
        UsersCmd::Me => {
            let me = conn.get(&["user", "info"]).await?;
            ctx.out.show(&me, || {
                print_fields(&[
                    ("User", cell(&me["username"])),
                    ("Uid", cell(&me["uid"])),
                    ("Owner", cell(&me["is_owner"])),
                    ("Admin", cell(&me["is_admin"])),
                ])
            });
        }
        UsersCmd::Info { user } => {
            let user = user_ref(conn, &user).await?;
            let info = conn.get(&["user", &user.uid]).await?;
            if ctx.out.json {
                ctx.out.json(&info);
            } else {
                print_permissions(conn, &info).await;
            }
        }
        UsersCmd::Create {
            username,
            password_stdin,
        } => {
            let password = new_password(password_stdin, &format!("Password for {username}"))?;
            let user = conn
                .post(
                    &["user"],
                    Body::Json(json!({"username": username, "password": password})),
                )
                .await?;
            ctx.out.show(&user, || {
                println!(
                    "Created user {} ({}). Grant permissions with `lodestone_cli users perms grant`.",
                    cell(&user["username"]),
                    cell(&user["uid"])
                )
            });
        }
        UsersCmd::Delete { user } => {
            let user = user_ref(conn, &user).await?;
            prompt::confirm(
                ctx.global.yes,
                &format!("Delete user {} ({})?", user.username, user.uid),
            )?;
            conn.delete(&["user", &user.uid]).await?;
            ctx.out.done(
                format!("Deleted user {}", user.username),
                json!({"uid": user.uid, "username": user.username}),
            );
        }
        UsersCmd::Rename { user, new_name } => {
            let user = user_ref(conn, &user).await?;
            conn.put(&["user", &user.uid, "rename"], Body::Json(json!(new_name)))
                .await?;
            ctx.out.done(
                format!("Renamed {} to {new_name}", user.username),
                json!({"uid": user.uid, "username": new_name}),
            );
        }
        UsersCmd::Passwd {
            user,
            password_stdin,
        } => {
            let me = conn.get(&["user", "info"]).await?;
            let target = match &user {
                Some(query) => user_ref(conn, query).await?,
                None => UserRef {
                    uid: cell(&me["uid"]),
                    username: cell(&me["username"]),
                },
            };
            let is_self = target.uid == cell(&me["uid"]);
            let old_password = if is_self {
                match std::env::var(ENV_PASSWORD) {
                    Ok(p) if !p.is_empty() => Some(p),
                    _ => Some(prompt::ask_secret("Current password")?),
                }
            } else {
                None
            };
            let password = new_password(
                password_stdin,
                &format!("New password for {}", target.username),
            )?;
            conn.put(
                &["user", &target.uid, "password"],
                Body::Json(json!({
                    "uid": target.uid,
                    "old_password": old_password,
                    "new_password": password,
                })),
            )
            .await?;
            let mut relogged = false;
            if is_self {
                // changing the password ends the user's sessions: renew the saved token
                if let (Some(name), Some(mut profile)) =
                    (session.profile_name.clone(), session.profile.clone())
                {
                    if let Ok(reply) = conn.login(&target.username, &password).await {
                        if let Some(token) = reply["token"].as_str() {
                            profile.token = token.to_string();
                            let mut profiles = Profiles::load()?;
                            profiles.profiles.insert(name, profile);
                            profiles.save()?;
                            relogged = true;
                        }
                    }
                }
            }
            let note = if is_self && !relogged {
                " Log in again with the new password."
            } else {
                ""
            };
            ctx.out.done(
                format!("Changed the password of {}.{note}", target.username),
                json!({"uid": target.uid, "token_renewed": relogged}),
            );
        }
        UsersCmd::Logout { user } => {
            let user = user_ref(conn, &user).await?;
            conn.post(&["user", "logout", &user.uid], Body::Empty)
                .await?;
            ctx.out.done(
                format!("Ended every session of {}", user.username),
                json!({"uid": user.uid}),
            );
        }
        UsersCmd::Perms(cmd) => perms(ctx, conn, cmd).await?,
    }
    Ok(())
}

async fn perms(ctx: &Ctx, conn: &Connection, cmd: PermsCmd) -> CliResult<()> {
    let (user, field, change) = match cmd {
        PermsCmd::Get { user } => {
            let user = user_ref(conn, &user).await?;
            let info = conn.get(&["user", &user.uid]).await?;
            if ctx.out.json {
                ctx.out.json(&info["permissions"]);
            } else {
                print_permissions(conn, &info).await;
            }
            return Ok(());
        }
        PermsCmd::Set {
            user,
            permission,
            value,
        } => {
            let field = permission_field(&permission)?;
            let change = if is_instance_permission(field) {
                PermissionChange::Set(instance_uuids(conn, &[value]).await?)
            } else {
                PermissionChange::Flag(super::values::parse_bool(&value)?)
            };
            (user, field, change)
        }
        PermsCmd::Grant {
            user,
            permission,
            instances,
        } => {
            let field = permission_field(&permission)?;
            (
                user,
                field,
                PermissionChange::Add(instance_uuids(conn, &instances).await?),
            )
        }
        PermsCmd::Revoke {
            user,
            permission,
            instances,
        } => {
            let field = permission_field(&permission)?;
            (
                user,
                field,
                PermissionChange::Remove(instance_uuids(conn, &instances).await?),
            )
        }
    };
    let user = user_ref(conn, &user).await?;
    let info = conn.get(&["user", &user.uid]).await?;
    let mut permissions = info["permissions"].clone();
    apply_permission(&mut permissions, field, change)?;
    conn.put(
        &["user", &user.uid, "update_perm"],
        Body::Json(permissions.clone()),
    )
    .await?;
    ctx.out.done(
        format!(
            "Updated {field} of {}: {}",
            user.username,
            cell(&permissions[field])
        ),
        json!({"uid": user.uid, "permission": field, "value": permissions[field]}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_names() {
        assert_eq!(
            permission_field("can_start_instance").unwrap(),
            "can_start_instance"
        );
        assert_eq!(
            permission_field("start_instance").unwrap(),
            "can_start_instance"
        );
        assert_eq!(
            permission_field("Read-Instance-File").unwrap(),
            "can_read_instance_file"
        );
        assert_eq!(
            permission_field("manage_permission").unwrap(),
            "can_manage_permission"
        );
        assert!(permission_field("fly").is_err());
    }

    #[test]
    fn applying_changes() {
        let mut p = json!({"can_start_instance": ["A"], "can_create_instance": false});
        apply_permission(
            &mut p,
            "can_start_instance",
            PermissionChange::Add(vec!["B".into(), "A".into()]),
        )
        .unwrap();
        assert_eq!(p["can_start_instance"], json!(["A", "B"]));
        apply_permission(
            &mut p,
            "can_start_instance",
            PermissionChange::Remove(vec!["A".into()]),
        )
        .unwrap();
        assert_eq!(p["can_start_instance"], json!(["B"]));
        apply_permission(&mut p, "can_start_instance", PermissionChange::Set(vec![])).unwrap();
        assert_eq!(p["can_start_instance"], json!([]));
        apply_permission(&mut p, "can_create_instance", PermissionChange::Flag(true)).unwrap();
        assert_eq!(p["can_create_instance"], json!(true));
        assert!(
            apply_permission(&mut p, "can_start_instance", PermissionChange::Flag(true)).is_err()
        );
        assert!(apply_permission(
            &mut p,
            "can_create_instance",
            PermissionChange::Add(vec!["A".into()])
        )
        .is_err());
    }
}
