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
- **P0** The first machine to claim a name reserves it. The password derives
  the member's signing key, with no server: the same name and password on
  another machine make it one more machine of the same member.
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
  UI, a folder named after themselves anywhere outside personal folders, and
  may have as many as they like, such as `/cheapmo/src/mario` and
  `/cheapmo/etc/mario`. Only the owner's machines write inside it, and the
  owner names its subfolders freely. If two of those machines change the same
  file before syncing, both versions are kept and the member chooses.
- **P0** Outside personal folders, a member's name is reserved for that
  member's personal folders: nobody else can create a folder with that name,
  and a new member cannot choose a name that a folder already bears. A folder
  named after a member therefore always belongs to them.
- **P0** Every other folder, the root included, is a drop folder: anyone can
  add files and folders to it, and nothing in it changes once received, not
  even for its author, except through a request. This holds for the folders
  themselves: once received, a folder is renamed or deleted only through a
  request.
- **P0** Every file therefore has exactly one owner: the member whose personal
  folder holds it, or the member who dropped it. Only the owner's machines
  write it, so two people never conflict.
- **P0** A new file in a drop folder is a draft: it stays on its author's
  machine, which can still edit or delete it. It is sent after 5 minutes
  without modification, and any modification before another member has
  received it entirely restarts this cycle.
- **P0** Once another member has received it entirely, the file is frozen. A
  later modification on disk never propagates: pigeon restores the original
  and keeps the modified content as a new draft named `name (2)`.
- **P0** When two members drop the same name at once, both files are kept and
  one is renamed.

## Requests

- **P0** A request replaces, renames, or deletes a file that the requester
  cannot write. It either proposes the change, which the owner accepts or
  refuses, or forces it, which needs no acceptance.
- **P0** Requests are made only from the web UI, for instance by selecting a
  file and proposing a replacement. Editing a file on disk never creates one.
- **P0** A machine of the file's owner applies every request: a proposal once
  accepted, a forced request as soon as that machine is online. The owner
  stays the only writer, so requests never conflict; a forced request waits
  until one of the owner's machines is on.
- **P0** Every member may force. Every request, proposed or forced, is visible
  to everyone with its author and date; the owner is notified; and the
  owner's machine keeps the replaced version, so a forced change can always
  be undone, even without a history machine.
- **P0** The web UI shows the differences for text files, and marks a
  proposal as based on an old version when the file changed since.
- **P0** Files a member cannot write are read-only on their disk. If they edit
  one anyway in someone's personal folder, pigeon restores the original, sets
  their version aside without sending anything, and the web UI offers to turn
  it into a request. In a drop folder, the `name (2)` rule applies.

## Subscriptions and local copies

- **P0** A machine downloads only what it asks for: subscribed folders are
  downloaded and kept in sync, and any file or folder can be downloaded once
  from the web UI.
- **P0** A one-time copy keeps the downloaded version. The web UI marks it
  outdated once a newer version exists, and one click refreshes it or turns
  it into a subscription.
- **P0** Unsubscribing from a folder turns its downloaded files into one-time
  copies, and offers to delete them to free the space.
- **P0** Deleting on disk a file one cannot write unsubscribes from that file:
  it is not downloaded again, and the web UI shows it as excluded, with a way
  to bring it back. Deleting a file in one's own personal folder deletes it
  for everyone, and machines that keep history keep its last version.
- **P0** An owner keeps files out of publication with a `.pigeonignore` file
  in gitignore syntax, read with the `ignore` crate. Ignored files are never
  published, not even their names.
- **P0** On disk, a group is a normal folder holding only downloaded content,
  with no placeholder files.

## Paths

- **P0** Each group has a local root folder, and subscriptions download into
  it at their place in the tree.
- **P1** The root has the same path on every machine: `/cheapmo` on Linux and
  macOS, created once with admin rights (on macOS through
  `/etc/synthetic.conf`), and `C:\cheapmo` on Windows, where native programs
  running on drive C: also resolve `/cheapmo/...`.
- **P1** The web UI can send any subscribed folder to another destination.
  pigeon then leaves a link at the folder's place in the root, a junction on
  Windows, which needs no admin rights. Destinations never nest, so
  everything downloaded keeps the same path on every machine.

## History

- **P1** Any machine can keep history. The group's "server" is nothing
  special: an always-on machine, usually headless, that joins as a member of
  its own, such as `server`, and subscribes to everything with history on. It
  owns no personal folder, so it gains no right to write anyone's files, and
  it serves files while their owners' machines are off.
- **P1** Default retention, adjustable on each machine: every version for 24
  hours, then the last version of each day for 30 days, then the last version
  of each week for a year; the last version before a deletion for a year;
  identical content stored once. A quota, 20% of the disk by default, prunes
  the oldest versions first and overrides every rule above.

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
- **P0** A CLI on that API, with the ergonomics of `gh`: `pigeon <noun> <verb>`
  commands, prompts only when a terminal is attached and an argument is
  missing, a flag for every prompt so that scripts never block, `--json`
  output, errors that name the command to run next, and shell completions.
  The CLI, over SSH, manages headless machines such as the server.
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
  rules stays on its machine, and the web UI asks its owner to rename it.

## Non-goals

- Protecting against a malicious member.
- Merging two versions of a file: a request replaces the whole file.
- Sharing with only part of the group.
- Guaranteed erasure: whatever was shared may have been copied.
- Reliable backup: the quota may prune history.
- Syncing symbolic links, Unix permissions, or extended attributes, apart
  from the executable bit.
- Placeholder files and a mobile app, in the first version.
