# pigeon

pigeon keeps a group's files in sync, peer to peer: every member owns folders
the whole group can see, and changes to someone else's files go through
requests.

Syncthing and similar tools sync folders that several machines write, so two
people editing the same file end up with a conflict copy. pigeon gives every
file exactly one owner instead: only the owner's machines write it, so people
never conflict, and anyone else proposes or forces a change from the web
interface or the command line. It is our own tool, written in Rust on top of
existing libraries, so that the group can adapt it at will.

## Status

Nothing is implemented yet. [REQUIREMENTS.md](REQUIREMENTS.md) describes what
the first version must do, and [SOUL.md](SOUL.md) holds the rules that never
change.

## How it works

- **Personal folders.** A folder named `@` followed by a member's name, such
  as `/cheapmo/src/@mario`, belongs to that member: only their machines write
  it, and everyone can read it. A member can have as many as they like.
- **Drop folders.** Every other folder is a drop folder: anyone can add files
  to it, and a file never changes once someone else has received it.
- **Requests.** To change a file you do not own, you propose or force the
  change from the web interface or the command line, and the owner's machine
  applies it.
- **Subscriptions.** Everyone sees the whole tree, in the web interface or the
  command line; each machine downloads only the folders it subscribes to, and
  single files on demand.
- **Peer to peer.** Machines connect directly, end-to-end encrypted, over the
  local network or the internet.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
the [MIT license](LICENSE-MIT), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in pigeon by
you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
