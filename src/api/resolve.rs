//! Finding instances and users by UUID or by name.

use serde_json::Value;

use super::error::{kind, CliError, CliResult};
use super::http::Connection;

/// An instance picked on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRef {
    pub uuid: String,
    pub name: String,
}

impl std::fmt::Display for InstanceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.name.is_empty() {
            write!(f, "{}", self.uuid)
        } else {
            write!(f, "{}", self.name)
        }
    }
}

/// Whether `s` is a full instance UUID as the core makes them.
pub fn is_instance_uuid(s: &str) -> bool {
    s.starts_with("INSTANCE_") || s.starts_with("DOCKER-")
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Picks the instance `query` names from `list` (`InstanceInfo` objects): its UUID (with
/// or without the `INSTANCE_` prefix, any case) or its name (any case). A name shared by
/// several instances is an error that lists them.
pub fn match_instance(list: &[Value], query: &str) -> CliResult<InstanceRef> {
    let to_ref = |v: &Value| InstanceRef {
        uuid: str_field(v, "uuid").to_string(),
        name: str_field(v, "name").to_string(),
    };
    let query_trimmed = query.trim();
    let by_uuid: Vec<&Value> = list
        .iter()
        .filter(|v| {
            let uuid = str_field(v, "uuid");
            uuid == query_trimmed
                || uuid.eq_ignore_ascii_case(query_trimmed)
                || uuid
                    .strip_prefix("INSTANCE_")
                    .is_some_and(|u| u.eq_ignore_ascii_case(query_trimmed))
        })
        .collect();
    if let [one] = by_uuid.as_slice() {
        return Ok(to_ref(one));
    }
    let by_name: Vec<&Value> = list
        .iter()
        .filter(|v| str_field(v, "name").to_lowercase() == query_trimmed.to_lowercase())
        .collect();
    match by_name.as_slice() {
        [one] => Ok(to_ref(one)),
        [] => {
            let names: Vec<&str> = list.iter().map(|v| str_field(v, "name")).collect();
            let known = if names.is_empty() {
                "You cannot see any instances.".to_string()
            } else {
                format!("Instances: {}", names.join(", "))
            };
            Err(CliError::not_found(format!(
                "No instance named `{query_trimmed}`. {known}"
            )))
        }
        many => {
            let matches: Vec<String> = many
                .iter()
                .map(|v| format!("{} ({})", str_field(v, "name"), str_field(v, "uuid")))
                .collect();
            Err(CliError::new(
                kind::AMBIGUOUS,
                format!(
                    "`{query_trimmed}` matches several instances; use a UUID: {}",
                    matches.join(", ")
                ),
            ))
        }
    }
}

/// Finds the instance `query` names on the core.
pub async fn instance(conn: &Connection, query: &str) -> CliResult<InstanceRef> {
    if is_instance_uuid(query) {
        return Ok(InstanceRef {
            uuid: query.to_string(),
            name: String::new(),
        });
    }
    let list = conn.get(&["instance", "list"]).await?;
    let list = list
        .as_array()
        .ok_or_else(|| CliError::bad_response("The instance list is not a list"))?;
    match_instance(list, query)
}

/// A user picked on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRef {
    pub uid: String,
    pub username: String,
}

/// Picks the user `query` names from `list` (`PublicUser` objects): its uid, or its name
/// (exactly, else ignoring case if that is unique).
pub fn match_user(list: &[Value], query: &str) -> CliResult<UserRef> {
    let to_ref = |v: &Value| UserRef {
        uid: str_field(v, "uid").to_string(),
        username: str_field(v, "username").to_string(),
    };
    if let Some(user) = list
        .iter()
        .find(|v| str_field(v, "uid") == query || str_field(v, "username") == query)
    {
        return Ok(to_ref(user));
    }
    let matches: Vec<&Value> = list
        .iter()
        .filter(|v| str_field(v, "username").eq_ignore_ascii_case(query))
        .collect();
    match matches.as_slice() {
        [one] => Ok(to_ref(one)),
        [] => Err(CliError::not_found(format!("No user named `{query}`"))),
        _ => Err(CliError::new(
            kind::AMBIGUOUS,
            format!("`{query}` matches several users; use the exact name or uid"),
        )),
    }
}

/// Finds a user on the core. Listing users needs the manage-users permission, so the
/// logged-in user is also matched against `/user/info`.
pub async fn user(conn: &Connection, query: &str) -> CliResult<UserRef> {
    let me = conn.get(&["user", "info"]).await?;
    if str_field(&me, "uid") == query || str_field(&me, "username") == query {
        return Ok(UserRef {
            uid: str_field(&me, "uid").to_string(),
            username: str_field(&me, "username").to_string(),
        });
    }
    let list = conn.get(&["user", "list"]).await?;
    let list = list
        .as_array()
        .ok_or_else(|| CliError::bad_response("The user list is not a list"))?;
    match_user(list, query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn instances() -> Vec<Value> {
        vec![
            json!({"uuid": "INSTANCE_aaaa-1111", "name": "Survival"}),
            json!({"uuid": "INSTANCE_bbbb-2222", "name": "creative"}),
            json!({"uuid": "INSTANCE_cccc-3333", "name": "Twin"}),
            json!({"uuid": "INSTANCE_dddd-4444", "name": "twin"}),
            json!({"uuid": "DOCKER-mc", "name": "docker mc"}),
        ]
    }

    #[test]
    fn by_name_ignoring_case() {
        let r = match_instance(&instances(), "survival").unwrap();
        assert_eq!(r.uuid, "INSTANCE_aaaa-1111");
        assert_eq!(r.name, "Survival");
        assert_eq!(
            match_instance(&instances(), "CREATIVE").unwrap().uuid,
            "INSTANCE_bbbb-2222"
        );
        assert_eq!(
            match_instance(&instances(), "docker mc").unwrap().uuid,
            "DOCKER-mc"
        );
    }

    #[test]
    fn by_uuid_with_or_without_prefix() {
        assert_eq!(
            match_instance(&instances(), "INSTANCE_bbbb-2222")
                .unwrap()
                .name,
            "creative"
        );
        assert_eq!(
            match_instance(&instances(), "BBBB-2222").unwrap().name,
            "creative"
        );
    }

    #[test]
    fn ambiguous_names_list_the_matches() {
        let e = match_instance(&instances(), "twin").unwrap_err();
        assert_eq!(e.kind, kind::AMBIGUOUS);
        assert!(e.message.contains("INSTANCE_cccc-3333"), "{}", e.message);
        assert!(e.message.contains("INSTANCE_dddd-4444"), "{}", e.message);
    }

    #[test]
    fn unknown_names_list_the_instances() {
        let e = match_instance(&instances(), "nope").unwrap_err();
        assert_eq!(e.kind, kind::NOT_FOUND);
        assert!(e.message.contains("Survival, creative"), "{}", e.message);
        let e = match_instance(&[], "nope").unwrap_err();
        assert!(e.message.contains("cannot see any"), "{}", e.message);
    }

    #[test]
    fn uuids_are_recognized() {
        assert!(is_instance_uuid("INSTANCE_1234"));
        assert!(is_instance_uuid("DOCKER-abc"));
        assert!(!is_instance_uuid("survival"));
    }

    #[test]
    fn users() {
        let list = vec![
            json!({"uid": "USER_1", "username": "Alice"}),
            json!({"uid": "USER_2", "username": "bob"}),
            json!({"uid": "USER_3", "username": "BOB"}),
        ];
        assert_eq!(match_user(&list, "alice").unwrap().uid, "USER_1");
        assert_eq!(match_user(&list, "USER_2").unwrap().username, "bob");
        assert_eq!(match_user(&list, "BOB").unwrap().uid, "USER_3");
        assert_eq!(match_user(&list, "Bob").unwrap_err().kind, kind::AMBIGUOUS);
        assert_eq!(
            match_user(&list, "carol").unwrap_err().kind,
            kind::NOT_FOUND
        );
    }
}
