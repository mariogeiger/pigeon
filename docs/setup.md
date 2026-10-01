# Setting up a group

This guide founds a group, brings in its members and their machines, and
sets up an always-on server and the group's own relay. Every command below
also exists as a form in the web interface that `pigeon ui` opens. Each
command asks for a missing argument when a terminal is attached, and
prints JSON with `--json`. On a machine with several groups, `-g <group>`
picks one.

## 1. Install pigeon on each machine

Install [Rust](https://rustup.rs), then:

```sh
cargo install --git https://github.com/mariogeiger/pigeon pigeon
```

Run the same command with `--force` to update.

## 2. Keep the daemon running

`pigeon daemon` syncs every group of the machine and serves the API and the
web interface on localhost port 6767, or the one `--port` names. The link
`pigeon ui` prints leaves the browser a cookie that lasts 400 days, so
afterwards <http://127.0.0.1:6767> opens the interface directly.

The daemon keeps its state in `$PIGEON_HOME`, by default a `pigeon` folder
in the user's data folder. Start it at login. On Linux, a systemd user
service does this:

```ini
# ~/.config/systemd/user/pigeon.service
[Unit]
Description=pigeon

[Service]
ExecStart=%h/.cargo/bin/pigeon daemon
Restart=on-failure

[Install]
WantedBy=default.target
```

```sh
systemctl --user enable --now pigeon
```

On macOS and Windows, add `pigeon daemon` to the programs started at login.

## 3. Found the group

```sh
pigeon group create --name cheapmo --member mario
```

The group name and member names use 1 to 32 characters among `a-z` and
`0-9`. pigeon asks for your password. Use the same one on each of your
machines.

The group's files live in its root folder: `/cheapmo` on Linux and macOS,
and `C:\cheapmo` on Windows, where programs on drive C: also resolve
`/cheapmo`. The root has the same path on every machine, so paths written
inside files work everywhere. When only an administrator can create the
root, pigeon prints the command to run once, and you then run
`pigeon group create` again:

- Linux: `sudo install -d -o "$USER" /cheapmo`.
- macOS keeps `/` read-only. The command adds a line to
  `/etc/synthetic.conf` that links `/cheapmo` to `~/cheapmo`.
- Windows: `mkdir` and `icacls`, in a Command Prompt run as administrator.

`--root <folder>` picks another folder instead, at the cost of paths that
differ from the other machines.

## 4. Bring in members and machines

```sh
pigeon group key
```

prints the group key. Send it privately, since it admits machines into the
group. The newcomer then runs, on each of their machines:

```sh
pigeon group join --key <the key> --member alice
```

with the same name and password on all of them. Keep the machine that gave
the key online until the newcomer has joined. `pigeon group join` waits for
the group's answer. If the name belongs to another password, it says so
and prints the `pigeon member claim` command that logs in with the right
one.

To manage members:

- `pigeon member password` changes your password. Your other machines then
  log in with `pigeon member claim`.
- `pigeon member reset --member alice` gives alice a new password, which you
  then tell her.
- `pigeon member exclude --member alice` excludes alice, and
  `pigeon group leave` excludes yourself. The name stays taken. Excluding
  renews the group key: machines already in the group keep working, but the
  old key admits no new machine, so run `pigeon group key` again for the
  next newcomer.

## 5. Organize the files

- A folder named `@` and a member's name, such as `/cheapmo/src/@mario`,
  belongs to that member. Only their machines write it, and everyone reads
  it. A member can have as many such folders as they like, anywhere in the
  tree.
- Every other folder is a drop folder, where anyone adds files. A new file
  is published once it has not changed for five minutes. After that it is
  frozen, and only a request changes it. `pigeon file pending` lists the
  edits still waiting, and `pigeon file publish --path <file or folder>`
  publishes them at once.
- To change a file you do not own, write it anyway with
  `pigeon file write --path <path> --content <local file> --mode propose`,
  or `--mode force`. Its owner sees the request with `pigeon request list`
  and answers with `pigeon request accept` or `pigeon request refuse`. A
  forced request is applied without waiting.
- A `.pigeonignore` file, in the gitignore syntax, keeps files out of
  publication. pigeon never publishes them, not even their names.
- A file pigeon may not publish, such as an edit in someone else's folder,
  is set aside. `pigeon aside list` shows these files, and `pigeon aside
  restore`, `request` or `discard` deals with them.

## 6. Choose what each machine holds

Everyone sees the whole tree, but a machine downloads only what its
selection follows: at first, the member's own folders. Patterns use the
`.pigeonignore` syntax, and the last matching rule wins:

```sh
pigeon selection follow --pattern /docs/
pigeon selection download --pattern /photos/2024/
pigeon selection unfollow --pattern /docs/old/ --free
pigeon selection pin --pattern /report/ --time 2026-10-01T12:00:00Z
```

`follow` keeps files in sync, `download` takes their current version once,
and `unfollow` stops syncing them, keeping the files unless `--free`
removes them. `pin` holds files as they were at a past time.

To edit every rule at once, write them one per line, the last matching
rule winning, and preview them before saving:

```sh
pigeon selection list > rules.txt
cat rules.txt
# follow @mario/
# frozen 2026-10-01T12:00:00Z /report/
# free *.iso
pigeon selection preview --rules rules.txt
pigeon selection set --rules rules.txt --version <the version the preview showed>
```

`frozen now` freezes files at the time of saving. The preview counts the
files and bytes held now and after saving, and what saving would download,
free and freeze, with the rule that decides each file. `set` replaces the
whole selection, keeping the rules as written, and with `--version` it
refuses if the selection changed since the preview. A modified copy not yet
published is never removed.

The web UI's Selection page edits a draft of the rules the same way: each
row tells how many files its rule matches and decides, the preview beside
it updates on each keystroke and as files arrive, and nothing changes
until Save, which asks first when it frees space. If the selection changes
elsewhere meanwhile, a banner offers to reload it or keep the draft.

The Files page gives each file and folder a box: checked
when followed, mixed when only part of a folder is. Unchecking asks whether
to keep the current copy, frozen, or free the space. A rule set this way
replaces the earlier rules for the paths inside it; hand-written patterns
such as `*.pdf` stay. The group's pages update themselves as files change.

`pigeon selection place --folder videos --destination /mnt/big/videos`
keeps a folder on another disk and leaves a link at its place, a junction
on Windows. While the destination is missing, such as an unplugged disk,
the folder waits: nothing in it syncs, and nothing counts as deleted.

Each machine keeps past versions of the files its member writes. It keeps
every version for a day, then one a day for a month and one a week for a
year, within 20% of the disk. `pigeon retention set` changes this, and
`--everything on` extends it to every file the machine downloads.

## 7. Run a server

An always-on machine, usually headless, keeps the files available while
their owners' machines are off. It joins as a member of its own:

```sh
pigeon group join --key <the key> --member server
pigeon selection follow --pattern '*'
pigeon retention set --everything on
```

It owns no folder, so it writes nobody's files. Run its daemon as the
systemd service above, and let it run without a login session with
`loginctl enable-linger "$USER"`. Manage it over SSH:
`ssh server .cargo/bin/pigeon group status`.

## 8. Run the group's relay

Machines connect directly when they can. Otherwise a relay carries their
traffic. It sees only ciphertext. By default these are iroh's public
relays. The group can run its own relay instead, on a machine reachable
from the internet on ports 80 and 443 under a domain name, such as the
server:

```sh
pigeon relay --hostname relay.example.org --contact you@example.org
```

serves it with a Let's Encrypt certificate and prints its URL. Run it as a
service too. On Linux, binding ports 80 and 443 takes root or, once,
`sudo setcap cap_net_bind_service=+ep ~/.cargo/bin/pigeon`. Then any member
names the relay for every machine of the group:

```sh
pigeon group relay --url https://relay.example.org
```

`pigeon group status` shows the relay each machine reached.
`pigeon group relay` without `--url` goes back to iroh's public relays.
