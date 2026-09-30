# pigeon requirements

This file scaffolds what pigeon must become, as agreed before the first line
of code. Expect it to change; [SOUL.md](SOUL.md) holds what does not.

**P0** is the first version, **P1** comes right after, and **Later** stays out
of the first implementation.

## Group and accounts

- **P0** A group is a closed set of people who trust each other: about 20
  members with one or two machines each. Each group has its own root folder,
  such as `/cheapmo`, and nothing in pigeon is specific to one group.
- **P0** Joining takes three fields at first launch: the group key, shared by
  the members, which admits a new machine; a name, of 1 to 32 characters
  among `a`–`z` and `0`–`9`, so that it is valid in any path on every
  system; and a personal password.
- **P0** The member list binds each name to a key, and joining claims a name:
  the earliest claim wins, and a machine that loses asks for another name.
  The password derives the member's signing key, with no server: the same
  name and password on another machine make it one more machine of the same
  member.
- **P0** Members, requests, and decisions are signed statements, stored as
  files in the hidden drop folder `/<group>/.pigeon`, which every machine
  follows. They sync, show up, and keep their history like any other file,
  and nothing ever edits them. A member's notifications are simply the
  statements that concern them.
- **P1** Changing one's password, resetting another member's password,
  excluding a member, and leaving the group all rebind a name, to the key of
  a new password or to none. A member changes their password from any
  machine where they are logged in; any member can reset a password or
  exclude someone from the web UI, without a vote. Everyone sees it, and the
  member is notified.
- **P1** Excluding a member, and leaving, which is excluding oneself, bind the
  name to none and replace the group key: the member list recognizes
  machines, while the group key only admits new ones, so the other machines
  keep working. The name stays taken, the excluded member keeps what they
  downloaded, and their files stay readable but frozen.

## Folders and ownership

- **P0** Every member sees the group's whole tree in the web UI.
- **P0** A folder is only a prefix shared by its files' paths, as in git: it
  exists while it holds a file, and renaming or deleting it renames or
  deletes every file inside, each under its own rule.
- **P0** Every folder follows one rule with two settings: who may add files
  to it, and when its files freeze. Only a file's owner's machines write it,
  and only until it freezes; any other change goes through a request. A
  personal folder lets only its owner add files, and they never freeze; a
  drop folder lets anyone add files, and each freezes once published.
- **P0** Freezing guards against mistakes, not against people: a frozen file
  still changes through a request, which everyone sees and which any
  member, its owner included, may force.
- **P0** A member creates a personal folder by creating, on disk or in the web
  UI, a folder named `@` followed by their name anywhere outside personal
  folders, and may have as many as they like, such as `/cheapmo/src/@mario`
  and `/cheapmo/etc/@mario`. The owner names its subfolders freely.
- **P0** Outside personal folders, a folder `@<name>` claims the name
  `<name>`, as joining does, and the earliest claim wins: once `<name>` is a
  member, only they can create `@<name>`, and while a folder `@<name>`
  exists, nobody can join under `<name>`. A folder `@<name>` whose name no
  member holds, such as npm's `@types`, is an ordinary folder.
- **P0** Every other folder, the root included, is a drop folder.
- **P0** Every file therefore has exactly one owner: the member whose personal
  folder holds it, or the member who dropped it. Only the owner's machines
  write it, so two people never conflict.
- **P0** A change that an owner's machine sees on disk to a file it may write,
  whether an edit, a rename, or a deletion, is published once the file has
  stopped changing: after a few seconds, or after 5 minutes in a drop
  folder, where publishing freezes the file. A deletion deletes the file for
  everyone, and the owner's history keeps its last version.
- **P0** Until it is published, a new file in a drop folder is a draft that
  stays on its author's machine, which can still edit or delete it. A change
  made through an action in the web UI or the CLI, statements included, is
  published at once.

## Changes

- **P0** Every change is a patch: a set of paths, each with its new content
  or nothing, so that one patch adds, replaces, renames, and deletes files
  at once. An owner's machine publishes its disk changes as patches, a
  request is a patch waiting for its owner, and an item set aside is a patch
  that nobody signed.
