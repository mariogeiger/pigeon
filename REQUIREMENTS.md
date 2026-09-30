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
  the members, which admits a new machine; a name; and a personal password.
- **P0** The first machine to claim a name reserves it; when two newcomers
  claim the same name at once, the earlier claim wins and the other machine
  asks for another name. The password derives the member's signing key, with
  no server: the same name and password on another machine make it one more
  machine of the same member.
- **P0** Members, requests, and decisions are signed statements, stored as
  files in the hidden drop folder `/<group>/.pigeon`, which every machine
  follows. They sync, show up, and keep their history like any other file,
  and nothing ever edits them; pigeon publishes them at once, without the
  5-minute delay of drop folders.
- **P1** A member changes their password from any machine where they are
  logged in. Any member can reset another member's password from the web UI,
  without a vote; everyone sees it, and the member is notified.
- **P1** The member list, mapping names to keys, recognizes machines; the
  group key only admits new ones. Excluding a member removes them from the
  list and replaces the group key; any member can do it, everyone sees it,
  and the other machines keep working. The excluded member keeps what they
  downloaded, and their folders stay readable but frozen. Leaving the group
  is excluding oneself.

## Folders and ownership

- **P0** Every member sees the group's whole tree in the web UI.
- **P0** A member creates a personal folder by creating, on disk or in the web
  UI, a folder named `@` followed by their name anywhere outside personal
  folders, and may have as many as they like, such as `/cheapmo/src/@mario`
  and `/cheapmo/etc/@mario`. Only the owner's machines write inside it, and
  the owner names its subfolders freely.
- **P0** Outside personal folders, only the member `<name>` can create a
  folder named `@<name>`, and nobody can join under a name while a folder
  `@<name>` exists. Other names starting with `@`, such as npm's `@types`,
  are ordinary folders.
- **P0** Every other folder, the root included, is a drop folder: anyone can
  add files and folders to it, and nothing in it changes once received, not
  even for its author, except through a request. This holds for the folders
  themselves: once received, a folder is renamed or deleted only through a
  request.
- **P0** Every file therefore has exactly one owner: the member whose personal
  folder holds it, or the member who dropped it. Only the owner's machines
  write it, so two people never conflict.
- **P0** When two machines of one member change the same file before
  syncing, the later change wins and the other is set aside.
- **P0** A new file in a drop folder is a draft: it stays on its author's
  machine, which can still edit or delete it. It is sent after 5 minutes
  without modification, and any modification before another member has
  received it entirely restarts this cycle.
- **P0** Once another member has received it entirely, the file is frozen,
  and a later modification on disk is set aside.
- **P0** When two members drop the same name at once, both files are kept and
  one is renamed.

## Set aside

- **P0** Whatever the disk holds that pigeon may not publish as it is goes to
  one local list: an edit to a file one cannot write, a name that is not
  portable, a folder `@<name>` created outside personal folders by anyone
  but `<name>`, and the losing side of a conflict between one member's
  machines. Nothing set aside is ever sent. Where the disk must show someone
  else's published version, pigeon restores it and keeps the other content
  in the list.
- **P0** For each item, the web UI offers to turn it into a request, drop it
  as a new file, rename it, or discard it.
- **P0** Files a member cannot write are read-only on their disk, so that
  most edits never reach the list.

## Requests

- **P0** A request replaces, renames, or deletes a file that the requester
  cannot write. It either proposes the change, which the owner accepts or
  refuses, or forces it, which needs no acceptance.
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
  proposal as based on an old version when the file changed since.

## Selection

- **P0** Each machine holds what its selection says: a list of rules, each a
  pattern in the gitignore syntax of `.pigeonignore` with a mode, where the
  last matching rule wins. A rule follows its paths, downloading them and
  keeping them in sync; freezes them at a date, keeping the versions of that
  moment; or excludes them.
- **P0** Every action on local copies edits the selection. Subscribing adds a
  follow rule, and a one-time download adds a freeze at the current date.
  Unsubscribing turns a follow rule into a freeze, or into an exclusion to
  free the space. Deleting on disk a file one cannot write adds an exclusion,
  and refreshing moves a freeze to the current date.
- **P0** The web UI marks a frozen file as outdated once a newer version
  exists.
- **P0** Deleting a file in one's own personal folder deletes it for everyone,
  and the owner's history keeps its last version.
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

- **P0** Every machine keeps the history of the files it writes.
- **P0** Default retention, adjustable on each machine: every version for 24
  hours, then the last version of each day for 30 days, then the last version
  of each week for a year; the last version before a deletion for a year;
  identical content stored once. A quota, 20% of the disk by default, prunes
  the oldest versions first and overrides every rule above.
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
- **P0** The web UI and the CLI offer exactly the same actions: whatever this
  file says the web UI does, the CLI does too.
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
- Syncing symbolic links, Unix permissions, or extended attributes, apart
  from the executable bit.
- Placeholder files and a mobile app, in the first version.
