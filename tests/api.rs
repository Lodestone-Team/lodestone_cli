//! Integration tests of the API subcommands against a mock core.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::{b64, fixture, instance_list, Cli, MockServer, Reply, Request};
use serde_json::{json, Value};

const SURVIVAL: &str = "INSTANCE_11111111-aaaa";
const SELF_SIGNED_FINGERPRINT: &str =
    "48:0C:EC:A0:35:1C:A3:33:3B:18:5A:BA:C4:11:6E:0F:0E:2A:AA:17:90:16:B1:84:D1:D3:88:55:BB:CD:33:89";

fn core_info() -> Value {
    json!({"version": "0.6.0", "is_setup": true, "os": "linux", "arch": "x86_64", "cpu": "x", "cpu_count": 4,
           "total_ram": 1, "total_disk": 1, "host_name": "h", "uuid": "u", "core_name": "Test core", "up_since": 0,
           "tls": {"mode": "self_signed", "fingerprint_sha256": SELF_SIGNED_FINGERPRINT}})
}

fn login_reply() -> Value {
    json!({"token": "jwt-from-login", "user": {"uid": "USER_1", "username": "owner", "is_owner": true, "is_admin": false, "permissions": {}}})
}

/// A core with the instance list, info and login; other routes answer 404.
fn basic_core(req: &Request) -> Reply {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/info") => Reply::ok(core_info()),
        ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
        ("POST", "/api/v1/user/login") => {
            // "owner:hunter2"
            if req.header("authorization") == Some("Basic b3duZXI6aHVudGVyMg==") {
                Reply::ok(login_reply())
            } else {
                Reply::error(401, "Unauthorized", "Unauthorized")
            }
        }
        _ => Reply::error(404, "NotFound", "Not found"),
    }
}

