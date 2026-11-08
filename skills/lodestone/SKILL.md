---
name: lodestone
description: Manage Lodestone game servers with the `lodestone_cli` command line tool. Use it to create, start, stop and configure Minecraft servers on a Lodestone Core, install and update mods or plugins from Modrinth, create servers from modpacks, read the console, edit, back up and download server files, and manage users and their permissions.
---

# Lodestone CLI

`lodestone_cli` talks to a running Lodestone Core over its REST and WebSocket API. It can do
everything the Lodestone dashboard can. Add `--help` to any command to see all its options.

## Install and authenticate

Download the binary for your platform from
https://github.com/Lodestone-Team/lodestone_cli/releases. Without a subcommand the same binary
installs and runs Lodestone Core itself; this skill only uses the subcommands.

### Agents: use an API key that the user gives you

Authenticate with an **API key**, not with the user's password. The user creates a key for
you, limited to what you need (a *custom* scope: some servers and some actions, and an
expiry), in the dashboard (User settings, API keys) or with `api-keys create`. They put it in
your environment, together with the core's URL and, for the default self-signed certificate,
its fingerprint:

- `LODESTONE_URL`: the core, such as `https://192.168.1.20:16662`;
- `LODESTONE_TOKEN`: the key (`lsk_...`). It takes any bearer token, so keys work there;
- `LODESTONE_FINGERPRINT`: the certificate's SHA-256 fingerprint (or `LODESTONE_CA_CERT`).

Then every command just works, with no login step:

```sh
lodestone_cli --json users me
lodestone_cli --json server list
```

- **Never print, store or echo the key.** Do not `echo` it, write it to a file, put it on a
  command line or in a message. The CLI itself only ever shows keys as `lsk_<id>_…`.
- **Never create, revoke or ask for keys on your own initiative**, and do not ask for the
  user's password instead. If the key lacks a permission (kind `PermissionDenied`), tell the
  user which one and let them decide.
- A key that was revoked or has expired fails with exit code 3 and kind `Unauthorized`; ask
  the user for a new one.

### People: log in once and save a profile

Log in with a password; the login is saved as a profile:

```sh
lodestone_cli login https://192.168.1.20:16662 --username alice
```

- The URL defaults to `https://` and port 16662, so `lodestone_cli login 192.168.1.20` works.
- Non-interactive: pass `--username` and `--password-stdin` (password on the first line of
  stdin), or set `LODESTONE_USERNAME` and `LODESTONE_PASSWORD`:

```sh
printf '%s\n' "$LODESTONE_PASSWORD" | lodestone_cli login 192.168.1.20 --username alice --password-stdin
```

- Or save an API key as a profile: `lodestone_cli login 192.168.1.20 --api-key` reads the
  key from `LODESTONE_API_KEY`, or from stdin (without echo on a terminal). The key is never
  a command line argument. Managing keys and changing passwords need a password login.
- Cores use a self-signed certificate by default. On the first login the CLI shows its SHA-256
  fingerprint (such as `AB:CD:...`) and asks the user to compare it with the fingerprint in the
  core's log or in Core Settings on the dashboard. Without a terminal it refuses and prints the
  fingerprint. **Never confirm a fingerprint yourself.** Ask the user to check it, then pass
  it:

```sh
lodestone_cli login 192.168.1.20 --username alice --password-stdin --fingerprint AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89
```

  After that, the profile accepts only that certificate. `--ca-cert ca.pem` trusts a CA
  instead. Certificate checks cannot be turned off.
- Profiles: `lodestone_cli profile list`, `lodestone_cli profile use home`,
  `lodestone_cli logout`. Pick one per command with `--profile home`.
- Environment variables override the profile: `LODESTONE_URL`, `LODESTONE_TOKEN` (a session
  token or an API key), and for TLS `LODESTONE_FINGERPRINT` or `LODESTONE_CA_CERT` (a PEM
  file).
