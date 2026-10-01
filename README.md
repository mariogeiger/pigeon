# pigeon

pigeon keeps a group's files in sync, peer to peer. Each member owns
folders that the whole group sees, and changes to someone else's files go
through requests.

![Three machines keep one tree in sync: an owner's edit reaches every machine, a locked file's change travels to its owner as a request she accepts, and a file dropped in inbox/ spreads and freezes](assets/pigeon.gif)

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
