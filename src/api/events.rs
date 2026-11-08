//! Events from the core: formatting them, and following background tasks (progression
//! events) such as instance creation until they end.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::Instant;

use super::error::{kind, CliError, CliResult};
use super::http::Connection;
use super::output::{format_unix_millis, Output};
use super::ws;

/// Snowflakes count milliseconds from this Unix time (`LODESTONE_EPOCH` in the dashboard).
pub const LODESTONE_EPOCH_MILLIS: i64 = 1_667_530_800_000;

/// The Unix time in milliseconds of a snowflake id.
pub fn snowflake_millis(snowflake: &str) -> Option<i64> {
    let id: i64 = snowflake.parse().ok()?;
    Some((id >> 22) + LODESTONE_EPOCH_MILLIS)
}

fn s<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The `EventQuery` the event routes take as their `filter` parameter. Empty lists mean
/// "no filter".
#[derive(Debug, Default, Clone)]
pub struct EventFilter {
    pub levels: Vec<String>,
    pub types: Vec<String>,
    pub instance_ids: Vec<String>,
    pub user_ids: Vec<String>,
    /// Unix milliseconds, inclusive.
    pub time_range: Option<(i64, i64)>,
}

impl EventFilter {
    /// The query as JSON. The live stream takes the bare token in `bearer_token`.
    pub fn to_query(&self, bearer_token: Option<&str>) -> Value {
        let list = |items: &Vec<String>| {
            if items.is_empty() {
                Value::Null
            } else {
                json!(items)
            }
        };
        json!({
            "event_levels": list(&self.levels),
            "event_types": list(&self.types),
            "instance_event_types": null,
            "user_event_types": null,
            "event_user_ids": list(&self.user_ids),
            "event_instance_ids": list(&self.instance_ids),
            "bearer_token": bearer_token,
            "time_range": self.time_range.map(|(start, end)| json!({"start": start, "end": end})),
        })
    }
}

/// Console lines (shown by `console`, not by `events`, as in the dashboard).
pub fn is_console_event(event: &Value) -> bool {
    let inner = &event["event_inner"];
    s(inner, "type") == "InstanceEvent"
        && matches!(
            s(&inner["instance_event_inner"], "type"),
            "InstanceOutput" | "InstanceInput" | "SystemMessage"
        )
}

