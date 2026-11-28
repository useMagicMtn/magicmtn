# Build your first provider

This example implements an `echo` inference task and serves it through the MagicMtn provider protocol. Use Rust 1.100.0 or later. Its SDK dependencies are pinned to a repository revision, so you can copy this directory into your own project directory and build it independently.

From a MagicMtn checkout, copy the example outside the repository:

```sh
mkdir -p "$HOME/magicmtn-providers"
cp -R sdk/examples/minimal-provider "$HOME/magicmtn-providers/echo"
cd "$HOME/magicmtn-providers/echo"
cargo build --locked
cargo test --locked
```

The tests launch the provider process, send repeated inference requests, verify package installation and checksums, and check shutdown and timeout behavior. The built executable is `target/debug/minimal-magicmtn-provider`.

Implement your backend in `src/main.rs`, then adapt the tests to its capabilities. Keep both SDK dependencies on the same revision. See the [SDK interfaces](https://github.com/useMagicMtn/magicmtn/blob/main/sdk/README.md) and [provider setup and packaging guide](https://github.com/useMagicMtn/magicmtn/blob/main/docs/provider-setup.md) for additional backend types, credentials, and runtime packaging.
