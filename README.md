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

The first version is taking shape: [REQUIREMENTS.md](REQUIREMENTS.md)
describes what it must do, and [SOUL.md](SOUL.md) holds the rules that never
change.

## Usage

Build and install the single `pigeon` program, then run its daemon, which
syncs every group of the machine and serves the web UI on localhost:

```sh
cargo install --path crates/pigeon
pigeon daemon
```

From another terminal, found a group, or join one with the key a member
shared:

```sh
pigeon group create --name cheapmo --member mario
pigeon group key
pigeon group join --key cheapmo-… --member alice
```

Every command reads `pigeon <noun> <verb>`, asks for a missing argument when
a terminal is attached, and prints JSON with `--json`:

```sh
pigeon file list --under docs
pigeon file write --path @mario/notes.txt --content notes.txt
pigeon selection follow --pattern /docs/
pigeon request list
pigeon request accept --request .pigeon/requests/….json
pigeon --help
```

`pigeon ui` prints the link that opens the web UI, which offers the same
actions. `pigeon completions bash`, or `zsh`, `fish` and `powershell`, prints
the shell's completion script. The daemon keeps its state in
`$PIGEON_HOME`, by default `pigeon` in the user's data folder.

## How it works

- **Personal folders.** A folder named `@` followed by a member's name, such
  as `/cheapmo/src/@mario`, belongs to that member: only their machines write
  it, and everyone can read it. A member can have as many as they like.
- **Drop folders.** Every other folder is a drop folder: anyone can add files
  to it, and a file freezes once published, after which only a request
  changes it.
- **Requests.** To change a file you do not own, you propose or force the
  change from the web interface or the command line, and the owner's machine
  applies it.
- **Members.** A name and password make a member on any machine. A member
  changes their password with `pigeon member password`, after which their
  other machines log in with `pigeon member claim`; any member resets
  someone's password with `pigeon member reset` or excludes them with
  `pigeon member exclude`, and `pigeon group leave` excludes oneself. The
  name stays taken, and excluding renews the group key: machines the member
  list recognizes keep working, while the old key admits no new machine.
- **Subscriptions.** Everyone sees the whole tree, in the web interface or the
  command line; each machine downloads only the folders it subscribes to, and
  single files on demand.
- **One path everywhere.** A group's root is `/cheapmo` on Linux and macOS
  and `C:\cheapmo` on Windows, where programs on drive C: also resolve
  `/cheapmo`, so paths written in files hold on every machine. When only an
  administrator may create it, pigeon prints the command to run once: on
  macOS, a line in `/etc/synthetic.conf` links `/cheapmo` to a folder of your
  home. `--root` picks another folder.
- **Peer to peer.** Machines connect directly, end-to-end encrypted, over the
  local network or the internet.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
the [MIT license](LICENSE-MIT), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in pigeon by
you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
