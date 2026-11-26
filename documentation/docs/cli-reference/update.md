---
title: update
---

Update `magic` to the latest public release:

```sh
magic update
```

The command downloads the binary for your platform, verifies its SHA-256 checksum, and replaces the installed CLI after checking that the new binary runs.

If an older build reports `a signed CLI release manifest is required`, reinstall once with the current installer:

```sh
curl -fsSL https://magicmtn.ai/install.sh | sh -s -- --no-modify-shell
```

If you installed to a custom directory, set `MAGIC_INSTALL_DIR` to that same absolute directory when running the installer. Subsequent updates use `magic update`.