fn player_names(players: &Value) -> String {
    players
        .as_array()
        .map(|list| {
            list.iter()
                .map(|p| s(p, "name").to_string())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// One line describing an event.
pub fn describe(event: &Value) -> String {
    let inner = &event["event_inner"];
    let details = s(event, "details");
    let text = match s(inner, "type") {
        "InstanceEvent" => {
            let name = s(inner, "instance_name");
            let e = &inner["instance_event_inner"];
            match s(e, "type") {
                "StateTransition" => format!("{name} is now {}", s(e, "to")),
                "InstanceWarning" => format!("{name}: warning: {}", s(e, "message")),
                "InstanceError" => format!("{name}: error: {}", s(e, "message")),
                "InstanceInput" => format!("{name}> {}", s(e, "message")),
                "InstanceOutput" | "SystemMessage" => format!("{name}: {}", s(e, "message")),
                "PlayerChange" => {
                    let joined = player_names(&e["players_joined"]);
                    let left = player_names(&e["players_left"]);
                    match (joined.is_empty(), left.is_empty()) {
                        (false, true) => format!("{joined} joined {name}"),
                        (true, false) => format!("{left} left {name}"),
                        _ => format!("Players on {name} changed"),
                    }
                }
                "PlayerMessage" => {
                    format!("{name}: <{}> {}", s(e, "player"), s(e, "player_message"))
                }
                _ => details.to_string(),
            }
        }
        "ProgressionEvent" => {
            let e = &inner["progression_event_inner"];
            match s(e, "type") {
                "ProgressionStart" => format!("Started: {}", s(e, "progression_name")),
                "ProgressionUpdate" => s(e, "progress_message").to_string(),
                "ProgressionEnd" => {
                    let message = e["message"].as_str().unwrap_or_default();
                    if e["success"].as_bool().unwrap_or(false) {
                        format!("Finished: {message}")
                    } else {
                        format!("Failed: {message}")
                    }
                }
                _ => details.to_string(),
            }
        }
        "MacroEvent" => {
            let pid = &inner["macro_pid"];
            match s(&inner["macro_event_inner"], "type") {
                "Started" => format!("Macro {pid} started"),
                "Detach" => format!("Macro {pid} detached"),
                "Stopped" => {
                    let status = &inner["macro_event_inner"]["exit_status"];
                    match s(status, "type") {
                        "Error" => format!("Macro {pid} failed: {}", s(status, "error_msg")),
                        other => format!("Macro {pid} stopped ({other})"),
                    }
                }
                _ => details.to_string(),
            }
        }
        _ => details.to_string(),
    };
    if text.is_empty() {
        details.to_string()
    } else {
        text
    }
}

/// `time  LEVEL  text` for an event.
pub fn format_event_line(event: &Value) -> String {
    let time = snowflake_millis(s(event, "snowflake"))
        .map(format_unix_millis)
        .unwrap_or_default();
    format!("{time}  {:<7}  {}", s(event, "level"), describe(event))
}

/* --------------------------------------------------------------- tasks */

/// A background task, from its progression events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub total: Option<f64>,
    pub progress: f64,
    pub message: String,
    /// The instance being created or deleted, if any.
    pub instance_uuid: Option<String>,
    pub started_millis: Option<i64>,
    /// `Some(success)` once the task ended.
    pub ended: Option<bool>,
    pub end_message: Option<String>,
    /// The `inner` of the end event (such as the new instance's info).
    pub end_value: Value,
}

impl Task {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "total": self.total,
            "progress": self.progress,
            "message": self.message,
            "instance_uuid": self.instance_uuid,
            "started": self.started_millis,
            "state": match self.ended {
                None => "running",
                Some(true) => "succeeded",
                Some(false) => "failed",
            },
            "end_message": self.end_message,
            "result": self.end_value,
        })
    }

    pub fn percent(&self) -> Option<f64> {
        let total = self.total.filter(|t| *t > 0.0)?;
        Some((self.progress / total * 100.0).clamp(0.0, 100.0))
    }
}

/// The tasks seen in a sequence of events, in the order they started.
#[derive(Debug, Default)]
pub struct Tasks {
    pub tasks: BTreeMap<String, Task>,
    order: Vec<String>,
}

impl Tasks {
    /// Applies an event. Returns the id of the task it belongs to, if it is a progression
    /// event.
    pub fn apply(&mut self, event: &Value) -> Option<String> {
        let inner = &event["event_inner"];
        if s(inner, "type") != "ProgressionEvent" {
            return None;
        }
        let id = s(inner, "event_id").to_string();
        if id.is_empty() {
            return None;
        }
        let e = &inner["progression_event_inner"];
        if !self.tasks.contains_key(&id) {
            self.order.push(id.clone());
        }
        let task = self.tasks.entry(id.clone()).or_insert_with(|| Task {
            id: id.clone(),
            ..Task::default()
        });
        match s(e, "type") {
            "ProgressionStart" => {
                task.name = s(e, "progression_name").to_string();
                task.total = e["total"].as_f64();
                task.started_millis = snowflake_millis(s(event, "snowflake"));
                if let Some(uuid) = e["inner"]["instance_uuid"].as_str() {
                    task.instance_uuid = Some(uuid.to_string());
                }
            }
            "ProgressionUpdate" => {
                task.progress += e["progress"].as_f64().unwrap_or(0.0);
                task.message = s(e, "progress_message").to_string();
            }
            "ProgressionEnd" => {
                task.ended = Some(e["success"].as_bool().unwrap_or(false));
                task.end_message = e["message"].as_str().map(str::to_string);
                task.end_value = e["inner"].clone();
                if task.instance_uuid.is_none() {
                    task.instance_uuid = e["inner"]["uuid"]
                        .as_str()
                        .or_else(|| e["inner"]["instance_uuid"].as_str())
                        .map(str::to_string);
                }
            }
            _ => {}
        }
        Some(id)
    }

