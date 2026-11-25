# Run MagicMtn locally

Run sandboxes on your Mac through a single-user MagicMtn server. The server stores resource records in bundled SQLite and uses providers installed on your computer.

## Prepare your machine

Install the [MagicMtn CLI](../README.md#install-from-source) and [Apple Container](https://github.com/apple/container). Start the container runtime:

```sh
container system start
```

Install the Apple Container provider:

```sh
magic provider install apple-container
```

## Start the server

If you previously signed in to hosted MagicMtn, run `magic logout`. Start the local server in its own terminal.

```sh
magic server start
```

The server creates `~/.config/magicmtn/local/magicmtn.sqlite3` automatically and listens on `127.0.0.1:1924`. Keep this terminal open. No database service or separate SQLite installation is required.

Starting the server selects local mode for the CLI and Pi extension. Commands use the server address and credential saved in `~/.config/magicmtn/local/connection.json`. Local mode uses your machine and works without a hosted account.

## Create and use a sandbox

In another terminal, list the local image catalog and prepare the bundled Canonical image for Apple Container:

```sh
magic --local images list
magic --local images prepare canonical --provider apple-container
```

Preparation downloads the immutable image for your machine and checks the sandbox runtime, Pi, user, and workspace. The prepared artifact is saved in the local deployment and reused. Create a Canonical sandbox, run a command, and clean up.

```sh
sandbox_id=$(magic --local computer new --image canonical)
magic --local computer exec "$sandbox_id" -- /bin/echo "hello from the sandbox"
magic --local computer terminate "$sandbox_id" --yes
```

To use the bundled Canonical image, run `magic --local computer new`. Use `magic --local images list` to list available images and `magic --local computer list` to list running sandboxes. To inspect a specific sandbox, run `magic --local computer show SANDBOX_ID --format json`.

## Add a custom image

Register an existing image without changing Canonical:

```sh
magic --local images add my-dev --from ghcr.io/my-org/dev:1.0
magic --local images prepare my-dev --provider apple-container
magic --local computer new --image my-dev
```

Registration resolves the tag to immutable platform digests. Reusing a name with different contents is rejected; choose a new name for another image version. The image must be readable from its registry and contain Python, Pi, the executable MagicMtn sandbox runtime at `/usr/local/bin/magicmtn-sandbox-agent`, and a writable `/workspace` for the `sandbox` user with UID and GID 10001. Preparation validates these requirements without modifying your image. Images can be built independently using the [sandbox runtime installer](../runtime/install.sh).

For a configured Modal provider, use `--provider modal` with the same `images prepare` command, then `magic --local computer new --image my-dev --provider modal`. Modal selects the image's AMD64 manifest and saves its imported image ID locally. Provider mappings and credentials stay with your local deployment.

`magic --local images list` shows registered images and preparation status per provider, rather than the container runtime's entire image inventory. Listing and registering images work without starting the server; preparation uses the running server and its configured provider connection. Use `--format json` for the catalog as JSON.

## Use Pi with sandbox tools

Install the Pi extension in local mode, then start Pi:

```sh
magic agents install pi
pi
```

Pi stays on your computer and uses its own model and provider login. Select your model with `/model`. Use `/magic new` to choose a prepared image and provider and create a sandbox and `/magic view` to manage existing sandboxes. Pi can create, inspect, execute commands in, read and write files in, and terminate sandboxes through the MagicMtn tools. Its built-in shell and file tools continue to work on your computer.

## Restart and clean up

Run `magic server stop` or press Ctrl+C in the server terminal, then run `magic server start` again. Sandbox records persist, and the lifecycle worker resumes inspection and pending cleanup. Stopping the server leaves running sandboxes available for the next startup.

Delete a sandbox and its files when finished:

```sh
magic computer terminate <ID> --yes
```

Sandboxes have a 24-hour default lifetime. Set `magic server start --ttl-seconds SECONDS` to choose a lifetime between 1 second and 24 hours for new sandboxes. The lifecycle worker cleans up expired sandboxes while the server is running and resumes that work after restart.

Run `magic server stop` before signing in to hosted MagicMtn with `magic login`.

Stop the server before installing or uninstalling providers. Keep provider releases referenced by active sandboxes until those sandboxes are deleted.

## Configuration

| Setting                     | Purpose                                                                                                                  |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `--network default`         | Give new Apple Container sandboxes internet access. Use `--network none` for an isolated network.                        |
| `--bind 127.0.0.1:PORT`     | Choose the local server port. The listener must use a loopback address.                                                  |
| `MAGICMTN_CONFIG_DIRECTORY` | Change the CLI configuration and installed-provider directory from `~/.config/magicmtn`.                                 |
| `MAGICMTN_LOCAL_DIRECTORY`  | Override the local server's database, credential, and lock directory. Set it for both the server and local CLI commands. |

Keep the local directory to retain your sandbox records and server credential. For a backup, stop the server and copy the entire directory. Each directory belongs to one local installation.

## Configure provider credentials

Install the provider by its Registry name, start the local server, and pass its JSON bundle with either `--secret-file` or `--secret-string`. Replace `PROVIDER` with the exact installed Registry name.

```sh
magic --local provider configure PROVIDER --secret-file ./credentials.json
magic --local provider configure PROVIDER --secret-string "$PROVIDER_CREDENTIAL_JSON"
```

Each provider declares its required JSON fields. The JSON file can have any filename. The file form keeps the credential out of command arguments and shell history. A successful command verifies the provider account, saves the encrypted bundle, and prepares it for use. To rotate a credential for the same account, run the command with the replacement bundle.

Back up `provider-credentials.key` separately from the SQLite database and restore both to recover configured credentials. The key is created in the private local data directory.
