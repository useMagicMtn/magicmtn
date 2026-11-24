#!/usr/bin/env python3
"""Prepare metadata-only execution approvals from the current public Registry.

Run on the release Mac with Podman for Linux adapter discovery. Does not install
host runtimes, create VMs, or change any server configuration.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import urllib.request

ORIGIN='https://magicmtn-provider-registry-993499103295.s3.us-east-2.amazonaws.com'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args=parser.parse_args()
    with urllib.request.urlopen(ORIGIN+'/index.json') as response:
        index=json.load(response)
    assert index['schema_version']==1
    releases=[]
    with tempfile.TemporaryDirectory(prefix='registry-discovery-') as directory:
        root=Path(directory)
        for provider in index['providers']:
            name,version=provider['name'],provider['version']
            assert name in ('apple-container','firecracker'), 'approve new Registry providers explicitly'
            for platform,artifact in provider['platforms'].items():
                with urllib.request.urlopen(f'{ORIGIN}/providers/{name}/{version}/{platform}.tar.gz') as response:
                    data=response.read(64*1024*1024+1)
                assert len(data)<=64*1024*1024 and hashlib.sha256(data).hexdigest()==artifact['sha256']
                package=root/'package.tar.gz'; package.write_bytes(data)
                with tarfile.open(package) as archive:
                    metadata=json.load(archive.extractfile('package.json'))
                    binary=archive.extractfile('provider').read()
                assert metadata['name']==name and metadata['version']==version
                assert f"{metadata['os']}-{metadata['arch']}"==platform
                assert hashlib.sha256(binary).hexdigest()==metadata['sha256']==artifact['executable_sha256']
                executable=root/'provider'; executable.write_bytes(binary); executable.chmod(0o755)
                protocol = metadata['protocol_version']
                assert protocol in (1, 2)
                frame = dict(id=1, protocol_version=protocol, provider=name)
                if protocol == 2:
                    frame.update(credential=None, verify_account=False)
                else:
                    frame['operation'] = dict(name='describe')
                request = json.dumps(frame) + '\n'
                command=[str(executable)]
                if metadata['os']=='linux':
                    arch={'aarch64':'arm64','x86_64':'amd64'}[metadata['arch']]
                    command=['podman','run','--rm','-i','--platform','linux/'+arch,'--network','none','-v',f'{root}:/adapter:ro','docker.io/library/alpine:3.22','/adapter/provider']
                environment = None if metadata['os'] == 'linux' else {
                    'PATH': os.defpath, 'HOME': str(root), 'TMPDIR': str(root)
                }
                response = json.loads(subprocess.check_output(
                    command, input=request, text=True, timeout=120, env=environment
                ))
                manifest=response['outcome']['ok']['value']
                assert manifest['provider_version']==version
                releases.append(dict(package=metadata,manifest=manifest,active=True,executable=None,
                    configuration_keys=['MAGICMTN_APPLE_CONTAINER_NETWORK'] if name=='apple-container' else ['MAGICMTN_FIRECRACKER_IMAGES'],runtime_environment={}))
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(releases,indent=2)+'\n')
    print(f'Prepared {len(releases)} metadata-only Registry approvals: {args.output}')


if __name__=='__main__': main()