    pub fn in_order(&self) -> impl Iterator<Item = &Task> {
        self.order.iter().filter_map(|id| self.tasks.get(id))
    }

    /// The task `target` names: a task id, or the instance a task creates or deletes
    /// (the newest such task).
    pub fn find(&self, target: &str) -> Option<&Task> {
        if let Some(task) = self.tasks.get(target) {
            return Some(task);
        }
        self.order
            .iter()
            .rev()
            .filter_map(|id| self.tasks.get(id))
            .find(|t| t.instance_uuid.as_deref() == Some(target))
    }
}

/// What a waiter is waiting for.
pub enum Target {
    /// A task id, or the UUID of an instance being created or deleted.
    Id(String),
    /// The first task whose name starts with this text and that was not in the buffer when
    /// waiting began. Only for older cores, whose zip and unzip routes return no task id.
    NewTaskWithPrefix(String),
}

/// Waits for a task to end, printing its progress to stderr. Subscribes to the event
/// stream before reading the event buffer, so no event falls in between, and reconnects if
/// the stream drops. With `action`, the stream is opened first, then the action runs (for
/// requests that start a task).
pub struct TaskWaiter<'a> {
    pub conn: &'a Connection,
    pub out: &'a Output,
    pub timeout: Option<Duration>,
}