#[test]
fn login_saves_a_private_profile_and_uses_it() {
    let server = MockServer::start(basic_core);
    let cli = Cli::new();
    let run = cli.run_with_stdin(
        &[
            "login",
            &server.url(),
            "--username",
            "owner",
            "--password-stdin",
            "--json",
        ],
        "hunter2\n",
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = run.json();
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["profile"], "default");
    assert_eq!(doc["user"]["username"], "owner");
    assert_eq!(doc["tls"]["mode"], "system");
    // the token and the password are never printed
    for out in [&run.stdout, &run.stderr] {
        assert!(!out.contains("jwt-from-login"), "{out}");
        assert!(!out.contains("hunter2"), "{out}");
    }
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(cli.profiles_path()).unwrap()).unwrap();
    assert_eq!(saved["current"], "default");
    assert_eq!(saved["profiles"]["default"]["token"], "jwt-from-login");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(cli.profiles_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let run = cli.run(&["instance", "list", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json(), instance_list());
    let list = server.requests_to("GET", "/api/v1/instance/list");
    assert_eq!(
        list[0].header("authorization"),
        Some("Bearer jwt-from-login")
    );

    let run = cli.run(&["profile", "list", "--json"]);
    assert_eq!(run.json()[0]["name"], "default");
    assert!(!run.stdout.contains("jwt-from-login"));

    let run = cli.run(&["logout", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let run = cli.run(&["instance", "list", "--json"]);
    assert_eq!(run.code, 3);
    assert_eq!(run.error_json()["error"]["kind"], "NotLoggedIn");
}

#[test]
fn wrong_password_is_an_auth_error() {
    let server = MockServer::start(basic_core);
    let cli = Cli::new();
    let run = cli.run_with_stdin(
        &[
            "login",
            &server.url(),
            "--username",
            "owner",
            "--password-stdin",
            "--json",
        ],
        "wrong\n",
    );
    assert_eq!(run.code, 3);
    let error = run.error_json();
    assert_eq!(error["error"]["kind"], "Unauthorized");
    assert_eq!(error["error"]["message"], "Wrong user name or password");
    assert!(!cli.profiles_path().exists());
}

#[test]
fn errors_map_to_exit_codes() {
    let server = MockServer::start(|req| match req.path.as_str() {
        "/api/v1/instance/list" => Reply::ok(instance_list()),
        p if p.ends_with("/start") => Reply::error(
            403,
            "PermissionDenied",
            "You don't have permission to start this instance",
        ),
        p if p.ends_with("/stop") => Reply::error(401, "Unauthorized", "Unauthorized"),
        p if p.ends_with("/info") => Reply::error(404, "NotFound", "Instance not found"),
        _ => Reply::Text(500, "boom".into()),
    });
    let cli = Cli::with_env(&server);

    let run = cli.run(&["instance", "start", "survival", "--json"]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.error_json(),
        json!({"error": {"kind": "PermissionDenied", "message": "You don't have permission to start this instance"}})
    );
    assert!(run.stdout.is_empty());

    let run = cli.run(&["instance", "stop", "survival", "--json"]);
    assert_eq!(run.code, 3);
    assert_eq!(run.error_json()["error"]["kind"], "Unauthorized");

    let run = cli.run(&["instance", "info", "INSTANCE_gone", "--json"]);
    assert_eq!(run.code, 1);
    assert_eq!(run.error_json()["error"]["kind"], "NotFound");

    // without --json, errors are text on stderr
    let run = cli.run(&["instance", "info", "INSTANCE_gone"]);
    assert_eq!(run.code, 1);
    assert_eq!(run.stderr.trim(), "error: Instance not found");

    // usage errors exit 2, as JSON with --json
    let run = cli.run(&["instance", "frobnicate", "--json"]);
    assert_eq!(run.code, 2);
    assert_eq!(run.error_json()["error"]["kind"], "Usage");
    let run = cli.run(&["instance", "frobnicate"]);
    assert_eq!(run.code, 2);
}

#[test]
fn instances_resolve_by_name_or_uuid() {
    let server = MockServer::start(|req| match req.path.as_str() {
        "/api/v1/instance/list" => Reply::ok(json!([
            {"uuid": "INSTANCE_a", "name": "Twin"}, {"uuid": "INSTANCE_b", "name": "twin"}, {"uuid": SURVIVAL, "name": "Survival"}
        ])),
        p if p.ends_with("/info") => Reply::ok(json!({"uuid": p.split('/').nth(4), "name": "x"})),
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&["instance", "info", "SURVIVAL", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["uuid"], SURVIVAL);

    let run = cli.run(&["instance", "info", "twin", "--json"]);
    assert_eq!(run.code, 1);
    let error = run.error_json();
    assert_eq!(error["error"]["kind"], "Ambiguous");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("INSTANCE_a") && message.contains("INSTANCE_b"),
        "{message}"
    );

    // a full UUID needs no lookup
    let before = server.requests_to("GET", "/api/v1/instance/list").len();
    let run = cli.run(&["instance", "info", "INSTANCE_b", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        server.requests_to("GET", "/api/v1/instance/list").len(),
        before
    );
}

#[test]
fn start_and_wait_until_running() {
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = polls.clone();
    let server = MockServer::start(move |req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
        ("PUT", p) if p == format!("/api/v1/instance/{SURVIVAL}/start") => Reply::ok(Value::Null),
        ("GET", p) if p == format!("/api/v1/instance/{SURVIVAL}/state") => {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            Reply::ok(json!(if n < 1 { "Starting" } else { "Running" }))
        }
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&["instance", "start", "survival", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json(),
        json!({"ok": true, "uuid": SURVIVAL, "action": "start"})
    );
    let start = server.requests_to("PUT", &format!("/api/v1/instance/{SURVIVAL}/start"));
    assert_eq!(start.len(), 1);
    assert_eq!(start[0].header("authorization"), Some("Bearer test-token"));

    let run = cli.run(&["instance", "start", "survival", "--wait", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["state"], "Running");
    assert!(polls.load(Ordering::SeqCst) >= 2);
}

#[test]
fn destructive_commands_need_confirmation() {
    let server = MockServer::start(|req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
        ("DELETE", _) => Reply::ok(Value::Null),
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&["instance", "delete", "survival", "--json"]);
    assert_eq!(run.code, 1);
    let error = run.error_json();
    assert_eq!(error["error"]["kind"], "ConfirmationRequired");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("--yes"));
    assert!(server.requests().iter().all(|r| r.method != "DELETE"));

    let run = cli.run(&["instance", "delete", "survival", "--yes", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        server
            .requests_to("DELETE", &format!("/api/v1/instance/{SURVIVAL}"))
            .len(),
        1
    );
}

fn mod_list(mods: Value) -> Value {
    json!({"kind": "mod", "loader": "fabric", "game_version": "1.21.1", "directory": "mods", "restart_required": false, "mods": mods})
}

fn versions() -> Value {
    json!([
        {"id": "new-forge", "project_id": "P7dR8mSH", "name": "x", "version_number": "0.110.0+forge", "version_type": "release",
         "game_versions": ["1.21.1"], "loaders": ["forge"], "date_published": "2024-09-02", "downloads": 1, "files": [], "dependencies": []},
        {"id": "beta-fabric", "project_id": "P7dR8mSH", "name": "x", "version_number": "0.105.0-beta", "version_type": "beta",
         "game_versions": ["1.21.1"], "loaders": ["fabric"], "date_published": "2024-09-01", "downloads": 1, "files": [], "dependencies": []},
        {"id": "good-fabric", "project_id": "P7dR8mSH", "name": "x", "version_number": "0.104.0+1.21.1", "version_type": "release",
         "game_versions": ["1.21", "1.21.1"], "loaders": ["fabric"], "date_published": "2024-08-01", "downloads": 1, "files": [], "dependencies": []},
        {"id": "old-fabric", "project_id": "P7dR8mSH", "name": "x", "version_number": "0.92.0+1.20.1", "version_type": "release",
         "game_versions": ["1.20.1"], "loaders": ["fabric"], "date_published": "2023-08-01", "downloads": 1, "files": [], "dependencies": []}
    ])
}

fn mods_core(req: &Request) -> Reply {
    let mods = format!("/api/v1/instance/{SURVIVAL}/mods");
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
        ("GET", p) if p == mods => Reply::ok(mod_list(json!([
            {"file_name": "sodium.jar", "enabled": true, "size": 10, "sha512": "x", "metadata": {"id": "sodium", "name": "Sodium", "version": "0.5", "description": null, "authors": []},
             "modrinth": {"project_id": "AANobbMI", "version_id": "v1", "slug": "sodium", "title": "Sodium", "icon_url": null, "version_number": "0.5",
                          "update": if req.query.contains("check_updates=true") { json!({"version_id": "v2", "version_number": "0.6"}) } else { Value::Null }}}
        ]))),
        ("GET", "/api/v1/modrinth/project/fabric-api") => Reply::ok(
            json!({"id": "P7dR8mSH", "slug": "fabric-api", "project_type": "mod", "title": "Fabric API"}),
        ),
        ("GET", "/api/v1/modrinth/project/P7dR8mSH/versions") => Reply::ok(versions()),
        ("GET", "/api/v1/modrinth/resolve") => {
            assert_eq!(
                req.query_param("url").as_deref(),
                Some("https://modrinth.com/mod/fabric-api/version/0.92.0+1.20.1")
            );
            Reply::ok(
                json!({"project_id": "P7dR8mSH", "slug": "fabric-api", "project_type": "mod", "version_id": "old-fabric"}),
            )
        }
        ("POST", p) if p == format!("{mods}/install") => {
            let body = req.json();
            Reply::ok(
                json!({"installed": [{"file_name": format!("{}.jar", body["version_id"].as_str().unwrap()), "enabled": true, "size": 1, "sha512": "x", "metadata": null,
                                            "modrinth": {"title": "Fabric API", "version_number": "?"}}],
                             "removed": [], "skipped": []}),
            )
        }
        ("POST", p) if p == format!("{mods}/upload") => {
            if req.query.contains("overwrite=true") {
                Reply::ok(
                    json!([{"file_name": "custom.jar", "enabled": true, "size": 5, "sha512": "x", "metadata": null, "modrinth": null}]),
                )
            } else {
                Reply::error(409, "Conflict", "A file named custom.jar already exists")
            }
        }
        ("PUT", p) if p == format!("{mods}/sodium.jar/disable") => {
            Reply::ok(json!({"file_name": "sodium.jar.disabled", "enabled": false}))
        }
        ("DELETE", p) if p == format!("{mods}/sodium.jar") => Reply::ok(Value::Null),
        _ => Reply::error(404, "NotFound", "Not found"),
    }
}

#[test]
fn mods_install_picks_the_newest_compatible_release() {
    let server = MockServer::start(mods_core);
    let cli = Cli::with_env(&server);
    let run = cli.run(&["mods", "survival", "install", "fabric-api", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["installed"][0]["file_name"], "good-fabric.jar");
    let install = server.requests_to("POST", &format!("/api/v1/instance/{SURVIVAL}/mods/install"));
    assert_eq!(
        install[0].json(),
        json!({"version_id": "good-fabric", "install_dependencies": true, "force": false})
    );

    // a version number, without dependencies, forced
    let run = cli.run(&[
        "mods",
        "survival",
        "install",
        "fabric-api",
        "--version",
        "0.92.0+1.20.1",
        "--no-deps",
        "--force",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let install = server.requests_to("POST", &format!("/api/v1/instance/{SURVIVAL}/mods/install"));
    assert_eq!(
        install[1].json(),
        json!({"version_id": "old-fabric", "install_dependencies": false, "force": true})
    );

    // a link to a version
    let run = cli.run(&[
        "mods",
        "survival",
        "install",
        "https://modrinth.com/mod/fabric-api/version/0.92.0+1.20.1",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let install = server.requests_to("POST", &format!("/api/v1/instance/{SURVIVAL}/mods/install"));
    assert_eq!(install[2].json()["version_id"], "old-fabric");
}

#[test]
fn mods_upload_update_disable_remove() {
    let server = MockServer::start(mods_core);
    let cli = Cli::with_env(&server);
    let dir = tempfile::tempdir().unwrap();
    let jar = dir.path().join("custom.jar");
    std::fs::write(&jar, b"PK\x03\x04jar").unwrap();
    let jar = jar.to_str().unwrap();

    let run = cli.run(&["mods", "survival", "upload", jar, "--json"]);
    assert_eq!(run.code, 1);
    let message = run.error_json()["error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.contains("--overwrite"), "{message}");

    let run = cli.run(&["mods", "survival", "upload", jar, "--overwrite", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let upload = server.requests_to("POST", &format!("/api/v1/instance/{SURVIVAL}/mods/upload"));
    let request = upload.last().unwrap();
    assert!(request
        .header("content-type")
        .unwrap()
        .starts_with("multipart/form-data; boundary="));
    let body = String::from_utf8_lossy(&request.body);
    assert!(
        body.contains("name=\"file\"; filename=\"custom.jar\""),
        "{body}"
    );
    assert!(body.contains("PK\u{3}\u{4}jar"), "{body}");
    assert_eq!(
        request.header("content-length").unwrap(),
        request.body.len().to_string()
    );

    let run = cli.run(&["mods", "survival", "update", "--all", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["updated"][0]["to"], "0.6");
    let install = server.requests_to("POST", &format!("/api/v1/instance/{SURVIVAL}/mods/install"));
    assert_eq!(install[0].json()["version_id"], "v2");

    let run = cli.run(&["mods", "survival", "disable", "Sodium", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()[0]["enabled"], false);

    let run = cli.run(&["mods", "survival", "remove", "sodium", "--json"]);
    assert_eq!(run.code, 1);
    assert_eq!(run.error_json()["error"]["kind"], "ConfirmationRequired");
    let run = cli.run(&["mods", "survival", "remove", "sodium", "--yes", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["removed"], json!(["sodium.jar"]));
}

#[test]
fn files_use_base64_paths() {
    let server = MockServer::start(|req| {
        let fs = format!("/api/v1/instance/{SURVIVAL}/fs");
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
            ("GET", p) if p == format!("{fs}/{}/ls", b64(".")) => Reply::ok(json!([
                {"name": "server.properties", "file_stem": "server", "extension": "properties", "path": "server.properties",
                 "size": 12, "creation_time": 1, "modification_time": 1, "file_type": "File"},
                {"name": "world", "file_stem": "world", "extension": null, "path": "world", "size": null,
                 "creation_time": 1, "modification_time": 1, "file_type": "Directory"}
            ])),
            ("GET", p) if p == format!("{fs}/{}/read", b64("server.properties")) => {
                Reply::Text(200, "motd=hi\n".into())
            }
            ("PUT", p) if p == format!("{fs}/{}/write", b64("server.properties")) => {
                Reply::ok(Value::Null)
            }
            ("GET", p) if p == format!("{fs}/{}/url", b64("world")) => {
                Reply::Text(200, "KEY123".into())
            }
            ("GET", "/api/v1/file/KEY123") => Reply::Raw {
                status: 200,
                headers: vec![(
                    "Content-Disposition".into(),
                    "attachment; filename=\"world.zip\"".into(),
                )],
                body: b"zipdata".to_vec(),
            },
            ("PUT", p) if p == format!("{fs}/zip") => Reply::ok(Value::Null),
            ("GET", "/api/v1/fs/L2V0Yy9ob3N0cw/read") => {
                Reply::Text(200, "127.0.0.1 localhost".into())
            }
            _ => Reply::error(404, "NotFound", &format!("no route {}", req.path)),
        }
    });
    let cli = Cli::with_env(&server);

    let run = cli.run(&["files", "survival", "ls", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    // folders first
    assert_eq!(run.json()[0]["name"], "world");

    let run = cli.run(&["files", "survival", "cat", "server.properties"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "motd=hi\n");

    let run = cli.run_with_stdin(
        &["files", "survival", "write", "server.properties", "--json"],
        "motd=new\n",
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    let write = server.requests_to(
        "PUT",
        &format!(
            "/api/v1/instance/{SURVIVAL}/fs/{}/write",
            b64("server.properties")
        ),
    );
    assert_eq!(write[0].body, b"motd=new\n");

    let dest = tempfile::tempdir().unwrap();
    let run = cli.run(&[
        "files",
        "survival",
        "download",
        "world",
        dest.path().to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        std::fs::read(dest.path().join("world.zip")).unwrap(),
        b"zipdata"
    );
    assert_eq!(run.json()["bytes"], 7);

    let run = cli.run(&[
        "files",
        "survival",
        "zip",
        "world",
        "-o",
        "backups/world.zip",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let zip = server.requests_to("PUT", &format!("/api/v1/instance/{SURVIVAL}/fs/zip"));
    assert_eq!(
        zip[0].json(),
        json!({"target_relative_paths": ["world"], "destination_relative_path": "backups/world.zip"})
    );

    // global paths are absolute
    let run = cli.run(&["files", "--global", "cat", "/etc/hosts"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "127.0.0.1 localhost");
    let run = cli.run(&["files", "--global", "zip", "/a", "-o", "/b.zip", "--json"]);
    assert_eq!(run.code, 1);
    assert_eq!(run.error_json()["error"]["kind"], "UnsupportedOperation");
}

fn console_event(message: &str, snowflake: u64) -> Value {
    json!({"event_inner": {"type": "InstanceEvent", "instance_uuid": SURVIVAL, "instance_name": "Survival",
                           "instance_event_inner": {"type": "InstanceOutput", "message": message}},
           "details": "", "snowflake": snowflake.to_string(), "level": "Info", "caused_by": {"type": "System"}})
}

#[test]
fn console_follows_until_a_line_appears() {
    let server = MockServer::start(|req| match req.path.as_str() {
        "/api/v1/instance/list" => Reply::ok(instance_list()),
        p if p.ends_with("/console/buffer") => Reply::ok(json!([
            console_event("Done (old run)", 1),
            console_event("Loading", 2)
        ])),
        p if p.ends_with("/console/stream") => {
            assert_eq!(
                req.query_param("token").as_deref(),
                Some("Bearer test-token")
            );
            Reply::WebSocket(vec![
                console_event("Loading", 2),
                console_event("Preparing spawn", 3),
                console_event("Done (2.1s)! For help, type \"help\"", 4),
                console_event("after", 5),
            ])
        }
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&[
        "console",
        "survival",
        "--follow",
        "--until",
        "Done (",
        "--timeout",
        "120",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    // the old "Done" from the buffer does not end the wait, and lines are not repeated
    assert_eq!(
        run.stdout,
        "Done (old run)\nLoading\nPreparing spawn\nDone (2.1s)! For help, type \"help\"\n"
    );

    let run = cli.run(&["console", "survival", "-n", "1", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json()[0]["event_inner"]["instance_event_inner"]["message"],
        "Loading"
    );

    let server2 = MockServer::start(|req| match req.path.as_str() {
        "/api/v1/instance/list" => Reply::ok(instance_list()),
        p if p.ends_with("/console") => Reply::ok(Value::Null),
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let cli2 = Cli::with_env(&server2);
    let run = cli2.run(&[
        "console", "send", "survival", "say", "hello", "world", "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let sent = server2.requests_to("POST", &format!("/api/v1/instance/{SURVIVAL}/console"));
    assert_eq!(sent[0].json(), json!("say hello world"));
}

fn progression(id: &str, inner: Value) -> Value {
    json!({"event_inner": {"type": "ProgressionEvent", "event_id": id, "progression_event_inner": inner},
           "details": "", "snowflake": "100", "level": "Info", "caused_by": {"type": "System"}})
}

#[test]
fn tasks_wait_for_instance_creation() {
    let server = MockServer::start(|req| match req.path.as_str() {
        "/api/v1/events/all/buffer" => Reply::ok(json!([progression(
            "77",
            json!({
            "type": "ProgressionStart", "progression_name": "Setting up Minecraft server new", "total": 10.0,
            "inner": {"type": "InstanceCreation", "instance_uuid": "INSTANCE_new"}})
        )])),
        "/api/v1/events/all/stream" => {
            let filter: Value = serde_json::from_str(&req.query_param("filter").unwrap()).unwrap();
            assert_eq!(filter["bearer_token"], "test-token");
            assert_eq!(filter["event_types"], json!(["ProgressionEvent"]));
            Reply::WebSocket(vec![
                progression(
                    "77",
                    json!({"type": "ProgressionUpdate", "progress_message": "2/4: Downloading", "progress": 5.0}),
                ),
                progression(
                    "77",
                    json!({"type": "ProgressionEnd", "success": true, "message": "Instance created successfully",
                                         "inner": {"type": "InstanceCreation", "uuid": "INSTANCE_new", "name": "new"}}),
                ),
            ])
        }
        p if p.ends_with("/info") => Reply::error(404, "NotFound", "Instance not found"),
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&[
        "tasks",
        "--wait",
        "INSTANCE_new",
        "--timeout",
        "120",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let task = run.json();
    assert_eq!(task["state"], "succeeded");
    assert_eq!(task["id"], "77");
    assert_eq!(task["result"]["name"], "new");

    let failing = MockServer::start(|req| match req.path.as_str() {
        "/api/v1/events/all/buffer" => Reply::ok(json!([])),
        "/api/v1/events/all/stream" => Reply::WebSocket(vec![
            progression(
                "78",
                json!({"type": "ProgressionStart", "progression_name": "Setting up Minecraft server bad", "total": null,
                                     "inner": {"type": "InstanceCreation", "instance_uuid": "INSTANCE_bad"}}),
            ),
            progression(
                "78",
                json!({"type": "ProgressionEnd", "success": false, "message": "Instance creation failed: no Java", "inner": null}),
            ),
        ]),
        _ => Reply::error(404, "NotFound", "Instance not found"),
    });
    let cli = Cli::with_env(&failing);
    let run = cli.run(&[
        "tasks",
        "--wait",
        "INSTANCE_bad",
        "--timeout",
        "120",
        "--json",
    ]);
    assert_eq!(run.code, 1);
    let error = run.error_json();
    assert_eq!(error["error"]["kind"], "TaskFailed");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no Java"));
}

#[test]
fn instance_create_sends_the_setup_value() {
    let server = MockServer::start(|req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/setup_manifest/MinecraftFabric") => {
            Reply::ok(json!({"setting_sections": {
                "section_1": {"section_id": "section_1", "name": "Basic", "description": "", "settings": {
                    "version": {"setting_id": "version", "value": {"type": "Enum", "value": "1.21.4"}, "value_type": {"type": "Enum", "options": ["1.21.4", "1.21.1"]}},
                    "port": {"setting_id": "port", "value": {"type": "UnsignedInteger", "value": 25565}, "value_type": {"type": "UnsignedInteger", "min": 1, "max": 65535}}}},
                "section_2": {"section_id": "section_2", "name": "Advanced", "description": "", "settings": {
                    "min_ram": {"setting_id": "min_ram", "value": {"type": "UnsignedInteger", "value": 1024}, "value_type": {"type": "UnsignedInteger", "min": 1, "max": null}},
                    "max_ram": {"setting_id": "max_ram", "value": {"type": "UnsignedInteger", "value": 2048}, "value_type": {"type": "UnsignedInteger", "min": 1, "max": null}},
                    "cmd_args": {"setting_id": "cmd_args", "value": null, "value_type": {"type": "String", "regex": null}}}}
            }}))
        }
        ("GET", p) if p.starts_with("/api/v1/check/port/") => {
            let port: u16 = p.rsplit('/').next().unwrap().parse().unwrap();
            Reply::ok(json!({"is_in_use": port == 25565, "is_allocated": false}))
        }
        ("POST", "/api/v1/instance/create/MinecraftFabric") => Reply::ok(json!("INSTANCE_created")),
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&[
        "instance",
        "create",
        "--flavour",
        "fabric",
        "--version",
        "1.21.1",
        "--name",
        "My Server",
        "--max-ram",
        "4096",
        "--restart-on-crash",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json(),
        json!({"ok": true, "uuid": "INSTANCE_created", "name": "My Server", "state": "creating"})
    );
    let create = server.requests_to("POST", "/api/v1/instance/create/MinecraftFabric");
    assert_eq!(
        create[0].json(),
        json!({
            "name": "My Server", "description": null, "auto_start": false, "restart_on_crash": true,
            "setting_sections": {
                "section_1": {"settings": {"version": {"value": {"type": "Enum", "value": "1.21.1"}}, "port": {"value": {"type": "UnsignedInteger", "value": 25566}}}},
                "section_2": {"settings": {"min_ram": {"value": {"type": "UnsignedInteger", "value": 1024}}, "max_ram": {"value": {"type": "UnsignedInteger", "value": 4096}}, "cmd_args": {"value": null}}}
            }
        })
    );

    let run = cli.run(&[
        "instance",
        "create",
        "--flavour",
        "fabric",
        "--version",
        "1.7.10",
        "--name",
        "x",
        "--json",
    ]);
    assert_eq!(run.code, 2);
    let run = cli.run(&["instance", "create", "--flavour", "fabric", "--json"]);
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert!(run.error_json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("--name"));
}

#[test]
fn user_permissions_are_granted_by_instance_name() {
    let server = MockServer::start(|req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/user/info") => {
            Reply::ok(json!({"uid": "USER_owner", "username": "owner"}))
        }
        ("GET", "/api/v1/user/list") => Reply::ok(
            json!([{"uid": "USER_owner", "username": "owner"}, {"uid": "USER_bob", "username": "bob"}]),
        ),
        ("GET", "/api/v1/user/USER_bob") => Reply::ok(
            json!({"uid": "USER_bob", "username": "bob", "is_owner": false, "is_admin": false,
            "permissions": {"can_start_instance": ["INSTANCE_other"], "can_create_instance": false}}),
        ),
        ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
        ("PUT", "/api/v1/user/USER_bob/update_perm") => Reply::ok(Value::Null),
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&[
        "users",
        "perms",
        "grant",
        "bob",
        "start_instance",
        "survival",
        "creative",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let update = server.requests_to("PUT", "/api/v1/user/USER_bob/update_perm");
    assert_eq!(
        update[0].json()["can_start_instance"],
        json!(["INSTANCE_other", SURVIVAL, "INSTANCE_22222222-bbbb"])
    );
    let run = cli.run(&[
        "users",
        "perms",
        "set",
        "bob",
        "can_create_instance",
        "true",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let update = server.requests_to("PUT", "/api/v1/user/USER_bob/update_perm");
    assert_eq!(update[1].json()["can_create_instance"], true);
}

/* ---------------------------------------------------------------- TLS */

#[test]
fn tls_login_needs_the_fingerprint_and_pins_it() {
    let server = MockServer::start_tls("test-cert.pem", "test-key.pem", basic_core);
    let cli = Cli::new();

    // not interactive and no --fingerprint: refuse, and show the fingerprint to check
    let run = cli.run_with_stdin(
        &[
            "login",
            &server.url(),
            "--username",
            "owner",
            "--password-stdin",
            "--json",
        ],
        "hunter2\n",
    );
    assert_eq!(run.code, 1);
    let error = run.error_json();
    assert_eq!(error["error"]["kind"], "UntrustedCertificate");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.contains(SELF_SIGNED_FINGERPRINT), "{message}");
    // no password was sent
    assert!(server
        .requests()
        .iter()
        .all(|r| r.path != "/api/v1/user/login"));

    // a wrong fingerprint is refused
    let wrong = "00".repeat(32);
    let run = cli.run_with_stdin(
        &[
            "login",
            &server.url(),
            "--username",
            "owner",
            "--password-stdin",
            "--fingerprint",
            &wrong,
            "--json",
        ],
        "hunter2\n",
    );
    assert_eq!(run.code, 1);
    assert_eq!(run.error_json()["error"]["kind"], "UntrustedCertificate");

    // the right one, in lower case without colons
    let given = SELF_SIGNED_FINGERPRINT.replace(':', "").to_lowercase();
    let run = cli.run_with_stdin(
        &[
            "login",
            &server.url(),
            "--username",
            "owner",
            "--password-stdin",
            "--fingerprint",
            &given,
            "--json",
        ],
        "hunter2\n",
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json()["tls"],
        json!({"mode": "pinned", "fingerprint_sha256": SELF_SIGNED_FINGERPRINT})
    );

    // later commands trust only that certificate
    let run = cli.run(&["instance", "list", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json(), instance_list());
}

#[test]
fn a_pinned_profile_refuses_another_certificate() {
    let other = MockServer::start_tls("ca-signed-cert.pem", "ca-signed-key.pem", basic_core);
    let mut cli = Cli::new();
    cli.env.push(("LODESTONE_URL".into(), other.url()));
    cli.env
        .push(("LODESTONE_TOKEN".into(), "test-token".into()));
    cli.env.push((
        "LODESTONE_FINGERPRINT".into(),
        SELF_SIGNED_FINGERPRINT.into(),
    ));
    let run = cli.run(&["instance", "list", "--json"]);
    assert_eq!(run.code, 1);
    let error = run.error_json();
    assert_eq!(error["error"]["kind"], "UntrustedCertificate", "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("certificate changed"),
        "{error}"
    );
    assert!(other.requests().is_empty());
}

#[test]
fn tls_login_with_a_ca_certificate() {
    let server = MockServer::start_tls("ca-signed-cert.pem", "ca-signed-key.pem", basic_core);
    let cli = Cli::new();
    let ca = fixture("test-ca.pem");
    let run = cli.run_with_stdin(
        &[
            "login",
            &server.url(),
            "--username",
            "owner",
            "--password-stdin",
            "--ca-cert",
            ca.to_str().unwrap(),
            "--json",
        ],
        "hunter2\n",
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["tls"]["mode"], "ca_cert");
    let run = cli.run(&["core", "info", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["core_name"], "Test core");

    // the CA does not vouch for the self-signed certificate
    let self_signed = MockServer::start_tls("test-cert.pem", "test-key.pem", basic_core);
    let run = Cli::new().run_with_stdin(
        &[
            "login",
            &self_signed.url(),
            "--username",
            "owner",
            "--password-stdin",
            "--ca-cert",
            ca.to_str().unwrap(),
            "--json",
        ],
        "hunter2\n",
    );
    assert_eq!(run.code, 1);
    assert_eq!(run.error_json()["error"]["kind"], "UntrustedCertificate");
}

#[test]
fn verbose_shows_requests_but_never_the_token() {
    // the plain HTTP warning for other machines is tested in the unit tests, so this test
    // only talks to the loopback mock core
    let server = MockServer::start(basic_core);
    let run = Cli::with_env(&server).run(&["core", "info", "--json", "--verbose"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(!run.stderr.contains("plain HTTP"), "{}", run.stderr);
    assert!(run.stderr.contains("> GET /api/v1/info"), "{}", run.stderr);
    assert!(!run.stderr.contains("test-token"), "{}", run.stderr);
}

#[test]
fn auto_start_and_restart_on_crash_use_their_routes() {
    let server = MockServer::start(|req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
        ("PUT", p) if p == format!("/api/v1/instance/{SURVIVAL}/auto_start") => {
            Reply::ok(Value::Null)
        }
        ("PUT", p) if p == format!("/api/v1/instance/{SURVIVAL}/restart_on_crash") => {
            Reply::ok(Value::Null)
        }
        // an older core has no such route: axum answers a bare 404
        _ => Reply::Text(404, String::new()),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&[
        "instance",
        "settings",
        "set",
        "survival",
        "auto_start",
        "true",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json(),
        json!({"ok": true, "uuid": SURVIVAL, "setting": "auto_start", "value": true})
    );
    let run = cli.run(&[
        "instance",
        "settings",
        "set",
        "survival",
        "restart-on-crash",
        "off",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        server.requests_to("PUT", &format!("/api/v1/instance/{SURVIVAL}/auto_start"))[0].json(),
        json!(true)
    );
    assert_eq!(
        server.requests_to(
            "PUT",
            &format!("/api/v1/instance/{SURVIVAL}/restart_on_crash")
        )[0]
        .json(),
        json!(false)
    );
    let run = cli.run(&[
        "instance",
        "settings",
        "set",
        "survival",
        "auto_start",
        "maybe",
        "--json",
    ]);
    assert_eq!(run.code, 2);

    let old = MockServer::start(|req| match req.path.as_str() {
        "/api/v1/instance/list" => Reply::ok(instance_list()),
        _ => Reply::Text(404, String::new()),
    });
    let run = Cli::with_env(&old).run(&[
        "instance",
        "settings",
        "set",
        "survival",
        "auto_start",
        "true",
        "--json",
    ]);
    assert_eq!(run.code, 1);
    assert_eq!(run.error_json()["error"]["kind"], "UnsupportedOperation");
}

/// A core whose zip route answers `zip_reply`; its event stream shows another new zip
/// failing before the requested one succeeds.
fn zip_core(zip_reply: Value) -> MockServer {
    MockServer::start(move |req| match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
        ("PUT", p) if p.ends_with("/fs/zip") => Reply::ok(zip_reply.clone()),
        ("GET", "/api/v1/events/all/buffer") => Reply::ok(json!([
            progression(
                "1",
                json!({"type": "ProgressionStart", "progression_name": "Zipping world", "total": null, "inner": null})
            ),
            progression(
                "1",
                json!({"type": "ProgressionEnd", "success": true, "message": "old", "inner": null})
            ),
        ])),
        ("GET", "/api/v1/events/all/stream") => Reply::WebSocket(vec![
            progression(
                "556",
                json!({"type": "ProgressionStart", "progression_name": "Zipping other", "total": null, "inner": null}),
            ),
            progression(
                "555",
                json!({"type": "ProgressionStart", "progression_name": "Zipping world", "total": null, "inner": null}),
            ),
            progression(
                "556",
                json!({"type": "ProgressionEnd", "success": false, "message": "Zip failed: other", "inner": null}),
            ),
            progression(
                "555",
                json!({"type": "ProgressionEnd", "success": true, "message": "Zipped", "inner": null}),
            ),
        ]),
        _ => Reply::error(404, "NotFound", "Not found"),
    })
}

#[test]
fn zip_waits_for_the_task_id_the_core_returns() {
    let server = zip_core(json!("555"));
    let cli = Cli::with_env(&server);
    let run = cli.run(&[
        "files", "survival", "zip", "world", "-o", "w.zip", "--wait", "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["task"]["id"], "555");
    assert_eq!(run.json()["task"]["state"], "succeeded");

    let run = cli.run(&["files", "survival", "zip", "world", "-o", "w.zip", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["task_id"], "555");

    // an older core returns null: the first new zip task is followed instead
    let old = zip_core(Value::Null);
    let run = Cli::with_env(&old).run(&[
        "files", "survival", "zip", "world", "-o", "w.zip", "--wait", "--json",
    ]);
    assert_eq!(run.code, 1);
    assert_eq!(run.error_json()["error"]["kind"], "TaskFailed");
}

#[test]
fn create_atom_returns_and_waits_for_the_uuid() {
    let server = MockServer::start(|req| match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/api/v1/instance/create_generic") => {
            assert_eq!(req.json()["url"], "https://example.com/atom");
            Reply::ok(json!("INSTANCE_atom"))
        }
        ("GET", "/api/v1/events/all/buffer") => Reply::ok(json!([])),
        ("GET", "/api/v1/events/all/stream") => Reply::WebSocket(vec![
            progression(
                "9",
                json!({"type": "ProgressionStart", "progression_name": "Setting up instance a", "total": 100.0,
                                    "inner": {"type": "InstanceCreation", "instance_uuid": "INSTANCE_atom"}}),
            ),
            progression(
                "9",
                json!({"type": "ProgressionEnd", "success": true, "message": "Instance created successfully", "inner": null}),
            ),
        ]),
        _ => Reply::error(404, "NotFound", "Instance not found"),
    });
    let cli = Cli::with_env(&server);
    let run = cli.run(&[
        "instance",
        "create-atom",
        "--url",
        "https://example.com/atom",
        "--name",
        "a",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json(),
        json!({"ok": true, "uuid": "INSTANCE_atom", "name": "a", "state": "creating"})
    );
    let run = cli.run(&[
        "instance",
        "create-atom",
        "--url",
        "https://example.com/atom",
        "--name",
        "a",
        "--wait",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["state"], "created");
    assert_eq!(run.json()["task"]["id"], "9");

    let old = MockServer::start(|_| Reply::ok(Value::Null));
    let run = Cli::with_env(&old).run(&[
        "instance",
        "create-atom",
        "--url",
        "https://example.com/atom",
        "--name",
        "a",
        "--wait",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["uuid"], Value::Null);
    assert!(run.stderr.contains("does not return"), "{}", run.stderr);
}

#[test]
fn extension_manifest_sends_the_url_as_a_query() {
    let server = MockServer::start(|req| match req.path.as_str() {
        "/api/v1/extension/fetchmanifest" => {
            assert!(req.body.is_empty());
            assert_eq!(
                req.query_param("url").as_deref(),
                Some("https://github.com/a/b?x=1&y=2")
            );
            Reply::ok(json!({"manifest": {"name": "b"}, "username": "a", "is_domain_true": true}))
        }
        _ => Reply::error(404, "NotFound", "Not found"),
    });
    let run = Cli::with_env(&server).run(&[
        "extension",
        "manifest",
        "https://github.com/a/b?x=1&y=2",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["manifest"]["name"], "b");
}

/* ------------------------------------------------------------- API keys */

const KEY: &str = "lsk_ab12cd34ef56_c2VjcmV0LXNlY3JldC1zZWNyZXQtc2VjcmV0LXNlY3JldA";
const KEY_SECRET: &str = "c2VjcmV0LXNlY3JldC1zZWNyZXQtc2VjcmV0LXNlY3JldA";
const NEW_KEY: &str = "lsk_zz99yy88xx77_bmV3LWtleS1uZXcta2V5LW5ldy1rZXktbmV3LWtleS1uZXc";

fn key_info(id: &str, name: &str, scope: Value) -> Value {
    json!({"id": id, "name": name, "display": format!("lsk_{id}_…"), "uid": "USER_owner",
           "created_at": 1700000000, "expires_at": null, "last_used_at": null, "scope": scope})
}

fn session_only() -> Reply {
    Reply::error(
        403,
        "PermissionDenied",
        "This action needs a signed-in session, not an API key",
    )
}

/// A core that accepts the session token `test-token` and the API key `KEY` (which is
/// refused the session-only routes, as the spec says).
fn keys_core(req: &Request) -> Reply {
    let bearer = req.header("authorization").unwrap_or_default().to_string();
    let is_key = bearer == format!("Bearer {KEY}");
    if !is_key && bearer != "Bearer test-token" && req.path != "/api/v1/info" {
        return Reply::error(401, "Unauthorized", "Unauthorized");
    }
    let session_only_route = req.path.contains("/api_keys");
    if is_key && session_only_route {
        return session_only();
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/info") => Reply::ok(core_info()),
        ("GET", "/api/v1/user/info") => Reply::ok(
            json!({"uid": "USER_owner", "username": "owner", "is_owner": true, "is_admin": false, "permissions": {}}),
        ),
        ("GET", "/api/v1/user/list") => Reply::ok(
            json!([{"uid": "USER_owner", "username": "owner"}, {"uid": "USER_bob", "username": "bob"}]),
        ),
        ("GET", "/api/v1/instance/list") => Reply::ok(instance_list()),
        ("GET", "/api/v1/user/api_keys") => Reply::ok(json!([
            key_info("ab12cd34ef56", "ci", json!({"type": "full"})),
            key_info(
                "qq11ww22ee33",
                "agent",
                json!({"type": "custom", "permissions": {"can_view_instance": [SURVIVAL]}})
            ),
        ])),
        ("GET", "/api/v1/user/USER_bob/api_keys") => Reply::ok(json!([key_info(
            "bb11bb22bb33",
            "bob's laptop",
            json!({"type": "full"})
        )])),
        ("POST", "/api/v1/user/api_keys") => {
            let body = req.json();
            Reply::ok(
                json!({"key": NEW_KEY, "info": key_info("zz99yy88xx77", body["name"].as_str().unwrap(), body["scope"].clone())}),
            )
        }
        ("DELETE", "/api/v1/user/api_keys/qq11ww22ee33")
        | ("DELETE", "/api/v1/user/USER_bob/api_keys/bb11bb22bb33") => Reply::ok(Value::Null),
        // a careless core echoing the key it was given
        _ if is_key => Reply::error(400, "BadRequest", &format!("bad token {KEY}")),
        _ => Reply::error(404, "NotFound", "Not found"),
    }
}

fn assert_no_secret(run: &common::Run, secret: &str) {
    assert!(
        !run.stdout.contains(secret),
        "stdout shows the key: {}",
        run.stdout
    );
    assert!(
        !run.stderr.contains(secret),
        "stderr shows the key: {}",
        run.stderr
    );
}

#[test]
fn login_with_an_api_key_from_stdin_or_the_environment() {
    let server = MockServer::start(keys_core);
    let cli = Cli::new();
    let run = cli.run_with_stdin(
        &["login", &server.url(), "--api-key", "--json", "--verbose"],
        &format!("{KEY}\n"),
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_no_secret(&run, KEY_SECRET);
    let doc = run.json();
    assert_eq!(
        doc["auth"],
        json!({"type": "api_key", "key": "lsk_ab12cd34ef56_…"})
    );
    assert_eq!(doc["user"]["username"], "owner");
    let info = server.requests_to("GET", "/api/v1/user/info");
    assert_eq!(
        info[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    // no password login happened
    assert!(server.requests_to("POST", "/api/v1/user/login").is_empty());

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(cli.profiles_path()).unwrap()).unwrap();
    assert_eq!(saved["profiles"]["default"]["token"], KEY);
    assert_eq!(saved["profiles"]["default"]["auth"], "api_key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(cli.profiles_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    for args in [
        &["profile", "list", "--json"][..],
        &["profile", "list"],
        &["instance", "list", "--verbose"],
    ] {
        let run = cli.run(args);
        assert_eq!(run.code, 0, "{}", run.stderr);
        assert_no_secret(&run, KEY_SECRET);
    }
    let run = cli.run(&["profile", "list", "--json"]);
    assert_eq!(run.json()[0]["auth"]["key"], "lsk_ab12cd34ef56_…");
    let list = server.requests_to("GET", "/api/v1/instance/list");
    assert_eq!(
        list.last().unwrap().header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );

    // from LODESTONE_API_KEY, with stdin left alone
    let mut cli = Cli::new();
    cli.env.push(("LODESTONE_API_KEY".into(), KEY.into()));
    let run = cli.run(&["login", &server.url(), "--api-key", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_no_secret(&run, KEY_SECRET);
}

#[test]
fn a_refused_or_malformed_key_is_never_echoed() {
    let server = MockServer::start(keys_core);
    let wrong = "lsk_ab12cd34ef56_d3Jvbmctd3Jvbmctd3Jvbmctd3Jvbmctd3Jvbmctd3Jvbmc";
    let run = Cli::new().run_with_stdin(
        &["login", &server.url(), "--api-key", "--json"],
        &format!("{wrong}\n"),
    );
    assert_eq!(run.code, 3);
    assert_no_secret(&run, "d3Jvbmc");
    let message = run.error_json()["error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.contains("lsk_ab12cd34ef56_…"), "{message}");

    // a password typed by mistake is not echoed either
    let run = Cli::new().run_with_stdin(
        &["login", &server.url(), "--api-key", "--json"],
        "hunter2\n",
    );
    assert_eq!(run.code, 2);
    assert_no_secret(&run, "hunter2");

    // the key cannot be a command line argument
    let run = Cli::new().run(&["login", &server.url(), "--api-key", KEY]);
    assert_eq!(run.code, 2);

    // a key echoed back in a core's error message is redacted
    let mut cli = Cli::with_env(&server);
    cli.env.retain(|(k, _)| k != "LODESTONE_TOKEN");
    cli.env.push(("LODESTONE_TOKEN".into(), KEY.into()));
    let run = cli.run(&["files", "--global", "cat", "/x", "--json"]);
    assert_eq!(run.code, 1);
    assert_no_secret(&run, KEY_SECRET);
    assert_eq!(
        run.error_json()["error"]["message"],
        "bad token lsk_ab12cd34ef56_…"
    );
    let run = cli.run(&["files", "--global", "cat", "/x", "--verbose"]);
    assert_no_secret(&run, KEY_SECRET);
    assert!(
        run.stderr.contains("bad token lsk_ab12cd34ef56_…"),
        "{}",
        run.stderr
    );
}