- **P0** Applying a patch $Q$ after a patch $P$ keeps $Q$'s content wherever
  both touch a path, and this composition is associative. Since each file
  has one writer, patches of different owners touch disjoint paths and
  commute: the tree at a time $t$ is the composition of every patch dated up
  to $t$, and depends only on each owner's own order.
- **P0** Every patch carries a hybrid logical clock timestamp, from the
  `uhlc` crate: its machine's time, never behind a timestamp that machine
  has received, with the machine's key breaking ties. Timestamps thus order
  all patches totally, and never place a patch before one its machine had
  already seen.
- **P0** A patch also records the versions it replaces, which tells whether
  two changes to a file are concurrent, made without seeing each other.
  Between machines of one member, the later of two concurrent changes wins
  and the other is set aside; a proposal concurrent with a change is marked
  as based on an old version.
- **P0** A member name, a folder `@<name>`, and a path in a drop folder are
  claims, compared without case, and the earliest claim wins: the losing
  machine sets its content aside, or asks for another name.

## Set aside

- **P0** Whatever the disk holds that pigeon may not publish as it is goes to
  one local list: an edit to a file one cannot write, a name that is not
  portable, a lost claim, and the losing side of concurrent changes by one
  member's machines. Nothing set aside is ever sent. Where the disk must show
  someone else's published version, pigeon restores it and keeps the other
  content in the list.
- **P0** For each item, the web UI offers to turn it into a request, drop it
  as a new file, rename it, or discard it. Turning an item into a request
  only signs its patch.
- **P0** Files a member cannot write are read-only on their disk, so that
  most edits never reach the list.

## Requests

- **P0** A request is a patch on files of one owner that the requester cannot
  write, like a pull request. It either proposes the patch, which the owner
  accepts or refuses as a whole, or forces it, which needs no acceptance. An
  action whose patch touches several owners' files makes one request per
  owner, since a patch is atomic only within one writer.
- **P0** Requests are made only from the web UI or the CLI, for instance by
  selecting a file and proposing a replacement. Editing a file on disk never
  creates one.
- **P0** A machine of the file's owner applies every request: a proposal once
  accepted, a forced request as soon as that machine is online. The owner
  stays the only writer, so requests never conflict; a forced request waits
  until one of the owner's machines is on.
- **P0** Every member may force. Every request, proposed or forced, is visible
  to everyone with its author and date, the owner is notified, and the
  owner's history keeps the replaced version, so a forced change can always
  be undone.
- **P0** The web UI shows the differences for text files, and marks a
  proposal based on an old version.

## Selection

- **P0** Each machine holds what its selection says: a list of rules, each
  mapping a pattern in the gitignore syntax of `.pigeonignore` to a time
  $t$, where the last matching rule wins. For each file, the machine holds
  its last version dated up to $t$. Following is $t = +\infty$, which keeps
  the file in sync; excluding is $t = -\infty$; pinning is any date that
  some machine's history still covers, which brings back an earlier state
  of any part of the tree.
- **P0** Every action on local copies sets $t$ for a pattern. Subscribing sets
  $+\infty$; a one-time download, or refreshing one, sets the current time;
  unsubscribing lowers $+\infty$ to the current time, or to $-\infty$ to
  free the space; deleting on disk a file one cannot write sets $-\infty$.
- **P0** The web UI marks a file as outdated when a newer version exists than
  the one held.
- **P0** An owner keeps files out of publication with a `.pigeonignore` file,
  read with the `ignore` crate. Ignored files are never published, not even
  their names; publishing and receiving thus share one rule language.
- **P0** On disk, a group is a normal folder holding only downloaded content,
  with no placeholder files.

## Paths

- **P0** Each group has a local root folder, and the selection downloads into
  it at each file's place in the tree.
- **P1** The root has the same path on every machine: `/cheapmo` on Linux and
  macOS, created once with admin rights (on macOS through
  `/etc/synthetic.conf`), and `C:\cheapmo` on Windows, where native programs
  running on drive C: also resolve `/cheapmo/...`.
