# Changelog

All notable user-visible changes to pigeon are documented here. While the
version is 0.x, a change that breaks compatibility increments the second
number, and any other change the third.

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