- A brand-new core has no owner yet: `lodestone_cli core setup 192.168.1.20 SETUP_KEY --username alice`
  creates the owner with the setup key from the core's log, and logs in.

## Output, errors and exit codes

**Always pass `--json`.** It prints exactly one JSON document on stdout: the core's response,
or `{"ok": true, ...}` for commands that only change something. Commands that stream
(`--follow`) print NDJSON, one JSON object per line. Progress messages go to stderr; never
parse stderr except for errors.

Errors go to stderr. With `--json` they are one line: `{"error": {"kind": "...", "message": "..."}}`.
`kind` is the core's error kind (`NotFound`, `BadRequest`, `PermissionDenied`, `Unauthorized`,
...) or one of the CLI's (`NotLoggedIn`, `Ambiguous`, `ConfirmationRequired`, `Timeout`,
`TaskFailed`, `UntrustedCertificate`, `Network`, `Usage`).

| Exit code | Meaning |
|---|---|
| 0 | Success |
| 1 | The core refused, or the request cannot be done (see `kind`) |
| 2 | Wrong command line |
| 3 | Not logged in, or the login expired: run `login` again |

Servers are named by UUID or by name (any case). An ambiguous name fails with kind
`Ambiguous` and lists the UUIDs.

## Command reference

Core:

```sh
lodestone_cli --json core info
lodestone_cli --json core system
lodestone_cli --json core settings get
lodestone_cli --json core settings set name "My Core"
lodestone_cli --json core settings set safe-mode true
lodestone_cli --json core games
lodestone_cli --json core check-port 25565
```

Settings for `core settings set`: `name`, `safe-mode`, `code-execution`, `domain`,
`allow-lan-http`, `playit` (owner only).

Servers (`instance` is the old name of `server` and still works):

```sh
lodestone_cli --json server list
lodestone_cli --json server info survival
lodestone_cli --json server versions fabric
lodestone_cli --json server create --flavour fabric --version 1.21.1 --name survival --max-ram 4096 --wait
lodestone_cli --json server create-modpack https://modrinth.com/modpack/fabulously-optimized --name fo --wait
lodestone_cli --json server create-modpack ./pack.mrpack --name mypack --inspect
lodestone_cli --json server start survival --wait
lodestone_cli --json server stop survival --wait
lodestone_cli --json server restart survival
lodestone_cli --json server kill survival --yes
lodestone_cli --json server rename survival "Survival 2"
lodestone_cli --json server description survival "Our main world"
lodestone_cli --json server settings get survival
lodestone_cli --json server settings set survival max_ram 6144
lodestone_cli --json server settings set survival max-players 30
lodestone_cli --json server settings set survival auto_start true
lodestone_cli --json server settings set survival restart_on_crash true
lodestone_cli --json server delete survival --yes
```

- Flavours: `vanilla`, `fabric`, `forge`, `paper`. `--version latest` is the default.
  Without `--port` the first free port from 25565 is used.
- `server settings get` lists every setting id. `server.properties` settings (such as
  `motd`, `difficulty`, `max-players`) appear after the server has run once. Use
  `section.setting` if an id is in two sections.
- Auto start and restart on crash are set with `--auto-start` and `--restart-on-crash` at
  creation, or later with `server settings set <server> auto_start|restart_on_crash true|false`.
- `server create-atom --url <url> --name <name> --wait` (owner only, needs code execution)
  and `server docker-watch <container>` exist too. `create-atom` returns the new UUID.

Console, players, resource usage:

```sh
lodestone_cli --json console survival --lines 50
lodestone_cli --json console survival --follow --until "Done (" --timeout 300
lodestone_cli --json console send survival "say Backup in 5 minutes"
lodestone_cli --json players survival
lodestone_cli --json players survival --set-max 30
lodestone_cli --json monitor survival
```

`--until` only matches lines that arrive after the command starts, never older lines.