impl TaskWaiter<'_> {
    fn filter(&self) -> EventFilter {
        EventFilter {
            types: vec!["ProgressionEvent".to_string()],
            ..EventFilter::default()
        }
    }

    async fn buffer(&self) -> CliResult<Vec<Value>> {
        let query = self.filter().to_query(None).to_string();
        let buffer = self
            .conn
            .get_query(&["events", "all", "buffer"], &[("filter", query)])
            .await?;
        Ok(buffer.as_array().cloned().unwrap_or_default())
    }

    async fn subscribe(&self) -> CliResult<ws::Socket> {
        let token = self
            .conn
            .token()
            .ok_or_else(|| CliError::new(kind::NOT_LOGGED_IN, "Not logged in"))?;
        let query = self.filter().to_query(Some(token)).to_string();
        ws::connect(
            self.conn,
            &["events", "all", "stream"],
            &[("filter", query)],
        )
        .await
    }

    /// Waits for `target`, running `action` once the stream is open. Returns the ended task;
    /// a failed task is an error.
    pub async fn wait<F, Fut, T>(&self, target: Target, action: F) -> CliResult<(T, Task)>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = CliResult<T>>,
    {
        self.wait_for(|_| target, action).await
    }

    /// Like [`wait`](Self::wait), but the target comes from the action's result (such as a
    /// task id the request returned).
    pub async fn wait_for<F, Fut, T>(
        &self,
        target_of: impl FnOnce(&T) -> Target,
        action: F,
    ) -> CliResult<(T, Task)>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = CliResult<T>>,
    {
        let mut target_of = Some(target_of);
        let mut target: Option<Target> = None;
        let deadline = self.timeout.map(|t| Instant::now() + t);
        let mut backoff = ws::Backoff::new();
        let mut tasks = Tasks::default();
        let mut known_before: Option<HashSet<String>> = None;
        let mut action = Some(action);
        let mut action_result: Option<T> = None;
        let mut last_message = String::new();
        loop {
            let socket = match self.subscribe().await {
                Ok(socket) => socket,
                Err(e) if ws::is_retryable(&e) && action_result.is_some() => {
                    self.out
                        .note(format!("Event stream unavailable ({e}), retrying"));
                    sleep_or_timeout(backoff.next_delay(), deadline).await?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            backoff.reset();
            let mut socket = socket;
            for event in self.buffer().await? {
                tasks.apply(&event);
            }
            if known_before.is_none() {
                known_before = Some(tasks.tasks.keys().cloned().collect());
            }
            if let Some(action) = action.take() {
                let result = action().await?;
                if let Some(target_of) = target_of.take() {
                    target = Some(target_of(&result));
                }
                action_result = Some(result);
            }
            let known = known_before.as_ref().expect("set above");
            let target = target.as_ref().expect("set with the action's result");
            let pick = |tasks: &Tasks| -> Option<Task> {
                match target {
                    Target::Id(id) => tasks.find(id).cloned(),
                    Target::NewTaskWithPrefix(prefix) => tasks
                        .in_order()
                        .find(|t| t.name.starts_with(prefix.as_str()) && !known.contains(&t.id))
                        .cloned(),
                }
            };
            if let Some(task) = pick(&tasks) {
                if task.ended.is_some() {
                    return finish(action_result, task);
                }
            } else if let Target::Id(id) = target {
                // the task may have ended long ago and left the buffer
                if crate::api::resolve::is_instance_uuid(id)
                    && self.conn.get(&["instance", id, "info"]).await.is_ok()
                {
                    let task = Task {
                        id: id.clone(),
                        name: "Server creation".to_string(),
                        instance_uuid: Some(id.clone()),
                        ended: Some(true),
                        end_message: Some("The server exists".to_string()),
                        ..Task::default()
                    };
                    return finish(action_result, task);
                }
            }
            loop {
                let next = match deadline {
                    Some(deadline) => {
                        match tokio::time::timeout_at(deadline, ws::next_json(&mut socket)).await {
                            Ok(next) => next,
                            Err(_) => return Err(timeout_error()),
                        }
                    }
                    None => ws::next_json(&mut socket).await,
                };
                match next {
                    ws::Next::Json(event) => {
                        tasks.apply(&event);
                        if let Some(task) = pick(&tasks) {
                            if task.ended.is_some() {
                                return finish(action_result, task);
                            }
                            if !task.message.is_empty() && task.message != last_message {
                                last_message = task.message.clone();
                                let percent = task
                                    .percent()
                                    .map(|p| format!(" ({p:.0}%)"))
                                    .unwrap_or_default();
                                self.out
                                    .note(format!("{}: {}{percent}", task.name, task.message));
                            }
                        }
                    }
                    ws::Next::Closed(reason) => {
                        self.out.note(format!(
                            "Event stream closed{}, reconnecting",
                            reason.map(|r| format!(" ({r})")).unwrap_or_default()
                        ));
                        sleep_or_timeout(backoff.next_delay(), deadline).await?;
                        break;
                    }
                }
            }
        }
    }
}

fn timeout_error() -> CliError {
    CliError::new(kind::TIMEOUT, "Timed out waiting for the task to end")
}

async fn sleep_or_timeout(delay: Duration, deadline: Option<Instant>) -> CliResult<()> {
    if let Some(deadline) = deadline {
        if Instant::now() + delay > deadline {
            return Err(timeout_error());
        }
    }
    tokio::time::sleep(delay).await;
    Ok(())
}

fn finish<T>(action_result: Option<T>, task: Task) -> CliResult<(T, Task)> {
    if task.ended == Some(false) {
        return Err(CliError::new(
            kind::TASK_FAILED,
            format!(
                "{} failed: {}",
                if task.name.is_empty() {
                    "The task"
                } else {
                    &task.name
                },
                task.end_message.as_deref().unwrap_or("no reason given")
            ),
        ));
    }
    let result = action_result.expect("the action runs before any task is picked");
    Ok((result, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progression(id: &str, inner: Value) -> Value {
        json!({
            "event_inner": {"type": "ProgressionEvent", "event_id": id, "progression_event_inner": inner},
            "details": "",
            "snowflake": "7000000000000000000",
            "level": "Info",
            "caused_by": {"type": "System"}
        })
    }

    #[test]
    fn snowflake_times() {
        assert_eq!(snowflake_millis("0"), Some(LODESTONE_EPOCH_MILLIS));
        assert_eq!(
            snowflake_millis(&(1000i64 << 22).to_string()),
            Some(LODESTONE_EPOCH_MILLIS + 1000)
        );
        assert_eq!(snowflake_millis("x"), None);
    }

    #[test]
    fn filter_json_shape() {
        let filter = EventFilter {
            types: vec!["ProgressionEvent".into()],
            time_range: Some((1, 2)),
            ..EventFilter::default()
        };
        assert_eq!(
            filter.to_query(Some("tok")),
            json!({
                "event_levels": null,
                "event_types": ["ProgressionEvent"],
                "instance_event_types": null,
                "user_event_types": null,
                "event_user_ids": null,
                "event_instance_ids": null,
                "bearer_token": "tok",
                "time_range": {"start": 1, "end": 2},
            })
        );
    }

    #[test]
    fn tasks_follow_progression_events() {
        let mut tasks = Tasks::default();
        tasks.apply(&progression(
            "11",
            json!({"type": "ProgressionStart", "progression_name": "Setting up Minecraft server x", "total": 10.0,
                   "inner": {"type": "InstanceCreation", "instance_uuid": "INSTANCE_1"}}),
        ));
        tasks.apply(&progression(
            "11",
            json!({"type": "ProgressionUpdate", "progress_message": "1/4: Creating directories", "progress": 2.5}),
        ));
        let task = tasks.find("INSTANCE_1").unwrap();
        assert_eq!(task.id, "11");
        assert_eq!(task.percent(), Some(25.0));
        assert_eq!(task.ended, None);
        tasks.apply(&progression(
            "11",
            json!({"type": "ProgressionEnd", "success": true, "message": "Server created successfully",
                   "inner": {"type": "InstanceCreation", "uuid": "INSTANCE_1", "name": "x"}}),
        ));
        let task = tasks.find("11").unwrap();
        assert_eq!(task.ended, Some(true));
        assert_eq!(task.end_value["name"], "x");
        assert_eq!(task.to_json()["state"], "succeeded");
        // other events are ignored
        assert_eq!(
            tasks.apply(&json!({"event_inner": {"type": "UserEvent"}})),
            None
        );
    }

    #[test]
    fn failed_tasks_are_errors() {
        let task = Task {
            name: "Unzipping a.zip".into(),
            ended: Some(false),
            end_message: Some("Unzip failed: bad archive".into()),
            ..Task::default()
        };
        let e = finish(Some(()), task).unwrap_err();
        assert_eq!(e.kind, kind::TASK_FAILED);
        assert_eq!(
            e.message,
            "Unzipping a.zip failed: Unzip failed: bad archive"
        );
    }

    #[test]
    fn descriptions() {
        let event = json!({
            "event_inner": {"type": "InstanceEvent", "instance_uuid": "I", "instance_name": "survival",
                            "instance_event_inner": {"type": "StateTransition", "to": "Running"}},
            "details": "", "snowflake": "0", "level": "Info", "caused_by": {"type": "System"}
        });
        assert_eq!(describe(&event), "survival is now Running");
        assert!(!is_console_event(&event));
        let console = json!({
            "event_inner": {"type": "InstanceEvent", "instance_uuid": "I", "instance_name": "survival",
                            "instance_event_inner": {"type": "InstanceOutput", "message": "Done (3.2s)!"}},
            "details": "", "snowflake": "0", "level": "Info", "caused_by": {"type": "System"}
        });
        assert!(is_console_event(&console));
        let settings = json!({"event_inner": {"type": "GlobalSettingsChanged"}, "details": "Code execution turned off", "snowflake": "0", "level": "Info"});
        assert_eq!(describe(&settings), "Code execution turned off");
    }
}
