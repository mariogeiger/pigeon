# Setting up a group

This guide creates a group, brings in its members and their machines, and
sets up an always-on server and the group's own relay. Everything below can
also be done in the web interface the daemon links to. Each
command asks for a missing argument when a terminal is attached, and
prints JSON with `--json`. On a machine with several groups, `-g <group>`
picks one.

## 1. Install pigeon on each machine

On Linux and macOS,

```sh
curl -sSf https://raw.githubusercontent.com/mariogeiger/pigeon/main/install.sh | sh
```

installs Rust with rustup if cargo is missing, builds pigeon the way
`pigeon update` does, from the newest release, and runs `pigeon setup`.
Elsewhere, install [Rust](https://rustup.rs), then run
`cargo install --locked --git https://github.com/mariogeiger/pigeon --tag <newest release> pigeon`
and
`pigeon setup`. The setup asks, step by step, the questions the commands
below answer: complete with Tab in the shell, start at login, join or
create a group, where its root folder goes, and what to follow. Running it again resumes where the machine
stands.

Tab completes commands, flags and the values the daemon knows: groups,
members, the group's paths and patterns one folder at a time, version
times and suggestion ids. `pigeon setup` offers to add the line that
loads it to the file your shell reads as it starts: `~/.zshrc`,
`~/.bashrc` or fish's `config.fish`. Elsewhere, load the script
`pigeon completions <shell>` prints, for elvish or PowerShell too:

```sh
source <(pigeon completions zsh)
```

To update, run `pigeon update`. It brings a clone of pigeon kept in your
cache folder to the newest release with git, the highest tag
`vMAJOR.MINOR.PATCH` of the repository and never the head of main, then
builds it with cargo, in your terminal, in a build folder kept between
updates, so only what moved recompiles. It puts the program it built in
the place of the one the daemon runs, keeping that one beside it as
`pigeon.previous`, and says where. The daemon then restarts onto the new
program, unless the build left it unchanged, and web pages left open
offer to reload. `pigeon update --rollback` puts the previous program
back the same way, keeping the one it replaces as the previous one, so
that running it again undoes it. `--path <clone>` builds a local clone of
the repository instead, for development. While the repository has no
release, `pigeon update` says so and changes nothing.
Machines whose versions of pigeon cannot talk to each
other show as incompatible in `pigeon group status` and on the group's web
page, each with its member, its version and commit, and whether it is
older or newer than this one: each machine asks the others over
`pigeon/hello`, a protocol that never changes, and one that does not
answer predates it, so it is older. Update the older ones. A machine this
one failed to sync with shows too, with why, until a session with it
opens. Patches dated more than five minutes ahead of this machine's clock
wait, with an error saying so, until one of the two clocks is set right;
every session compares what both sides hold each minute, so they then
arrive by themselves, as does anything a session missed.

## 2. Keep the daemon running

`pigeon daemon` syncs every group of the machine and serves the API and the
web interface on localhost port 6767, or the one `--port` names. The link
it prints at start, which `pigeon ui` prints again, leaves the browser a
cookie that lasts 400 days, so afterwards <http://127.0.0.1:6767> opens the
interface directly. `pigeon daemon stop` stops it.

On Linux, pigeon keeps its files in three `pigeon` folders, where the XDG
base directories put configuration, data and state. On macOS one folder,
`~/Library/Application Support/pigeon`, holds all three, and on Windows
`%LOCALAPPDATA%\pigeon`; `$PIGEON_HOME` names one folder for all three.

- The configuration, in `~/.config/pigeon`: for each group,
  `groups/<group>/config.toml` holds the member, the root, the selection,
  the retention and the places. Edit it by hand, then apply it with
  `pigeon daemon reload`, which changes nothing if a group's file does not
  read, tells what the edits download, free and pin on this machine,
  and asks first when they free space; `--yes` skips the question. The
  Overview page of the web interface edits the same file. pigeon rewrites the file whole, without your comments, whenever it
  changes a setting, and refuses to while the file holds edits it has not
  read.
- The data, in `~/.local/share/pigeon`: for each group, in
  `groups/<group>/`, `secrets.toml` holds the group key and this machine's
  secret key, to share with nobody, and `state.redb` and `blobs/` the
  patches, what the disk holds, and the files' contents.
- The state, in `~/.local/state/pigeon`: `daemon.toml` holds the token that
  guards the API and the address the daemon listens on, `daemon.log` the
  output, appended across starts, of a daemon started by hand, and `relay/` the certificates of a
  relay.

```sh
pigeon service install
```

starts it at login, as a systemd user service on Linux or a launchd agent
on macOS, in place of a daemon started by hand. The service starts the daemon
again five seconds after it crashes, however many times, and the panic
message stays in the journal (`journalctl --user -u pigeon`). `--linger` starts it at
boot too, without a login, as a server needs. On Windows, add
`pigeon daemon` to the programs started at login.

## 3. Create the group

```sh
pigeon group create --name cheapmo --member mario
```

The group name and member names use 1 to 32 characters among `a-z` and
`0-9`. Use the same member name on each of your machines.

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
group. The newcomer then runs `pigeon setup`, or, on each of their machines:

```sh
pigeon group names --key <the key>
pigeon group join --key <the key> --member alice
```

with the same name on all of them. `pigeon group names` hears the group
without joining it and lists its members, to add a machine of theirs, and
the names taken. Keep the machine that gave the key
online until the newcomer has joined. `pigeon group join` waits for the
group's answer. If another key holds the name or a folder claims it, it
says so and prints the `pigeon member claim` command that claims another
name.

`pigeon group leave` takes this machine out of the group: it stops syncing
the group and forgets its key, secrets and state, keeping the files on
disk, and tells the group nothing, so the member's name stays taken and
their other machines go on. Whoever holds the group key may join, so share
it only with those meant to.

## 5. Organize the files

Everyone may change every file. Two rules say which changes a machine
publishes by itself; every other change becomes a suggestion that the
whole group sees and anyone decides.

- A tag, `+` followed by a member's name, makes a file that member's,
  whether it names a folder holding the file, as in
  `/cheapmo/src/+mario/plan.txt`, or sits in the file's own name, as in
  `/cheapmo/docs/texte+mario.txt`. Read from right
  to left, the first tag naming a member decides: `+mario/+emmy/a` is
  emmy's. The owner's machines publish the owner's edits, additions and
  deletions there within seconds. A tag `+<name>` that names no member
  changes nothing, but while it exists nobody can join under that name.
- Every other path belongs to no one, and anyone may add a file there. A
  new file is a draft while it changes, which the other machines see, and
  is published once it has not changed for five minutes; it then has no
  owner, and pigeon shows who made it. `pigeon file pending` lists the
  edits still waiting, and `pigeon file publish --path <file or folder>`
  publishes them at once.
- Any other change made on disk, such as an edit in someone else's
  folder, a deletion of someone else's file, an edit of a published file
  that belongs to no one, or a name some system cannot hold, becomes a
  suggestion. The disk that made it keeps it until someone decides. A
  move on disk is one change, which keeps the file's history: moving a
  folder outside the rules suggests one move per file. When two machines
  change a file while apart, the later change wins and the other becomes
  a suggestion that its disk keeps.
- `pigeon suggestion list` shows every suggestion, oldest first, with why
  it waits, and an id. Anyone, from any machine, decides one or several
  at once: `pigeon suggestion validate --suggestions <ids>` publishes
  them, the later one winning at a path, and `--to <path>` publishes the
  one file of a single suggestion at another path, as a name Windows
  cannot hold needs; `pigeon suggestion discard --suggestions <ids>`
  brings back the group's version on the disk that suggested it. An id
  names the suggestion as it was listed: one that changed or was decided
  since is refused, so the first decision is final and decides what was
  shown. A discarded suggestion stays in the history, so the group can
  sort out the changes of someone who never opens pigeon.
- A change made through pigeon, `pigeon file write`, `rename` or `delete`
  and the web interface alike, is published at once, whoever owns the
  file. A rename or move keeps the file's history, which `pigeon file
  history` follows back through the paths it moved from.
- `pigeon file restore --pattern /docs/ --time 2026-10-01T12:00:00Z`
  brings files back as they were at a past time, deleted files included,
  as new versions: the history is never rewritten, so a restore is undone
  by another. `pigeon selection times` lists the times of the versions.
- A `.pigeonignore` file, in the gitignore syntax, keeps files out of
  publication. pigeon never publishes them, not even their names.

## 6. Choose what each machine holds

Everyone sees the whole tree, but a machine downloads only what its
selection follows: at first, the member's own folders. Patterns use the
`.pigeonignore` syntax, and the last matching rule wins:

```sh
pigeon selection follow --pattern /docs/
pigeon selection pin --pattern /photos/2024/ --time now
pigeon selection free --pattern /docs/old/
pigeon selection pin --pattern /report/ --time 2026-10-01T12:00:00Z
```

`follow` keeps files in sync, `pin` holds them as they were at a time,
`now` keeping their current version, and `free` removes them from this
machine.

To edit every rule at once, edit the `selection` list of the group's
`config.toml`, one rule per line, the last matching rule winning:

```toml
selection = [
    "follow +mario/",
    "pin 2026-10-01T12:00:00Z /report/",
    "free *.iso",
]
```

Each pin names its time in RFC 3339, so that the file says the same
whenever it is applied; `pigeon selection pin --time now` writes the time
it ran at. `pigeon selection times --pattern /report/` lists the times of
the versions of the files a pattern matches: pinning at each holds
something new.
`pigeon config preview` counts the files and bytes held now and after
applying the file, and what applying it would download, free and pin,
with the rule that decides each file; `pigeon daemon reload` applies it. A
modified copy not yet published is never removed.

The web interface's Overview page holds the same file in an editor. Below
it, a preview updates on each keystroke and as files arrive: the totals now
and after saving, what each rule matches and decides, the largest files
each change concerns, and a warning when your own files would go. A pin's
row offers each of those version times with the number of files, or a date
and time picked in local time, and rewrites its line. Nothing changes until
Save, which asks first when it frees space, applies the file to this group
only, and refuses if the file changed elsewhere since the page loaded it.

A group's page is its Files page: the whole group as one tree whose
folders open and close in place, as `pigeon setup` does, with each folder's size, latest
time and waiting edits; `?under=docs/report` opens it down to a folder.
Each file and folder has a box: checked when followed, mixed when only part
of a folder is. Unchecking asks whether to pin the current copy here, as
it is now, or free the space. A rule set this way replaces the earlier rules for the
paths inside it; hand-written patterns such as `*.pdf` stay. Each row's ⋯
renames, replaces, adds, deletes, pins now or publishes now; each
change asks to be confirmed, then publishes at once.
Each status is one emoji, which a legend under the tree explains: ⏬ on
its way, 📌 pinned copy, ⏳ and 🗑️ an edit or a deletion waiting, ✍️
another member's draft, ⚠️ and 🛑 rival drafts, 📬 a suggestion.
Drafts other members are adding show greyed, with their author and the
time left; when two members add the same path, both are warned, and the
one whose copy will become a suggestion is told to rename it. `pigeon
file pending` lists the same drafts. 📬 marks each suggested change on the
line of its file, greyed when the file does not exist yet, and folders
count them; the Files tab counts the suggestions. A line's menu validates
or discards each suggestion, or validates it at another path, and a
folder's menu validates or discards all those shown under it at once. A
file's page shows the difference each suggestion makes, and its history
with a button that restores each version. The group's pages update
themselves as files change.

`pigeon selection place --folder videos --destination /mnt/big/videos`
keeps a folder on another disk and leaves a link at its place, a junction
on Windows. While the destination is missing, such as an unplugged disk,
the folder waits: nothing in it syncs, and nothing counts as deleted.

Each machine keeps past versions of the files its member writes. It keeps
every version for a day, then one a day for a month and one a week for a
year, within 20% of the disk. The `[retention]` table of `config.toml`
changes this, counting days, and `everything = true` extends it to every
file the machine downloads.

## 7. Run a server

An always-on machine, usually headless, keeps the files available while
their owners' machines are off. It joins as a member of its own:

```sh
pigeon group join --key <the key> --member server
pigeon selection follow --pattern '*'
```

then set `everything = true` under `[retention]` in its `config.toml`, and
run `pigeon daemon reload`.

It owns no folder and nobody edits its disk, so it only receives. Run its daemon with
`pigeon service install --linger`, which starts it at boot. On Linux and
macOS,

```sh
curl -sSf https://raw.githubusercontent.com/mariogeiger/pigeon/main/install.sh | sh -s -- server --key <the key> --member server
```

does all of this. Manage it over SSH:
`ssh server .cargo/bin/pigeon group status`.

## 8. Run the group's relay

Machines connect directly when they can. Otherwise a relay carries their
traffic. It sees only ciphertext. By default these are iroh's public
relays. The group can also run its own relay, used alongside them, on a
machine reachable from the internet on ports 80 and 443 under a domain
name, such as the server:

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

Every machine keeps iroh's public relays too, so a relay that goes down
or is misconfigured never cuts the group apart. `pigeon group status`
shows the relay each machine reached. `pigeon group relay` without
`--url` goes back to iroh's public relays alone.
