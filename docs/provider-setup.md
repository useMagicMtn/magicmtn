# Install and configure providers

Provider installation makes an executable runnable. Provider configuration establishes a connection to the user's account. Sandbox and inference operations use that saved connection.

## Package a runtime

A provider package contains `package.json`, `provider`, and `LICENSE`. Add `runtime.tar.gz` and an optional `runtime` declaration in `package.json` to include a private Python runtime and its dependencies:

```json
{
  "runtime": {
    "sha256": "SHA256_OF_RUNTIME_ARCHIVE",
    "environment": {
      "MAGICMTN_EXAMPLE_PYTHON": "python/bin/python3.13",
      "SSL_CERT_FILE": "python/lib/python3.13/site-packages/certifi/cacert.pem"
    },
    "check": {
      "executable": "python/bin/python3.13",
      "args": ["-I", "-c", "import example_sdk"]
    }
  }
}
```

The declaration is part of `magicmtn_provider_framework::host::Package`. Runtime paths are relative to the unpacked runtime directory. Supported environment entries are runtime selectors in your provider's `MAGICMTN_PROVIDER_` namespace and the certificate paths `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`, and `GRPC_DEFAULT_SSL_ROOTS_FILE_PATH`. Connection settings cannot override the package's runtime environment.

Build one dependency closure for each package platform. Use regular files with relative paths; materialize symlinks when creating the archive. The installer verifies its checksum, extracts it into staging, runs the check with a clean environment and a 30-second deadline, and activates the provider only after discovery succeeds. It downloads no dependencies, modifies no global Python environment, and receives no provider credentials. Failed installation preserves the active release. Runtime archives are limited to 256 MiB compressed, 1 GiB expanded, and 50,000 regular files.

The Modal provider's `scripts/build-runtime.py` builds its closure with a pinned standalone Python and hash-locked binary dependencies. Registry and source-package builders include that closure in each package.

## Implement account setup

Implement `magicmtn_sdk::provider_setup::ProviderSetup` alongside the workload backend:

```rust
pub trait ProviderSetup {
    fn configure<'a>(
        &'a self,
        options: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, ConfiguredConnection>;

    fn verify(&self) -> BoxFuture<'_, String>;
}
```

Attach explicit credentials before invoking either method. `configure` may idempotently create or select required account resources. It returns `ConfiguredConnection { account, configuration }`: a stable account or resource identity and nonsecret provider settings. Resolve omitted options from the saved configuration before applying defaults. Reconfiguration must preserve the saved identity; missing resources must not be silently replaced. Setup must not launch workload compute.

`verify` returns the same identity through a read-only check. Use the framework's `run_credentialed_with_setup` helper with a `SetupSpec`, an initializer returning the credentialed backend, and a workload dispatcher. The helper advertises setup options, routes `Operation::ProviderConfigure` to `configure`, and routes verification startup to `verify`. Credential-free discovery advertises setup without initializing the SDK or accessing an account. Other language hosts can implement the same RPC frames directly.

The host validates declared options, rejects credential fields and settings outside the provider's `MAGICMTN_PROVIDER_` namespace, and persists configuration separately from encrypted credentials. It preserves setup across restarts and provider upgrades. Changed settings advance the connection generation for future resources; existing resources retain their frozen bindings. Providers without setup support keep the credential-verification flow.

## Use a configured connection

```sh
magic provider install modal
magic server start
```

In another terminal, configure an account and prepare an image:

```sh
magic provider configure modal --secret-file ./credentials.json --option app=my-workspaces
magic provider verify modal
magic images prepare canonical --provider modal
```

Configuration creates or selects the Modal app and stores its immutable ID. Verification reports missing or inaccessible apps without creating replacements. Image preparation imports and validates the image in that app. Ordinary sandbox operations reuse the saved configuration and prepared image; they never install dependencies or invoke provider setup.
