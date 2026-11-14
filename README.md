# Lodestone CLI

Lodestone CLI is a command line tool that helps you install, update and run Lodestone Core.

You should always install Lodestone CLI over Lodestone Core, since Lodestone Core does not have the ability to self-update.

# Platform support

Lodestone CLI is available for all platform that Lodestone Core supports:
- Linux (x86_64)
- Windows (x86_64) 
- MacOS (aarch64).

# Installation

Lodestone CLI is a self-contained exectuable, you can simply download and run the newest release [here](https://github.com/Lodestone-Team/lodestone_cli/releases)

# Managing a running core

The same binary can do everything the Lodestone dashboard can, from the command line. Log in
once, then use the subcommands:

```sh
lodestone_cli login 192.168.1.20          # https and port 16662 by default
lodestone_cli server list
lodestone_cli server create --flavour fabric --version 1.21.1 --name survival --wait
lodestone_cli mods survival install fabric-api
lodestone_cli server eula survival --accept   # you agree to the Minecraft EULA: https://aka.ms/MinecraftEULA
lodestone_cli server start survival --wait
lodestone_cli console survival --follow
```

| Command | What it does |
|---|---|
| `login`, `logout`, `profile` | Log in and manage saved logins (profiles) |
| `core` | Core info, global settings, system usage, first-time setup |
| `server` (alias `instance`) | List, create (also from a modpack), accept the Minecraft EULA, start, stop, rename, settings, delete |
| `console`, `players`, `monitor` | Console output and commands, players, CPU and memory |
| `files` | A server's files, or the core machine's with `--global` |
| `mods`, `modrinth` | Mods and plugins from Modrinth or uploaded jars; Modrinth search |
| `macros` | Run and configure macros (owner only) |
| `users` | Users, passwords and permissions |
| `api-keys` | Create, list and revoke API keys |
| `events`, `tasks` | Notifications, live events, and waiting for background tasks |
| `playit`, `extension` | playit.gg tunnels and extensions (owner only) |
| `trace` | The core's debug traces, live: its log, API requests, server states, tasks (owner only) |

Run `lodestone_cli <command> --help` for the details.

- **Scripts and agents:** add `--json` for machine-readable output. Exit codes are 0 for
  success, 1 for an error from the core, 2 for a wrong command line and 3 when not logged in.
  `LODESTONE_URL` and `LODESTONE_TOKEN` override the saved login.
- **API keys:** give scripts, CI and agents a named, limited, expiring key instead of your
  password: `lodestone_cli api-keys create --name ci --allow view_instance=survival` prints
  it once. Use it as `LODESTONE_TOKEN`, or save it with `lodestone_cli login <url> --api-key`
  (it reads the key from stdin or `LODESTONE_API_KEY`). `api-keys list` and `api-keys revoke`
  manage them; keys are only ever shown as `lsk_<id>_…`.
- **Certificates:** cores use a self-signed certificate. On the first login the CLI shows its
  SHA-256 fingerprint; compare it with the one in the core's log or Core Settings. After that
  the login accepts only that certificate. `--fingerprint` and `--ca-cert` do the same without
  a prompt.
- **Destructive commands** ask first; `--yes` skips the question.
- **Debug traces:** `trace snapshot`, `trace topics` and `trace watch [patterns…]` read what
  the core is doing: its log (`log.warn`, `log.error`, …), API requests (method, route,
  status, time), server states, players, tasks, macros and other events. Owner only. The
  core keeps the last 2000 events in memory and sends them only to the owner's connection;
  busy topics (console lines, every request, debug logs) only while someone watches.
  Payloads are scrubbed of anything that looks like a password, token or key.

The agent skill in [`skills/lodestone/SKILL.md`](skills/lodestone/SKILL.md) describes every
command, the JSON output, common workflows and safety rules for AI agents.
