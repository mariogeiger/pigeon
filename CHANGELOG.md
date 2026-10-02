# Changelog

All notable user-visible changes to pigeon are documented here. While the
version is 0.x, a change that breaks compatibility increments the second
number, and any other change the third.

## 0.8.0 — 2026-10-02

### Changed

- Leaving a group is local: `pigeon group leave` and the Overview's Leave
  stop this machine syncing and forget the group's secrets and state,
  keeping its files, and publish nothing, so the member's name stays in
  the group. Whoever knows the group key is admitted.
- The sync protocol is `pigeon/sync/9`, which 0.7 machines cannot join:
  every machine of a group must update.
- pigeon no longer sets the permissions of the files it writes: a new file
  takes those of the umask, executable where readable when its content
  executes, and a file it replaces keeps its own, only whether it executes
  changing.
- Selection patterns and paths match regardless of case, as the names of
  files already did.
- The selection changes through the verbs of `config.toml`: `pigeon
  selection follow`, `pin --time <RFC 3339 time|now>` and `free`. The Files
  menu's Pin pins a file or folder as it is now, and stopping following
  asks whether to pin now or free the space. `pigeon file list` and `file
  pending` show how the selection takes each file as its line would:
  `follow`, `pin <time>` or `free`.
- A copy kept at a time is a pin wherever pigeon speaks of it: `pigeon
  config preview` and `daemon reload` show a `pin` column in place of
  `freeze`, the Files page 📌 pinned in place of 🧊 frozen, and `pigeon
  setup` asks whether to keep a pinned copy.
- Why a suggestion waits, what each of its changes is and how another
  machine's pigeon stands read the same on the command line as on the
  web, as sentences, and `pigeon suggestion list` names a suggestion by
  its `id` alone.
- `pigeon daemon reload` and `pigeon config set` ask the web's question
  before edits free space on this machine, and apply them on yes;
  without a terminal they refuse unless given `--yes`.
- A group whose new `config.toml` does not start runs on with the
  configuration it had, which pigeon writes back, and the error says so.

### Removed

- Excluding a member: `pigeon member exclude`, the Overview's Exclude
  and the renewal of the group key it made. The exclusions in a group's
  history are ignored.
- The upgrades of the files and states of pigeon 0.6 and older: update to
  0.7 and run it once first.
- The certificate `secrets.toml` kept for members of groups created
  before 0.2.3, whose key does not derive from their name: such a group
  must be created again. A group whose `secrets.toml` holds fields of
  older pigeons does not start, and the error names the field; `pigeon
  group leave` still takes it off the machine.

### Fixed

- A machine catches up on a history of any size: the patches it lacks
  come in as many messages as keep each within the bound, where a single
  message over 64 MiB was refused on every retry.
- A version a machine makes at a path its member neither owns nor
  follows, such as an action in another member's folder, keeps its
  content until another machine fetches it: garbage collection could
  delete it first, so that the owner never received it.

## 0.7.3 — 2026-10-02

### Added

- `pigeon setup`, which the install script runs, offers to turn on Tab
  completion: it adds the line that loads it to `~/.zshrc`, `~/.bashrc`
  or fish's `config.fish`, whichever your login shell reads.

## 0.7.2 — 2026-10-02

### Added

- Tab completion in zsh, bash, fish, elvish and PowerShell completes the
  values the daemon knows: groups, members, the group's paths and
  patterns one folder at a time, version times and suggestion ids, each
  with what `list` shows of it. `pigeon completions <shell>` prints the
  script to source.

### Changed

- `pigeon setup`, the help and the docs say "create a group" rather than
  "found a group".

## 0.7.1 — 2026-10-02

### Changed

- The README animation shows suggestions: an edit of someone else's file
  that anyone validates, and a deletion that another member discards.

## 0.7.0 — 2026-10-02

### Changed

- Every member may change every file. A machine publishes by itself only
  its member's changes in their own `+name` paths, and new files at paths
  no one owns once they have not changed for five minutes; every other
  change made on disk, a deletion included, becomes a suggestion that the
  whole group sees and anyone validates or discards. The disk that made
  it keeps it until then, and a discard brings back the group's version.
- Files are no longer read-only on disk, and a published file of no
  `+name` path has no owner: pigeon shows who made it.
- When two machines change a file while apart, the later change wins, and
  the other becomes a suggestion that its disk keeps.
- `pigeon file write`, `rename` and `delete`, and the web interface's
  actions, publish at once, whoever owns the file, after a confirmation
  in the web interface; `--mode` and `--message` are gone.
