# Run MagicMtn locally

Run sandboxes on your Mac through a single-user MagicMtn server. The server stores resource records in bundled SQLite and uses providers installed on your computer.

## Prepare your machine

Install the [MagicMtn CLI](../documentation/docs/cli-basics.md#install) and [Apple Container](https://github.com/apple/container#get-started). Start the container runtime:

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

The server creates `~/.config/magicmtn/local/magicmtn.sqlite3` automatically and listens on `127.0.0.1:1924`. Open <http://localhost:1900> to view computers, images and their preparation status, and installed providers. In [Templates](http://localhost:1900/templates), save public GitHub repositories as `owner/repo` to clone them into `/workspace` on new computers. Open the [MCP Catalog](http://localhost:1900/mcp) to search services and find connection details for your MCP client. Sign in as `magic` with the dashboard password printed in the terminal. When server output is redirected, read the password from the private file named in the startup message. Keep this terminal open. No database service or separate SQLite installation is required.

To view the dashboard on an internal server, run `ssh -N -L 1900:127.0.0.1:1900 user@server` from your workstation, then open <http://localhost:1900> and use the dashboard credentials from that server.

Starting the server selects local mode for the CLI and Pi extension. Commands use the server address and credential saved in `~/.config/magicmtn/local/connection.json`. Local mode uses your machine and works without a hosted account.

## Create and use a sandbox

In another terminal, list the local image catalog and prepare the bundled Canonical image for Apple Container:

```sh
magic images list
magic images prepare canonical --provider apple-container
```

Preparation downloads the immutable image for your machine and checks the sandbox runtime, Pi, user, and workspace. The prepared artifact is saved in the local deployment and reused. Create a Canonical sandbox, run a command, and clean up.

```sh
sandbox_id=$(magic computer new --image canonical --provider apple-container)
magic computer exec "$sandbox_id" -- /bin/echo "hello from the sandbox"
magic computer terminate "$sandbox_id" --yes
```

To use the bundled Canonical image, run `magic computer new`. Use `magic images list` to list available images and `magic computer list` to list running sandboxes. To inspect a specific sandbox, run `magic computer show SANDBOX_ID --format json`.

## Add a custom image

Register an existing image without changing Canonical:

```sh
magic images add my-dev --from ghcr.io/my-org/dev:1.0
magic images prepare my-dev --provider apple-container
magic computer new --image my-dev
```

Registration resolves the tag to immutable platform digests. Reusing a name with different contents is rejected; choose a new name for another image version. The image must be readable from its registry and contain Python, Pi, the executable MagicMtn sandbox runtime at `/usr/local/bin/magicmtn-sandbox-agent`, and a writable `/workspace` for the `sandbox` user with UID and GID 10001. Preparation validates these requirements without modifying your image. Images can be built independently using the [sandbox runtime installer](../runtime/install.sh).

For a configured Modal provider, use `--provider modal` with the same `images prepare` command, then `magic computer new --image my-dev --provider modal`. Modal selects the image's AMD64 manifest and saves its imported image ID locally. Provider mappings and credentials stay with your local deployment.

`magic images list` shows registered images and preparation status per provider, rather than the container runtime's entire image inventory. Listing and registering images work without starting the server; preparation uses the running server and its configured provider connection. Use `--format json` for the catalog as JSON.

## Run Pi inside a sandbox

Install [Pi](../documentation/docs/coding-agents/pi.md#install-pi), configure an inference provider, and prepare Canonical for your sandbox provider. The [Modal and Pi walkthrough](../README.md#add-inference-with-modal-and-pi) covers provider installation and credentials, model selection, and image preparation.

Install the Pi extension in local mode, then start Pi:

```sh
magic agents install pi
pi
```

In Pi, run `/magic default` to select a model from your configured inference providers. Run `/magic new` to start a managed Pi session inside a Canonical sandbox. When multiple sandbox backends are prepared, choose the one to use; a single prepared backend is selected automatically. MagicMtn routes the sandbox's model requests to the selected inference provider.

Inside the managed session, Pi's shell and file tools operate in the sandbox, starting in `/computer`. Press **Ctrl+]** to disconnect while the sandbox session keeps running. Use `/magic view` and choose **Resume** to reconnect or **Terminate** to delete the sandbox and its workspace files.

The Pi process on your computer launches and reconnects to these managed sessions. When you return to host Pi, its built-in shell and file tools operate on your computer, and the MagicMtn tools let it manage sandboxes.

## Restart and clean up

Run `magic server stop` or press Ctrl+C in the server terminal, then run `magic server start` again. Sandbox records persist, and the lifecycle worker resumes inspection and pending cleanup. Stopping the server leaves running sandboxes available for the next startup.

Delete a sandbox and its files when finished:

```sh
magic computer terminate <ID> --yes
```

Sandboxes have a 24-hour default lifetime. Set `magic server start --ttl-seconds SECONDS` to choose a lifetime between 1 second and 24 hours for new sandboxes. The lifecycle worker cleans up expired sandboxes while the server is running and resumes that work after restart.

Run `magic server stop` before signing in to hosted MagicMtn with `magic login`.

Stop the server before installing or uninstalling providers. Keep provider releases referenced by active sandboxes until those sandboxes are deleted.

## Diagnose a Pi session

The server terminal reports session failures and recovery with the session ID, operation, error category, and retry delay. Persistent failures retry with exponential backoff capped at 30 seconds. Diagnostics omit raw error messages, credentials, and conversation content.

Run `magic pi status WORKSPACE_ID` to inspect a session. The `transport` and `error_category` fields identify a failed sandbox connection and clear after a successful exchange. Database inventory failures preserve existing session workers while the server retries; transport recovery reconnects to the existing sandbox and agent.

## Configuration

| Setting                     | Purpose                                                                                                                  |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `--network NAME`            | Override the Apple Container provider’s network. The provider defaults to `default` for internet access. Use `none` for isolation.                        |
| `--ui-bind 127.0.0.1:PORT`  | Choose the web dashboard port. The listener must use a loopback address.                                                 |
| `--bind 127.0.0.1:PORT`     | Choose the local server port. The listener must use a loopback address.                                                  |
| `MAGICMTN_CONFIG_DIRECTORY` | Change the CLI configuration and installed-provider directory from `~/.config/magicmtn`.                                 |
| `MAGICMTN_LOCAL_DIRECTORY`  | Override the local server's database, credential, and lock directory. Set it for both the server and local CLI commands. |

Keep the local directory to retain your sandbox records and server credential. For a backup, stop the server and copy the entire directory. Each directory belongs to one local installation.

## Configure provider credentials

Install the provider by its Registry name or with `magic provider install --file PACKAGE`, start the local server, and pass its JSON bundle with either `--secret-file` or `--secret-string`. Replace `PROVIDER` with the installed provider name.

```sh
magic provider configure PROVIDER --secret-file ./credentials.json
magic provider configure PROVIDER --secret-string "$PROVIDER_CREDENTIAL_JSON"
```

Each provider declares its required JSON fields. The JSON file can have any filename. The file form keeps the credential out of command arguments and shell history. A successful command configures the provider account, saves its resource identities and encrypted credential bundle, and prepares the connection for use. To rotate a credential for the same account, run the command with the replacement bundle.

Providers can declare setup options. For Modal, choose an app name with `--option app=NAME`; the default is `magicmtn`. Configuration creates the app if needed on the first connection and preserves its identity on subsequent calls.

```sh
magic provider configure modal --secret-file ./credentials.json --option app=my-workspaces
magic provider verify modal
```

Verification checks the saved connection without creating resources. Provider packages can include a private runtime and pinned dependencies; installation checks the runtime before activating the release. [Provider setup contract](provider-setup.md)

Back up `provider-credentials.key` separately from the SQLite database and restore both to recover configured credentials. The key is created in the private local data directory.
