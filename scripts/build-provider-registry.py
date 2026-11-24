#!/usr/bin/env python3
"""Build only the authority-curated Registry adapters; never publish source code."""
import hashlib
import gzip
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
SOURCES = ROOT.parent
OUT = ROOT / 'dist/registry/packages'


def package(name, version, os_name, arch, executable, license_path, protocol_version=1):
    payload = executable.read_bytes()
    manifest = dict(name=name, version=version, protocol_version=protocol_version, os=os_name, arch=arch,
                    sha256=hashlib.sha256(payload).hexdigest())
    OUT.mkdir(parents=True, exist_ok=True)
    target = OUT / f'{name}-{version}-{os_name}-{arch}.tar.gz'
    with target.open('wb') as raw, gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as compressed, tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as archive:
        for filename, data, mode in [('package.json', json.dumps(manifest).encode(), 0o644),
                                     ('provider', payload, 0o755), ('LICENSE', license_path.read_bytes(), 0o644)]:
            info = tarfile.TarInfo(filename)
            info.size, info.mode = len(data), mode
            archive.addfile(info, io.BytesIO(data))
    return target


def build_linux(name, arches):
    provider = SOURCES / f'provider-{name}'
    metadata = tomllib.loads((provider / 'Cargo.toml').read_text())['package']
    version = metadata['version']
    protocol = metadata.get('metadata', {}).get('magicmtn', {}).get('protocol-version', 1)
    with tempfile.TemporaryDirectory(prefix='magic-registry-build-') as temporary:
        context = Path(temporary)
        source = context / 'provider'
        source.mkdir()
        for filename in ['Cargo.toml', 'Cargo.lock', 'LICENSE']:
            shutil.copy2(provider / filename, source / filename)
        shutil.copytree(provider / 'src', source / 'src')
        for crate in ['magicmtn-sdk', 'magicmtn-provider-framework']:
            shutil.copytree(ROOT / 'sdk/rust' / crate, context / 'sdk' / crate)
        (source / '.cargo').mkdir()
        (source / '.cargo/config.toml').write_text('[patch.crates-io]\nmagicmtn-sdk = { path = "../sdk/magicmtn-sdk" }\nmagicmtn-provider-framework = { path = "../sdk/magicmtn-provider-framework" }\n')
        targets = [f'{arch}-unknown-linux-musl' for arch in arches]
        target_options = ' '.join(f'--target {target}' for target in targets)
        exports = '\n'.join(
            f'COPY --from=builder /build/provider/target/{arch}-unknown-linux-musl/release/magicmtn-provider-{name} /{name}-{arch}'
            for arch in arches)
        (context / 'Containerfile').write_text(f'''FROM docker.io/library/rust:1.100.0-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea AS builder
RUN apt-get update && apt-get install -y --no-install-recommends python3-venv && rm -rf /var/lib/apt/lists/*
RUN python3 -m venv /opt/cross && /opt/cross/bin/pip install --no-cache-dir cargo-zigbuild==0.23.4 ziglang==0.14.1
ENV PATH=/opt/cross/bin:$PATH
RUN rustup target add {' '.join(targets)}
COPY sdk /build/sdk
COPY provider /build/provider
WORKDIR /build/provider
RUN cargo zigbuild --locked --release {target_options}
FROM scratch
{exports}
''')
        subprocess.run(['podman', 'build', '--platform', 'linux/arm64', '--layers', '-t', 'localhost/magicmtn-registry-build', str(context)], check=True)
        container = subprocess.check_output(['podman', 'create', 'localhost/magicmtn-registry-build', f'/{name}-{arches[0]}'], text=True).strip()
        try:
            for arch in arches:
                binary = context / f'{name}-{arch}'
                subprocess.run(['podman', 'cp', f'{container}:/{name}-{arch}', str(binary)], check=True)
                package(name, version, 'linux', arch, binary, provider / 'LICENSE', protocol)
        finally:
            subprocess.run(['podman', 'rm', container], check=True)


def main():
    toolchain = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    cargo_build = ['cargo', f'+{toolchain}', 'build', '--release', '--locked']
    for name in ['aws', 'apple-container', 'microsoft-mxc', 'modal', 'ollama']:
        executable = f'magicmtn-provider-{name}'
        provider = SOURCES / f'provider-{name}'
        subprocess.run(cargo_build, cwd=provider, check=True)
        metadata = tomllib.loads((provider / 'Cargo.toml').read_text())['package']
        version = metadata['version']
        protocol = metadata.get('metadata', {}).get('magicmtn', {}).get('protocol-version', 1)
        package(name, version, 'macos', 'aarch64', provider / 'target/release' / executable,
                provider / 'LICENSE', protocol)
        if name in ('aws', 'modal'):
            build_linux(name, ['x86_64', 'aarch64'])
    build_linux('firecracker', ['x86_64', 'aarch64'])
    build_linux('nvx', ['x86_64'])
    print(OUT)


if __name__ == '__main__':
    main()