- A rename or move, through pigeon or on disk, keeps the file's history,
  which `pigeon file history` follows back through the paths it moved
  from.
- The sync protocol is `pigeon/sync/8`, which 0.6 machines cannot join.
  The patches of 0.6 fold again under the new rules, where every one is
  accepted, and the requests, decisions and set-aside files of 0.6 are
  ignored; what a machine had set aside comes back as its suggestions.

### Added

- `pigeon suggestion list`, `validate` and `discard`, which decide
  several suggestions at once by the ids the list shows, each as it was
  listed, so the first decision is final; `validate --to` publishes the
  one file of a single suggestion at another path, as a name Windows
  cannot hold needs.
- `pigeon file restore --pattern --time` brings files back as they were
  at a past time as new versions, and a file's page restores any version
  of its history in one click.
- The Files page's menus validate or discard each suggestion, or all
  those under a folder at once, and the machines that follow a path fetch
  what is suggested there, so a file's page shows the difference.

### Removed

- `pigeon change list`, `apply`, `ask`, `place` and `discard`, with the
  requests addressed to an owner they decided: suggestions replace them.

## 0.6.0 — 2026-10-01

### Changed

- The Changes page is gone: the Files page shows, with 📬 on the line of
  its file, every change waiting for someone, one per file, a request or
  what a machine set aside, greyed when the file does not exist yet, and
  folders count them. A line's menu applies the change, asks its owner,
  places it at another path or discards it, and the file's page shows the
  difference it makes. The Files tab counts the proposals addressed to you
  and what your machines set aside.
- `pigeon change list`, `apply`, `ask`, `place` and `discard` replace
  `pigeon request` and `pigeon aside`, each change named by its `--entry`.
- A request holds one change, so each file is decided on its own, and
  anyone may accept or refuse any proposal: the first decision is final.
- Every change made through pigeon, from the web interface or `pigeon file
  write`, `rename` and `delete`, becomes a request, which the owner's
  machine applies at once when the owner made it. Each dialog asks only to
  confirm a change that is all yours, and otherwise whether to apply it
  now or ask the owners first.

## 0.5.0 — 2026-10-01

### Changed

- Set-aside items are shown to the whole group, and anyone may resolve any
  of them from any machine: each machine publishes each item it sets aside
  as a file in `.pigeon/aside/`, and `pigeon aside list` lists every
  machine's items with their member. `pigeon aside discard`, `restore` and
  `request` take the item's `--file` instead of its number, and ask the
  item's member, whose machines carry them out unattended; a restored file
  belongs to the item's member unless its path names its owner. The
  Changes page counts the items of your machines and lists those of others
  under "Set aside by others".

### Fixed

- A file created outside the selection, such as a new drop file, is no
  longer deleted from the disk when the engine starts or the selection
  changes: only what a change of the selection unselects is freed. A drop
  that lost its name to another member's while apart thus stays set aside
  with its content.
- A file whose name not every system can hold, once restored under a
  portable name, leaves the disk and is no longer set aside again at the
  next start.
- Garbage collection waits until the engine first computed which blobs to
  keep: right after a start it could delete the content of a file
  published while offline, which no other machine could then fetch.

## 0.4.0 — 2026-10-01

### Changed

- A pin in `config.toml` names its time in RFC 3339: `pin now` no longer
  reads, so that the file says the same whenever it is applied. The config
  editor's "now" writes the present time into the line instead.

## 0.3.1 — 2026-10-01

### Added

- A selection line of the config editor that pins without a valid time,
  such as `"pin "` or `"pin /docs/"`, gets under its error a menu of the
  times its files have versions at, now, or a local date and time, which
  write the time into the line.

## 0.3.0 — 2026-10-01

### Changed

- The web interface's bar reads 🐦 pigeon › group, then the group's tabs:
  Overview, Files and Changes, which counts what waits for you. Pages show a
  🐦 icon.
- The Overview page holds the group's status in one line, its errors and
  incompatible machines if any, the members with their machines and who is
  online, to exclude one or leave, the group key with a Copy button, the
  places, and the raw ids folded under Details.
- `config.toml` is how one changes the selection and the retention: the
  Overview page edits it, highlighted, with a live preview of what saving
  would download, free and freeze, rule by rule, and lets a pin pick one of
  its files' version times or any local time. Save asks first when it frees
  space, refuses if the file changed elsewhere, and applies it to this group
  only. `pigeon config show`, `preview` and `set` do the same on the
  command line.
