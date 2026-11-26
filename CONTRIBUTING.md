# Contributing to MagicMtn

Start with the [local quickstart](docs/local-server.md) for a user workflow and the [Bazel guide](docs/bazel.md) for reusable engine and SDK targets.

## Build and test

Install Rust 1.100.0 and Bazelisk. Run these checks from the repository root.

```sh
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
bazel test //...
```

Cargo uses a three-day cooldown when selecting newly published dependency versions.

For changes to the Pi extension, use Node.js 24 and run `npm ci --prefix interfaces/pi --ignore-scripts` followed by `npm --prefix interfaces/pi test`. For runtime changes, install `runtime/requirements.txt` into a Python 3.13 virtual environment and run `python -m unittest discover -s runtime/tests`. The [runtime CI workflow](.github/workflows/sandbox-runtime.yml) shows how to exercise the real Pi and OpenCode binaries.

## Web dashboard

The `web-ui` crate owns the dashboard shell, embedded assets, and shared page templates. To extend the dashboard in another Axum host, merge `magicmtn_web_ui::assets()` into its router and add the crate's `templates` directory after the host's templates in `askama.toml`. Extend `shared/shell.html` from the host's `base.html` to supply navigation entries, styles, or scripts. Shared pages use the host's layout and render data supplied by its authenticated handlers. Run `cargo test -p magicmtn-cli --test mode_exclusion` to exercise both listeners through the CLI.

## Submit a change

Open a pull request that describes the user-visible behavior and checks you ran. Keep changes focused and update documentation when behavior changes. Sign commits with your GPG key. Contributions to MagicMtn are licensed under the [Apache License 2.0](LICENSE).

Report vulnerabilities using the private channel in [SECURITY.md](SECURITY.md).
