#!/usr/bin/env python3
"""Validate a complete curated release, then optionally publish it to the Registry.

Without --publish this only prepares dist/registry/release. Publishing requires
MagicMtn's AWS credentials and a backend Registry owner connection. A release is
complete only after its metadata and user documentation are recorded in Postgres.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BUCKET = 'magicmtn-provider-registry-993499103295'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def inspect_package(path):
    if path.stat().st_size > 64 * 1024 * 1024:
        raise ValueError('package too large')
    with tarfile.open(path, 'r:gz') as archive:
        entries = archive.getmembers()
        if len(entries) != 3 or {e.name for e in entries} != {'package.json', 'provider', 'LICENSE'}:
            raise ValueError('invalid package entries')
        if any(not e.isfile() or e.size > (64 * 1024 * 1024 if e.name == 'provider' else 65536) for e in entries):
            raise ValueError('invalid package entry type or size')
        manifest = json.load(archive.extractfile('package.json'))
        binary = archive.extractfile('provider').read()
        if manifest['protocol_version'] not in (1, 2) or digest(binary) != manifest['sha256']:
            raise ValueError('invalid protocol or executable digest')
        if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?', manifest['version']):
            raise ValueError('invalid version')
        # Verify platform labels against the executable header before distribution.
        if manifest['os'] == 'linux':
            machine = int.from_bytes(binary[18:20], 'little')
            if binary[:6] != b'\x7fELF\x02\x01' or machine != {'x86_64': 62, 'aarch64': 183}.get(manifest['arch']):
                raise ValueError('Linux executable architecture mismatch')
        elif manifest['os'] == 'macos':
            if binary[:4] != b'\xcf\xfa\xed\xfe' or int.from_bytes(binary[4:8], 'little') != 0x100000c or manifest['arch'] != 'aarch64':
                raise ValueError('macOS executable architecture mismatch')
        else:
            raise ValueError('unsupported platform')
        return manifest


def prepare(paths, output, providers=None):
    catalog = json.loads((ROOT / 'registry/providers.json').read_text())
    approved = {entry['name']: entry for entry in catalog['providers']}
    if len(approved) != len(catalog['providers']) or any(not re.fullmatch('[a-z][a-z0-9-]{0,63}', name) for name in approved):
        raise ValueError('Registry names must be valid and globally unique')
    if providers is not None:
        if not providers or not set(providers) <= approved.keys():
            raise ValueError('selected provider is not approved for the Registry')
        approved = {name: entry for name, entry in approved.items() if name in providers}
    releases = {}
    uploads = {}
    for path in paths:
        manifest = inspect_package(path)
        name, version = manifest['name'], manifest['version']
        if name not in approved:
            raise ValueError(f'{name} is not approved for the Registry')
        platform = f"{manifest['os']}-{manifest['arch']}"
        if platform not in approved[name]['platforms']:
            raise ValueError(f'{name}: unapproved platform {platform}')
        release = releases.setdefault(name, dict(name=name, description=approved[name]['description'], version=version, platforms={}))
        if release['version'] != version or platform in release['platforms']:
            raise ValueError(f'{name}: supply exactly one version and one package per platform')
        release['platforms'][platform] = {'sha256': digest(path.read_bytes()), 'executable_sha256': manifest['sha256']}
        key = f'providers/{name}/{version}/{platform}.tar.gz'
        uploads[key] = path
    for name, entry in approved.items():
        if name not in releases or set(releases[name]['platforms']) != set(entry['platforms']):
            raise ValueError(f'{name}: all approved platform builds are required')
    output.mkdir(parents=True, exist_ok=True)
    index = output / 'index.json'
    index.write_text(json.dumps({'schema_version': 1, 'providers': [releases[name] for name in approved]}, indent=2)+'\n')
    return index, uploads


def aws(*args):
    return subprocess.check_output(['aws', *args, '--profile', 'magicmtn', '--region', 'us-east-2', '--output', 'json'], text=True)


def publication(index, documentation):
    """Join release metadata with publisher-supplied user documentation."""
    source = json.loads(documentation.read_text())['providers']
    docs = {entry['name']: entry for entry in source}
    if len(docs) != len(source):
        raise ValueError('duplicate provider documentation')
    providers = []
    for release in json.loads(index.read_text())['providers']:
        entry = docs.get(release['name'])
        if not entry or any(not isinstance(entry.get(key), str) or not entry[key].strip()
                            for key in ('requirements', 'documentation', 'repo_url')):
            raise ValueError(f"{release['name']}: requirements, user documentation and repo_url are required")
        providers.append({**release, 'requirements': entry['requirements'], 'documentation': entry['documentation'], 'repo_url': entry['repo_url']})
    return {'schema_version': 1, 'providers': providers}


def record_publication(metadata, publisher, check=False):
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / 'publication.json'
        path.write_text(json.dumps(metadata))
        command = [publisher, '--file', str(path)]
        if check:
            command.append('--check')
        # REGISTRY_DATABASE_URL stays in the environment, never in arguments.
        subprocess.run(command, check=True)


def publish(index, uploads, metadata, publisher='magicmtn-publish-provider', preserve_existing=False):
    record_publication(metadata, publisher, check=True)
    candidate = json.loads(index.read_text())
    publication_index = index
    # Conditional publication serializes maintainers and protects immutable versions.
    condition = ['--if-none-match', '*']
    with tempfile.TemporaryDirectory() as temporary:
        previous_path = Path(temporary) / 'previous.json'
        try:
            result = subprocess.run(['aws', 's3api', 'get-object', '--bucket', BUCKET, '--key', 'index.json', str(previous_path), '--profile', 'magicmtn', '--region', 'us-east-2', '--output', 'json'], capture_output=True, text=True, check=True)
        except subprocess.CalledProcessError as error:
            if 'NoSuchKey' not in error.stderr:
                raise
        else:
            condition = ['--if-match', json.loads(result.stdout)['ETag']]
            previous_index = json.loads(previous_path.read_text())
            previous = {entry['name']: entry for entry in previous_index['providers']}
            if previous_index.get('schema_version') != 1 or len(previous) != len(previous_index['providers']):
                raise ValueError('invalid existing Registry index')
            for release in candidate['providers']:
                old = previous.get(release['name'])
                if old and old['version'] == release['version']:
                    if old['platforms'] != release['platforms']:
                        raise ValueError('increment provider version before changing published artifacts')
            if preserve_existing:
                previous.update((release['name'], release) for release in candidate['providers'])
                publication_index = index.with_name('publication-index.json')
                publication_index.write_text(json.dumps({
                    'schema_version': 1, 'providers': list(previous.values())
                }, indent=2) + '\n')
    # One complete index is the commit point; old binaries are removed afterward.
    for key, path in uploads.items():
        aws('s3api', 'put-object', '--bucket', BUCKET, '--key', key, '--body', str(path),
            '--content-type', 'application/gzip', '--cache-control', 'no-cache')
    aws('s3api', 'put-object', '--bucket', BUCKET, '--key', 'index.json', '--body', str(publication_index),
        '--content-type', 'application/json', '--cache-control', 'no-cache', *condition)
    # A failed database write fails publication and leaves artifacts for a retry.
    # Record only after the conditional S3 commit succeeds. Cleanup follows both
    # commits. Maintainer publications must run serially across these two stores.
    record_publication(metadata, publisher)
    if preserve_existing:
        print(f'Published {len(uploads)} platform builds; retained existing providers and artifacts.')
        return
    listing = json.loads(aws('s3api', 'list-objects-v2', '--bucket', BUCKET, '--prefix', 'providers/'))
    obsolete = [obj['Key'] for obj in listing.get('Contents', []) if obj['Key'] not in uploads]
    for offset in range(0, len(obsolete), 1000):
        result = json.loads(aws('s3api', 'delete-objects', '--bucket', BUCKET, '--delete',
                               json.dumps({'Objects': [{'Key': key} for key in obsolete[offset:offset+1000]], 'Quiet': True})))
        if result.get('Errors'):
            raise RuntimeError('old Registry artifacts could not be removed: '+str(result['Errors']))
    print(f'Published {len(uploads)} platform builds; removed {len(obsolete)} obsolete objects.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('packages', nargs='*', type=Path)
    parser.add_argument('--publish', action='store_true')
    parser.add_argument('--documentation', type=Path, required=True)
    parser.add_argument('--registry-publisher', default='magicmtn-publish-provider')
    parser.add_argument('--provider', action='append', help='Publish only this approved provider; preserve all other providers and existing artifacts')
    parser.add_argument('--import-index', type=Path, help='Record an existing Registry index in the backend without uploading or deleting artifacts')
    args = parser.parse_args()
    if args.import_index:
        if args.packages or args.publish or args.provider:
            parser.error('--import-index cannot be combined with packages, --publish, or --provider')
        record_publication(publication(args.import_index, args.documentation), args.registry_publisher)
        return
    if not args.packages:
        parser.error('packages are required')
    index, uploads = prepare(args.packages, ROOT / 'dist/registry/release', providers=args.provider)
    metadata = publication(index, args.documentation)
    print(index)
    if args.publish:
        publish(index, uploads, metadata, args.registry_publisher, preserve_existing=bool(args.provider))


if __name__ == '__main__':
    main()
