# pigeon

pigeon keeps a group's files in sync, peer to peer. Each member owns
folders that the whole group sees, anyone may add files elsewhere, and
every other change waits as a suggestion that anyone in the group
validates or discards, with every version kept in the history.

![Three machines keep one tree in sync: an owner's edit reaches every machine; another member's edit of her file becomes a suggestion every machine sees, which a third member validates; a file dropped in inbox/ spreads to every machine; a deletion of it on one machine becomes a suggestion, which another member discards, bringing the file back](assets/pigeon.gif)

## Install

On Linux or macOS:

```sh
curl -sSf https://raw.githubusercontent.com/mariogeiger/pigeon/main/install.sh | sh
```

It installs Rust if needed, builds pigeon, and runs `pigeon setup`, which
joins or founds a group step by step and opens the web interface. Run
`pigeon setup` again at any time, and `pigeon update` to update.

An always-on server of a group asks nothing:

```sh
curl -sSf https://raw.githubusercontent.com/mariogeiger/pigeon/main/install.sh | sh -s -- server --key <the key> --member server
```

Every step also exists as a command: see [docs/setup.md](docs/setup.md).

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
the [MIT license](LICENSE-MIT), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in pigeon by
you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
