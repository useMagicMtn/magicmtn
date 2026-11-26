---
title: Local
---

Local sandboxes run on your computer. Install the runtime for your operating system before enabling local mode. These instructions require an existing saved local runner connection.

To run Codex with your ChatGPT subscription on your Mac, follow [Codex in a local sandbox](../codex.md).

## macOS: Apple Container

Install [Apple Container](https://github.com/apple/container), then start and check the service:

```sh
container system start
container system status
```

Install the adapter from the MagicMtn Registry:

```sh
magic provider install apple-container
```

Canonical is included in the local image catalog. With the local server running, use `magic --local images prepare canonical --provider apple-container` to download and validate it for Apple Container. Use `magic --local images list` to inspect registered images and preparation status.

## Linux: Firecracker

Follow the [Firecracker getting started guide](https://github.com/firecracker-microvm/firecracker/blob/main/docs/getting-started.md) to install Firecracker on an x86-64 or ARM64 Linux host. The runner user needs read/write access to `/dev/kvm`. Install Python 3 and OpenSSH client tools too.

```sh
firecracker --version
test -r /dev/kvm && test -w /dev/kvm
```

Firecracker needs a kernel and disk image. Prepare the following on the host:

1. A compatible Linux kernel and an ext4 root filesystem containing the selected MagicMtn Canonical image's software, SSH server, `timeout`, and `/workspace`.
2. Guest SSH authentication, with the host key pinned in a dedicated `known_hosts` file. Keep the private key on the host.
3. A TAP interface and guest address for each concurrent sandbox, plus routing, outbound NAT, and DNS. Follow Firecracker's networking guidance for your distribution.

Install the adapter from the MagicMtn Registry:

```sh
magic provider install firecracker
```

Create a private host configuration, for example `~/.config/magicmtn/firecracker.json`. All paths must be absolute. Replace the example paths, addresses, and image label with your prepared resources:

```json
{
  "firecracker": "/usr/local/bin/firecracker",
  "state_dir": "/home/alice/.local/share/magicmtn/firecracker",
  "images": {
    "default-v1": {
      "kernel": "/home/alice/vms/vmlinux",
      "rootfs": "/home/alice/vms/magicmtn-canonical.ext4"
    }
  },
  "slots": [{
    "tap": "magic-tap0",
    "mac": "06:00:00:00:00:01",
    "guest_ip": "172.16.0.2",
    "user": "root",
    "ssh_key": "/home/alice/vms/id_ed25519",
    "known_hosts": "/home/alice/vms/known_hosts",
    "boot_args": "console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw ip=172.16.0.2::172.16.0.1:255.255.255.0::eth0:off"
  }]
}
```

Configure the runner environment. The mapping key must be the **exact repository and SHA-256 digest** of your organization's MagicMtn Canonical image; its value is the corresponding prepared image label above. Do not map a different root filesystem to that digest.

```sh
export MAGICMTN_FIRECRACKER_CONFIG="$HOME/.config/magicmtn/firecracker.json"
export MAGICMTN_FIRECRACKER_IMAGES='{"registry.example/default@sha256:REPLACE_WITH_DIGEST":"default-v1"}'
```

Each sandbox receives a separate writable disk copy and reserves one network slot. Terminating it removes that copy and frees the slot. A stopped VM retains its slot until cleanup.

This provider launches Firecracker as the runner user; it does not configure Firecracker's jailer. Use it for personal local workloads, and follow upstream's jailer guidance before designing a shared host service.

## Linux: Microsoft NVX

Use [Microsoft NVX](https://github.com/microsoft/nvx) to run bounded commands in local microVMs on x86-64 Linux. Follow NVX's [setup guide](https://github.com/microsoft/nvx/blob/dev/doc/setup.md) to install `nvx-rs`, OpenVMM, the kernel, and the Alpine control initramfs. Install Python 3.9+ and grant the runner access to `/dev/kvm`, or `/dev/mshv` for an MSHV installation.

Prepare an EROFS distro layer containing `/bin/sh`, your tools, and a non-root user, plus a compatible preformatted ext4 scratch template. Record the EROFS filesystem UUID. Follow the provider's [image preparation and compatibility instructions](https://github.com/useMagicMtn/provider-nvx/blob/main/CONTRIBUTING.md#host-and-guest-configuration).

Install the adapter:

```sh
magic provider install nvx
```

Create a private configuration file such as `~/.config/magicmtn/nvx.json`. Replace the paths and UUID with your prepared resources:

```json
{
  "nvx": "/opt/nvx/bin/nvx-rs",
  "artifact_root": "/opt/nvx",
  "state_dir": "/home/alice/.local/share/magicmtn/nvx",
  "hypervisor": "kvm",
  "memory_mib": 2048,
  "workload_user": "65534:65534",
  "network": "none",
  "images": {
    "default-v1": {
      "distro": "/home/alice/vms/canonical.erofs",
      "distro_uuid": "11111111-1111-1111-1111-111111111111",
      "scratch": "/home/alice/vms/scratch-template.ext4"
    }
  }
}
```

Configure the runner environment. Replace the digest placeholder with the exact SHA-256 digest of the image whose software you prepared in the distro layer:

```sh
export MAGICMTN_NVX_CONFIG="$HOME/.config/magicmtn/nvx.json"
export MAGICMTN_NVX_IMAGES='{"registry.example/canonical@sha256:REPLACE_WITH_DIGEST":"default-v1"}'
```

Each sandbox receives a private writable scratch copy. Commands run as the configured non-root user; ensure that user exists in the distro and can write its workspace. Set `"network": "outbound"` to allow outbound networking through NVX while keeping ingress and host-loopback access denied.

Use this provider for bounded command workloads. Files persist between commands, but NVX ends background processes when their execution ends. For Pi sessions that launch a persistent background sandbox agent, choose a provider that supports that process lifetime, such as Firecracker.

Keep the state directory and NVX installation available across runner restarts. Terminate sandboxes to stop their VM and release their scratch copy. If cleanup fails, resolve the reported problem and retry termination; retain the state and disk until NVX confirms that the VM has stopped.

## Existing local runner connections

Run `magic-provider-runner` with your saved catalog and credentials. Keep it running while using or deleting local sandboxes.

With an existing saved connection, `local-sandbox = true` in `~/.config/magicmtn/config` enables local placement for `magic computer new` and Pi's `/magic new`. An offline runner never causes local mode to switch to cloud compute.