- `pigeon daemon reload` tells what the edits download, free and freeze on
  each group, and asks first when they free space; `--yes` skips the
  question, and without a terminal it refuses unless given.
- The Changes page merges the requests and the set-aside list, one line per
  change by who must act: to you, here but not sent, waiting for others,
  and the requests done on demand; each difference opens on click.
- `pigeon member list` shows each member's machines and those online.

### Removed

- The Selection, Retention, Members, Requests and Set aside pages, now part
  of Overview and Changes.
- `pigeon selection list`, `set` and `preview`, and `pigeon retention show`
  and `set`: edit `config.toml` instead.

## 0.2.5 — 2026-10-01

### Changed

- On Linux, pigeon follows the XDG base directories: each group's
  `config.toml` lives in `~/.config/pigeon/groups/<group>/`, its secrets,
  state database and blobs stay in `~/.local/share/pigeon/groups/<group>/`,
  and `daemon.toml`, `daemon.log` and the relay's certificates live in
  `~/.local/state/pigeon`. Files in the older places move there once. On
  macOS and Windows, and under `$PIGEON_HOME`, one folder still holds
  everything.

## 0.2.4 — 2026-10-01

### Fixed

- A machine of a group founded before member keys derived from names
  speaks again for its member: upgrading keeps its certificate in
  `secrets.toml` rather than deriving one that the group does not know,
  which made 0.2.3 see the member's name as taken.

## 0.2.3 — 2026-10-01

### Changed

- Everything lives in one pigeon folder, by default in the local data
  folder, which on Windows is `%LOCALAPPDATA%` rather than `%APPDATA%`: the
  older folder moves there.
- `daemon.toml` holds the API token and the daemon's address, in place of
  the files `token` and `address`.
- Each group keeps its member, root, selection, retention and places in
  `config.toml`, readable and editable by hand, and its group key and
  machine key in `secrets.toml`, in place of `config.json`, `machine.key`
  and the settings of the state database. The daemon upgrades the older
  files once, at start.

### Added

- `pigeon daemon reload` restarts every group from its files, applying the
  edits of each `config.toml`, and changes nothing if one does not read.
  pigeon rewrites the file whole, without comments, when it changes a
  setting, and refuses to while it holds edits not yet reloaded.

## 0.2.2 — 2026-10-01

### Changed

- The README animation lasts 40 seconds and shows pigeon with almost no
  words.

## 0.2.1 — 2026-10-01

### Added

- The README opens with an animation of how pigeon works.

## 0.2.0 — 2026-10-01

### Changed

- The sync protocol is now `pigeon/sync/7`: a machine on 0.1 cannot sync
  with one on 0.2, so every machine of a group must update.
- The Files page shows the whole group as one tree whose folders open and
  close in place, with each folder's size, latest time and waiting edits,
  Expand all and Collapse all, and `?under=` opening it down to a folder.
  Each row's ⋯ renames, replaces, adds, deletes, downloads once or
  publishes now, and turns a change one may not write into a request.
- Each status of the Files page is one emoji, explained on hover and by a
  legend under the tree, and only what departs from being in sync shows.
- The tree of `pigeon setup` is the one the Files page draws. It follows
  and unfollows the member's own files like any other, and asks once
  whether unchecked files on this machine keep a frozen copy or free the
  space.
- The selection preview shows below the rules rather than beside them.

### Added

- A machine announces its drafts in drop folders to the others, signed,
  with their size and the time left before they are published. Others'
  drafts show greyed in the Files page and in `pigeon file pending`, with
  their author. When two members add the same path, both are warned, and
  the one published later learns its copy will be set aside unless renamed.
- A draft not yet published can be renamed or deleted through an action.
- The overview names the member and version of each machine that speaks
  another protocol, which every machine now tells over `pigeon/hello`.

### Fixed

- A machine reported as incompatible is no longer once it updated and
  opened a session.
- A draft deleted before it was published no longer stays listed as
  waiting.

## 0.1.0 — 2026-10-01

### Added

- The first version: the daemon that syncs every group peer to peer, its
  localhost JSON API, web UI and command line, all generated from one
  catalog of actions; member names, personal `+<member>` folders and drop
  folders; requests, set-aside edits, a selection that follows, pins or
  frees files, history within a disk quota, folders placed at other
  destinations, a group's own relay; `install.sh`, `pigeon setup` and
  `pigeon update`.