- **P1** A selection rule can place a folder at another destination. pigeon
  then leaves a link at the folder's place in the root, a junction on
  Windows, which needs no admin rights. Destinations never nest, so
  everything downloaded keeps the same path on every machine.

## History

- **P0** Every machine keeps the history of the files it writes, as the
  sequence of patches applied to them.
- **P0** Default retention, adjustable on each machine: every version for 24
  hours, then the last version of each day for 30 days, then the last version
  of each week for a year; the last version before a deletion for a year;
  identical content stored once. A quota, 20% of the disk by default, prunes
  the oldest versions first and overrides every rule above. Pruning composes
  the patches between the versions it keeps, so every state it keeps stays
  exact.
- **P1** "Keep history" extends it to everything the machine downloads. The
  group's "server" is nothing special: an always-on machine, usually
  headless, that joins as a member of its own, such as `server`, follows
  everything, and keeps history. It owns no personal folder, so it gains no
  right to write anyone's files, and it serves files while their owners'
  machines are off.

## Network

- **P0** Machines connect peer to peer, on the local network and over the
  internet, with iroh. A machine is reached by its public key, traffic is
  end-to-end encrypted, NAT traversal connects machines directly, and when it
  fails a relay that only sees ciphertext carries the traffic: one of iroh's
  public relays, or one run on the group's server. Machines on the same local
  network find each other without help.
- **P0** Content moves in hash-verified chunks (BLAKE3, iroh-blobs), so an
  interrupted transfer resumes where it stopped.
- **P1** Any machine serves the chunks it holds, even while it is still
  downloading, and a machine fetches chunks from several peers at once,
  fastest first: local network, then direct connections, then relays.
  Sending a file of size $F$ to $N$ machines then approaches the minimum
  possible time (Kumar and Ross, 2006):

  $$T = \max\left(\frac{F}{u_s}, \frac{F}{d_{\min}}, \frac{N F}{u_s + \sum_i u_i}\right)$$

  where $u_s$ is the source's upload rate, $u_i$ each peer's, and $d_{\min}$
  the smallest download rate.

## Platforms and interfaces

- **P0** Windows, Linux, and macOS, on Intel and Apple Silicon.
- **P0** One Rust program per machine synchronizes, answers a JSON API, and
  serves the web UI with pages generated in Rust, all on localhost only: a
  single binary.
- **P0** The API answers only calls carrying the user's secret token, which
  the CLI reads from the user's configuration and the web UI receives in the
  link that opens it. Otherwise any website the user visits could call
  localhost and act in their name.
- **P0** A CLI on that API, with the ergonomics of `gh`: `pigeon <noun> <verb>`
  commands, prompts only when a terminal is attached and an argument is
  missing, a flag for every prompt so that scripts never block, `--json`
  output, errors that name the command to run next, and shell completions.
  The CLI, over SSH, manages headless machines such as the server.
- **P0** Each action is defined once, with its name, arguments, and effect,
  and the CLI commands, the JSON API, and the web UI forms are generated from
  that definition. The web UI and the CLI therefore offer exactly the same
  actions: whatever this file says the web UI does, the CLI does too.
- **Later** iOS, and a WebAssembly build that runs pigeon in a browser
  without installation.

## Scale

- **P0** Up to 1 TB per group, files up to 100 GB, and one million files.
- **P0** A file whose size and modification time have not changed is never
  hashed again.

## Portable names

- **P0** A machine publishes only names valid on all three systems: no
  character Windows forbids (`< > : " \ | ? *` and control characters), no
  reserved name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`),
  no trailing space or dot, and no two names in one folder that differ only
  by case. Names are normalized to Unicode NFC. A file that breaks these
  rules is set aside.

## Non-goals

- Protecting against a malicious member.
- Merging two versions of a file: a request replaces the whole file.
- Sharing with only part of the group.
- Guaranteed erasure: whatever was shared may have been copied.
- Reliable backup: the quota may prune history.
- Syncing symbolic links, empty folders, Unix permissions, or extended
  attributes, apart from the executable bit.
- Placeholder files and a mobile app, in the first version.
