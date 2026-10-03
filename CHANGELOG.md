# Changelog

All notable user-visible changes to pigeon are documented here. While the
version is 0.x, a change that breaks compatibility increments the second
number, and any other change the third.

## 0.9.0 — 2026-10-03

Every machine of a group must update to 0.9.0 together: sessions now
speak `pigeon/sync/10`, which earlier versions do not, so a machine left
on 0.8 syncs with none of the others until it updates.

### Changed

- A deletion is published only when pigeon sees the file gone, in a
  folder it could read, under a root that holds its `.pigeon` folder. An
  unreadable folder, an unplugged disk or any other error publishes
  nothing and is reported.
- A group whose root or `.pigeon` folder goes missing after files were
  synced there pauses: it keeps receiving and serving, says why on a
  `Paused:` line of `pigeon group status` and of the Overview, and
  resumes on its own when the folder returns. Moving a group to another
  root resumes there when that folder holds the group's `.pigeon`
  folder, starts afresh like a join when it is empty or missing, leaving
  the old root as it was, and is refused when it holds other files.
- A name some machine cannot hold (decomposed Unicode, a character or
  name Windows forbids, a trailing dot or space, two names differing only
  by case) is kept out of the group, instead of being suggested to the
  whole group. The machine holding it lists it with the free portable
  name closest to it, which one action renames it to on disk. Renaming
  only the case of a name is published as a move.
- A path `.pigeonignore` excludes is never read, published, written or
  deleted on this machine, whether the group knows it or not.
- Deleting, restoring, validating, discarding and leaving ask one
  question, the same on the command line, the API and the web UI, and
  `--yes` answers it, so scripts that delete or restore files now pass
  `--yes`. Leaving says when this machine holds the only copy of the
  group's history.
- `pigeon update` and the install script install the newest `vX.Y.Z`
  release instead of the head of main, in the place of the program the
  daemon runs. `--path` still builds from a local checkout.
- The daemon restarts five seconds after it fails, however many times.
  A release build aborts on a panic, so that the service sees the crash,
  and a detached daemon appends to `daemon.log` instead of truncating it.
- The API takes its token only from the `Authorization` header, never
  from the cookie a browser attaches, and a request that changes
  something is refused when it comes from a page of another site.
- `pigeon status` and the Overview answer without waiting for the work
  in progress, and the command line gives up on a daemon that does not
  connect.
- A group opens on its Files page, at `/g/{group}`, with the Overview at
  `/g/{group}/overview`; the Files page lists no statement under
  `.pigeon`.
- The relay the group names is used alongside iroh's public relays, so
  that one relay going down never cuts the group apart.
- Edits that settle together are published in patches of at most a
  megabyte of changes, each move kept whole.
- pigeon is built with the Rust that `rust-toolchain.toml` names, and
  its tests run on Linux, macOS and Windows on every push.

### Added

- `pigeon file unportable` lists the names kept out of the group, and
  `pigeon file make-portable` renames one to its proposed portable name,
  as the button above the Files page's tree does.
- `pigeon group serve` makes this machine a server of its group: it
  follows everything and keeps every version. The setup, the install
  script and the Overview page use it.
- `pigeon update --rollback` puts back the program the last update
  replaced, kept as `pigeon.previous`.
- The status says why each machine could not be synced with, until a
  session opens, and when this machine's clock lags the group's.

### Fixed

- Two machines that missed each other's patches find the gap: each
  session's hello, and every minute after, compares a digest of each
  machine's patches and sends all of a machine's patches when they
  differ.
- A patch is dated after every time the clock stamped or observed, across
  restarts and when the system clock is set back, and a machine refuses
  loudly the patches a copy of its own key made elsewhere.
- Each batch of patches received is stored in one transaction before
  the ledger takes it, and refused whole when storing fails.
- Files, the configuration and the group's secrets are written whole and
  flushed to the disk under a name no other write shares, so a crash
  leaves the old content or the new, never a torn one.
- A file is written, moved or removed only while the disk shows what
  pigeon last saw there, so an edit made meanwhile is never overwritten,
  and a file is published only if it did not change while copied.
- Files are hashed by chunks without mapping them, so a file truncated
  while hashed no longer crashes the daemon; an edit that keeps the size
  and lands within the timestamp's precision is seen; a replaced file is
  told by its inode.
- Temporary files a stopped run left are removed when the group starts.
- A change of this machine that lost is suggested once, whatever its
  disk holds, and a suggestion the group decided never comes back.
- A selection pattern matches however its names are spelled in Unicode,
  and a place whose destination nests with the root or another
  destination is refused where the system resolves them, from the API
  and the config file alike.
- Walking the disk, hashing and moving placed folders no longer block
  the daemon's other work, and a burst of more than 256 changes is read
  as one rescan.
- The handshake is bounded and timed, a machine that cannot be reached
  is dialed less and less often, a fetch that stops growing fails, and a
  blob that failed waits longer before it is fetched again.
- `pigeon reload` reloads the groups whose configuration reads even when
  another's does not, and a group that does not start can be mended with
  `pigeon config show` and `pigeon config set`.
- Uploads of any size are taken: the API, the web forms and `pigeon file
  write --content` stream a file's content to the disk as it arrives
  instead of holding it in memory, downloads are sent in chunks, and a
  version too large to compare is left unread.
- A version garbage collection takes while it is read or downloaded
  reads as no longer held on this machine, which the web answers with a
  404, instead of failing, and a download under way keeps its content
  until it ends.
- Stopping a group ends its blob fetches and every blob it serves, then
  stops its blob store whole, so it can start again at once and a request
  arriving during the shutdown no longer aborts the daemon.
- A blob is kept from garbage collection from the start of its fetch
  until the file it makes is on the disk, so a collection running
  meanwhile no longer takes a file fetched but not yet written.
- A file only opened or read no longer wakes a rescan, which made
  pigeon's own scans wake one another without end on Linux.
- A burst of changes is read in one look, each folder listed once, while
  the engine goes on with its other work, so a burst of files no longer
  lands at about five a second nor costs a listing per file.
- pigeon watches with notify 9, whose Windows watcher tells when it lost
  events, and rescans the whole root whenever the system lost track of
  what changed. A walk waits until the watcher watches every folder it
  reported new, so each file made in a new folder is seen, in the walk or
  as a change.
- The patches a machine receives are stored and passed on as they come,
  however long its disk work takes, so a machine busy writing a large
  change no longer holds back the group's patches, nor the machines it
  relays them to.
- pigeon waits for the disk once for each file it writes, half as often
  as before, and never to note what its index already says, so a scan
  that finds nothing new waits for no disk.
- A fetch keeps asking a machine that holds nothing of the blob yet,
  giving up only once no machine is left or the fetch stalls, and an
  error of the blob store tells each cause under it.

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
- A group that does not start is no longer taken for absent: `pigeon
  group list` and the web's list of groups show it with why, and every
  call naming it tells why, then how to mend it or take it off the
  machine.
- `pigeon daemon reload` and `pigeon config set` ask the web's question
  before edits free space on this machine, and apply them on yes;
  without a terminal they refuse unless given `--yes`.
- A group whose new `config.toml` does not start runs on with the
  configuration it had, which pigeon writes back, and the error says so.

### Removed

- `pigeon selection download` and `unfollow`, and the Files menu's
  Download: `pin --time now` keeps a copy as it is now, and `free` frees
  it.
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
