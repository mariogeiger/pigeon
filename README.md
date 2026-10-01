# pigeon

pigeon keeps a group's files in sync, peer to peer. Each member owns
folders that the whole group sees, and changes to someone else's files go
through requests.

## Join a group

Ask a member for the group key, install [Rust](https://rustup.rs), then:

```sh
cargo install --git https://github.com/mariogeiger/pigeon pigeon
pigeon daemon
```

Leave the daemon running: it prints the link that opens the web interface.
In another terminal:

```sh
pigeon group join --key <the key> --member <your name>
```

Use the same name on your other machines.
If pigeon says only an administrator can create the group's folder, run the
command it prints and join again. The daemon's link opens the web
interface, where you choose the folders to follow; `pigeon ui` prints it
again. Open it once per browser; from then on the interface is at
<http://127.0.0.1:6767>.

`pigeon update` later builds the latest pigeon and restarts the daemon
onto it. To found a group, run its server, or manage members, see
[docs/setup.md](docs/setup.md).

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
the [MIT license](LICENSE-MIT), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in pigeon by
you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
