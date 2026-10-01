# Changelog

All notable user-visible changes to pigeon are documented here. While the
version is 0.x, a change that breaks compatibility increments the second
number, and any other change the third.

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
