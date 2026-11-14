//! Command line parsing tests, including every example command in the agent skill file.

use clap::{CommandFactory, Parser};

use crate::api::cli::{Command, ConsoleCmd, EventTypeArg, FilesCmd, InstanceCmd, ModsCmd};
use crate::Args;

const SKILL: &str = include_str!("../skills/lodestone/SKILL.md");

/// Splits a shell command line into words: single and double quotes group words,
/// backslashes escape outside single quotes. Returns the commands of a pipeline
/// (`a | b`), each without leading `VAR=value` assignments.
fn shell_commands(line: &str) -> Vec<Vec<String>> {
    let mut commands = vec![Vec::new()];
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    in_word = true;
                }
            }
            (None, ' ' | '\t') => {
                if in_word {
                    commands.last_mut().unwrap().push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, '|') => {
                if in_word {
                    commands.last_mut().unwrap().push(std::mem::take(&mut word));
                    in_word = false;
                }
                commands.push(Vec::new());
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    assert!(quote.is_none(), "unclosed quote in: {line}");
    if in_word {
        commands.last_mut().unwrap().push(word);
    }
    commands
        .into_iter()
        .map(|words| {
            words
                .into_iter()
                .skip_while(|w| {
                    w.split_once('=').is_some_and(|(k, _)| {
                        !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                    })
                })
                .collect()
        })
        .collect()
}

/// Every `lodestone_cli` command in the skill file: lines of ```sh blocks (joined at
/// trailing backslashes) and inline code spans.
fn skill_examples(skill: &str) -> Vec<Vec<String>> {
    let mut lines = Vec::new();
    let mut in_block = false;
    let mut pending = String::new();
    for line in skill.lines() {
        if line.trim_start().starts_with("```") {
            in_block = !in_block && line.trim_start() == "```sh";
            continue;
        }
        if in_block {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(start) = line.strip_suffix('\\') {
                pending.push_str(start);
                continue;
            }
            pending.push_str(line);
            lines.push(std::mem::take(&mut pending));
        } else {
            let mut rest = line;
            while let Some(start) = rest.find('`') {
                let after = &rest[start + 1..];
                let Some(end) = after.find('`') else { break };
                let span = &after[..end];
                if span.starts_with("lodestone_cli ") {
                    lines.push(span.to_string());
                }
                rest = &after[end + 1..];
            }
        }
    }
    lines
        .iter()
        .flat_map(|l| shell_commands(l))
        .filter(|words| words.first().map(String::as_str) == Some("lodestone_cli"))
        .collect()
}

fn parse(words: &[&str]) -> Result<Args, clap::Error> {
    Args::try_parse_from(std::iter::once("lodestone_cli").chain(words.iter().copied()))
}

#[test]
fn the_command_definitions_are_valid() {
    Args::command().debug_assert();
}

#[test]
fn trace_watch_takes_patterns() {
    use crate::api::cli::TraceCmd;
    let args = parse(&[
        "trace",
        "watch",
        "log.warn",
        "*.error",
        "--backlog",
        "5",
        "--count",
        "3",
    ])
    .unwrap();
    let Some(Command::Trace(TraceCmd::Watch(watch))) = args.command else {
        panic!("not trace watch")
    };
    assert_eq!(watch.topics, ["log.warn", "*.error"]);
    assert_eq!((watch.backlog, watch.count), (5, Some(3)));
    assert!(matches!(
        parse(&["trace", "snapshot"]).unwrap().command,
        Some(Command::Trace(TraceCmd::Snapshot))
    ));
}

#[test]
fn every_skill_example_parses() {
    let examples = skill_examples(SKILL);
    // a Windows checkout may give the file CRLF line endings
    assert_eq!(skill_examples(&SKILL.replace('\n', "\r\n")), examples);
    assert!(
        examples.len() > 80,
        "found only {} examples",
        examples.len()
    );
    for words in &examples {
        let args = Args::try_parse_from(words)
            .unwrap_or_else(|e| panic!("`{}` does not parse:\n{e}", words.join(" ")));
        assert!(
            args.command.is_some(),
            "`{}` is not a subcommand",
            words.join(" ")
        );
    }
}

#[test]
fn shell_splitting() {
    assert_eq!(
        shell_commands(r#"printf '%s\n' "$X" | LODESTONE_URL=x lodestone_cli a "b c" d\ e"#),
        vec![
            vec!["printf".to_string(), "%s\\n".to_string(), "$X".to_string()],
            vec!["lodestone_cli", "a", "b c", "d e"]
                .into_iter()
                .map(String::from)
                .collect()
        ]
    );
}

#[test]
fn legacy_flags_still_work() {
    let args = parse(&[]).unwrap();
    assert!(args.command.is_none());
    let args = parse(&[
        "-y",
        "-r",
        "-s",
        "--version",
        "0.5.1",
        "-i",
        "/opt/lodestone",
    ])
    .unwrap();
    assert!(args.command.is_none());
    assert!(args.yes_all && args.run_core && args.skip_update_check);
    assert_eq!(args.version.unwrap().to_string(), "v0.5.1");
    assert_eq!(args.install_path.unwrap().to_str(), Some("/opt/lodestone"));
    assert!(
        parse(&["--uninstall", "--list-versions"])
            .unwrap()
            .uninstall
    );
    // args.json keeps working and never holds the new options
    let from_file: Args = serde_json::from_str(r#"{"run_core": true}"#).unwrap();
    assert!(from_file.run_core && from_file.command.is_none());
}

#[test]
fn global_options_go_anywhere() {
    for words in [
        &["--json", "instance", "list"][..],
        &["instance", "--json", "list"],
        &["instance", "list", "--json"],
    ] {
        let args = parse(words).unwrap();
        assert!(args.api.json, "{words:?}");
        assert!(matches!(
            args.command,
            Some(Command::Instance(InstanceCmd::List))
        ));
    }
    let args = parse(&["--profile", "home", "--yes", "instance", "delete", "x"]).unwrap();
    assert_eq!(args.api.profile.as_deref(), Some("home"));
    assert!(args.api.yes);
}

#[test]
fn server_and_instance_both_work() {
    for word in ["server", "instance"] {
        let args = parse(&[word, "start", "survival", "--wait"]).unwrap();
        let Some(Command::Instance(InstanceCmd::Start(start))) = args.command else {
            panic!("{word}")
        };
        assert_eq!(start.power.instance, "survival");
        assert!(start.power.wait);
        // the EULA is only accepted when asked (LS-08)
        assert!(!start.accept_eula);
    }
    let args = parse(&["server", "restart", "survival", "--accept-eula"]).unwrap();
    let Some(Command::Instance(InstanceCmd::Restart(restart))) = args.command else {
        panic!("restart")
    };
    assert!(restart.accept_eula);
    // stopping has nothing to do with the EULA
    assert!(parse(&["server", "stop", "survival", "--accept-eula"]).is_err());
    for flag in ["--server", "--instance"] {
        let args = parse(&["events", flag, "a,b"]).unwrap();
        let Some(Command::Events(events)) = args.command else {
            panic!("{flag}")
        };
        assert_eq!(events.instance, ["a", "b"]);
    }
    for value in ["server", "instance"] {
        let args = parse(&["events", "--type", value]).unwrap();
        let Some(Command::Events(events)) = args.command else {
            panic!("{value}")
        };
        assert_eq!(events.event_type, [EventTypeArg::Instance]);
    }
}

#[test]
fn files_take_a_server_or_global() {
    let args = parse(&["files", "survival", "ls", "world"]).unwrap();
    let Some(Command::Files(files)) = args.command else {
        panic!()
    };
    assert_eq!(files.instance.as_deref(), Some("survival"));
    assert!(matches!(files.action, FilesCmd::Ls { path: Some(ref p) } if p == "world"));

    let args = parse(&["files", "--global", "ls", "/tmp"]).unwrap();
    let Some(Command::Files(files)) = args.command else {
        panic!()
    };
    assert!(files.global && files.instance.is_none());

    let args = parse(&["files", "survival", "cp", "a", "b", "dest"]).unwrap();
    let Some(Command::Files(files)) = args.command else {
        panic!()
    };
    let FilesCmd::Cp { sources, dest } = files.action else {
        panic!()
    };
    assert_eq!(
        (sources, dest.as_str()),
        (vec!["a".to_string(), "b".to_string()], "dest")
    );

    assert!(parse(&["files", "ls"]).is_err());
    assert!(parse(&["files", "survival", "--global", "ls", "/"]).is_err());
    assert!(parse(&["files", "survival", "unzip", "a.zip", "--to", "x", "--smart"]).is_err());
}

#[test]
fn console_shorthand_and_send() {
    let args = parse(&["console", "survival", "-f", "--until", "Done"]).unwrap();
    let Some(Command::Console(console)) = args.command else {
        panic!()
    };
    assert!(console.action.is_none());
    assert_eq!(console.show.instance.as_deref(), Some("survival"));
    assert!(console.show.follow);

    let args = parse(&[
        "console", "send", "survival", "tp", "@p", "~", "-10", "~", "--json",
    ])
    .unwrap();
    assert!(args.api.json);
    let Some(Command::Console(console)) = args.command else {
        panic!()
    };
    let Some(ConsoleCmd::Send { instance, command }) = console.action else {
        panic!()
    };
    assert_eq!(instance, "survival");
    assert_eq!(command.join(" "), "tp @p ~ -10 ~");

    // --until needs --follow
    assert!(parse(&["console", "survival", "--until", "Done"]).is_err());
    assert!(parse(&["console"]).is_err());
}

#[test]
fn mods_commands() {
    let args = parse(&["mods", "survival", "update", "--all"]).unwrap();
    let Some(Command::Mods(mods)) = args.command else {
        panic!()
    };
    assert!(matches!(
        mods.action,
        ModsCmd::Update {
            all: true,
            r#mod: None
        }
    ));
    assert!(parse(&["mods", "survival", "update"]).is_err());
    assert!(parse(&["mods", "survival", "update", "sodium", "--all"]).is_err());
    let args = parse(&[
        "mods", "survival", "search", "fabric", "api", "--limit", "5",
    ])
    .unwrap();
    let Some(Command::Mods(mods)) = args.command else {
        panic!()
    };
    let ModsCmd::Search { query, page, .. } = mods.action else {
        panic!()
    };
    assert_eq!(query.join(" "), "fabric api");
    assert_eq!(page.limit, 5);
    assert!(parse(&["mods", "survival", "search", "x", "--limit", "500"]).is_err());
}

#[test]
fn usage_errors_exit_with_code_2() {
    let error = parse(&["instance", "frobnicate"]).unwrap_err();
    assert_eq!(error.exit_code(), 2);
}