Files (paths are relative to the server folder; `--global` uses absolute paths on the
core's machine and needs global file permissions):

```sh
lodestone_cli --json files survival ls
lodestone_cli --json files survival ls world
lodestone_cli files survival cat server.properties
lodestone_cli --json files survival write server.properties --from ./server.properties
lodestone_cli --json files survival upload ./icon.png ./ops.json --to .
lodestone_cli --json files survival download world ./backups
lodestone_cli --json files survival mkdir backups
lodestone_cli --json files survival touch notes.txt
lodestone_cli --json files survival mv notes.txt docs/notes.txt
lodestone_cli --json files survival cp world backups
lodestone_cli --json files survival zip world -o backups/world.zip --wait
lodestone_cli --json files survival unzip backups/world.zip --to restored --wait
lodestone_cli --json files survival rm notes.txt --yes
lodestone_cli --json files survival rm old-world -r --yes
lodestone_cli --json files --global ls /home/minecraft
```

`files write` reads stdin when `--from` is missing. Downloading a folder gives a zip.
`zip` and `unzip` run in the background and return the task id as `task_id`; with `--wait`
they wait for that task and exit 1 with kind `TaskFailed` if it fails.

Mods and plugins (Fabric, Forge, Paper and Spigot servers; plugin servers use the same
commands):

```sh
lodestone_cli --json mods survival list
lodestone_cli --json mods survival list --check-updates
lodestone_cli --json mods survival search sodium --sort downloads
lodestone_cli --json mods survival show sodium
lodestone_cli --json mods survival versions sodium
lodestone_cli --json mods survival install fabric-api
lodestone_cli --json mods survival install https://modrinth.com/mod/lithium
lodestone_cli --json mods survival install sodium --version mc1.21.1-0.6.0-fabric
lodestone_cli --json mods survival upload ./my-mod.jar
lodestone_cli --json mods survival disable sodium
lodestone_cli --json mods survival enable sodium
lodestone_cli --json mods survival update --all
lodestone_cli --json mods survival remove sodium --yes
lodestone_cli --json modrinth search "fabulously optimized" --type modpack
lodestone_cli --json modrinth versions fabulously-optimized
```

- `install` takes a slug, project id, version id or Modrinth link. By default it installs the
  newest release for the server's loader and game version, with required dependencies
  (`--no-deps` to skip them). `--force` installs a version for another loader or game version.
- Mods are named by file name, slug, Modrinth title or the name in the jar.
- After changes to a running server the result says a restart is needed.

Macros (owner only, needs code execution on):

```sh
lodestone_cli --json macros survival list
lodestone_cli --json macros survival run backup nightly
lodestone_cli --json macros survival tasks
lodestone_cli --json macros survival history
lodestone_cli --json macros survival config get backup
lodestone_cli --json macros survival config set backup keep 7
```

Users and permissions:

```sh
lodestone_cli --json users list
lodestone_cli --json users me
printf '%s\n' "$NEW_PASSWORD" | lodestone_cli --json users create bob --password-stdin
lodestone_cli --json users perms get bob
lodestone_cli --json users perms grant bob view_instance survival
lodestone_cli --json users perms revoke bob start_instance survival
lodestone_cli --json users perms set bob can_create_instance false
lodestone_cli --json users rename bob robert
lodestone_cli --json users delete robert --yes
```

Per-server permissions (their names still say `instance`): `view_instance`,
`start_instance`, `stop_instance`, `access_instance_console`, `access_instance_setting`,
`read_instance_resource`, `write_instance_resource`, `access_instance_macro`,
`read_instance_file`, `write_instance_file`. For the whole core (true or false): `create_instance`,
`delete_instance`, `read_global_file`, `write_global_file`, `manage_permission`,
`install_extension`. The `can_` prefix is optional.

API keys (for the user; these need a password login, and agents never run them on their own
initiative):

```sh
lodestone_cli --json api-keys list
lodestone_cli --json api-keys create --name agent --expires 30d --allow view_instance=survival --allow start_instance=survival --allow access_instance_console=survival
lodestone_cli --json api-keys create --name ci --full --expires 90d
lodestone_cli --json api-keys revoke lsk_ab12cd34ef56_… --yes
lodestone_cli --json api-keys list --user bob
```

- `--allow PERMISSION=VALUE` takes the permissions of `users perms set`, repeated: server
  names for per-server permissions, `true` for the others. `--full` gives everything the
  user can do. `--expires` is `30d`, `90d` (default), `1y` or `never`.
- `create` prints the key once (in `key` with `--json`) and never again.
- With an API key login, these fail with "needs a signed-in session".

Events, tasks, playit.gg:

```sh
lodestone_cli --json events --limit 20
lodestone_cli --json events --level warning,error --since 24h
lodestone_cli --json events --server survival --follow
lodestone_cli --json tasks
lodestone_cli --json tasks --wait INSTANCE_8f0c1b2e-0000-4000-8000-000000000000 --timeout 1800
lodestone_cli --json playit status
lodestone_cli --json playit tunnels
```

`tasks --wait` takes a task id (such as the `task_id` of `files zip`) or the UUID of a
server being created; it exits 1 with kind `TaskFailed` if the task fails.

## Workflows

Create a Fabric server, add Fabric API and a mod from a Modrinth link, start it (ask the
user about the EULA first, see the safety rules), and wait until it is up:

```sh
lodestone_cli --json server create --flavour fabric --version 1.21.1 --name survival --wait
lodestone_cli --json mods survival install fabric-api
lodestone_cli --json mods survival install https://modrinth.com/mod/lithium
lodestone_cli --json server start survival
lodestone_cli --json console survival --follow --until "Done (" --timeout 600
```

`server create` without `--wait` returns `{"ok": true, "uuid": ...}` at once; then wait with
`lodestone_cli --json tasks --wait <uuid>`. `server start survival --wait` also waits until
the state is `Running`.

Update every mod, then restart so the changes apply:

```sh
lodestone_cli --json mods survival list --check-updates
lodestone_cli --json mods survival update --all
lodestone_cli --json server restart survival --wait
```

Back up a world (zip it on the core, then download the zip):

```sh
lodestone_cli --json console send survival save-all
lodestone_cli --json files survival zip world -o backups/world-2026-11-07.zip --wait
lodestone_cli --json files survival download backups/world-2026-11-07.zip ./backups
```

Add a user who can only see, start and stop one server and use its console:

```sh
printf '%s\n' "$NEW_PASSWORD" | lodestone_cli --json users create bob --password-stdin
lodestone_cli --json users perms grant bob view_instance survival
lodestone_cli --json users perms grant bob start_instance survival
lodestone_cli --json users perms grant bob stop_instance survival
lodestone_cli --json users perms grant bob access_instance_console survival
lodestone_cli --json users perms get bob
```

## Safety rules for agents

- **Destructive commands need the user's confirmation** in the conversation before you run
  them: `server delete`, `server kill`, `files rm`, `mods remove`, `users delete`,
  `macros kill`, overwriting with `files write` or `mods upload --overwrite`, and turning off
  `code-execution` (it stops all macros). Only then pass `--yes`. Without `--yes` and without a
  terminal they fail with kind `ConfirmationRequired` instead of hanging.
- **Starting a Minecraft server accepts Mojang's EULA** (https://aka.ms/MinecraftEULA). Only
  the user can agree to it. Ask before the first `server start`, `restart` or a creation
  with `--auto-start`, and never write `eula=true` yourself.
- **Authenticate with the API key the user gave you** (`LODESTONE_URL` and `LODESTONE_TOKEN`),
  not with their password. **Never print, store or echo keys, tokens or passwords**, never
  pass them as command line arguments, and never create or revoke API keys unless the user
  asks. Never confirm a TLS fingerprint on the user's behalf.
- **Macros and atom servers are owner only**, and also need code execution to be on. A
  `PermissionDenied` error for them is expected for other users; do not try to work around it.
- Prefer `--wait` and `tasks --wait` over polling, and use `--timeout` so a command cannot run
  forever.
