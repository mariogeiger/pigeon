# Changelog

All notable user-visible changes to pigeon are documented here. While the
version is 0.x, a change that breaks compatibility increments the second
number, and any other change the third.

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
