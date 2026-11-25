import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('publisher', Path(__file__).with_name('publish-provider-registry.py'))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class RegistryReleaseTests(unittest.TestCase):
    def package(self, root, name='firecracker', version='0.1.0', arch='aarch64', duplicate=False):
        binary = bytearray(64)
        binary[:6] = b'\x7fELF\x02\x01'
        binary[18:20] = (183).to_bytes(2, 'little')
        manifest = dict(name=name, version=version, protocol_version=1, os='linux', arch=arch, sha256=publisher.digest(binary))
        path = root / 'package.tar.gz'
        with tarfile.open(path, 'w:gz') as archive:
            for name, body in [('package.json', json.dumps(manifest).encode()), ('provider', binary), ('LICENSE', b'license')]+([('provider', binary)] if duplicate else []):
                info = tarfile.TarInfo(name); info.size = len(body)
                archive.addfile(info, io.BytesIO(body))
        return path

    def test_runtime_payload_checksums_and_paths_are_verified(self):
        payload = io.BytesIO()
        with tarfile.open(fileobj=payload, mode='w:gz') as archive:
            info = tarfile.TarInfo('python/bin/python'); info.size = 7; info.mode = 0o755
            archive.addfile(info, io.BytesIO(b'fixture'))
        data = payload.getvalue()
        spec = {'sha256': publisher.digest(data), 'environment': {'MAGICMTN_FIXTURE_PYTHON': 'python/bin/python'},
                'check': {'executable': 'python/bin/python', 'args': []}}
        publisher.inspect_runtime(data, spec)
        with self.assertRaisesRegex(ValueError, 'checksum'):
            publisher.inspect_runtime(data + b'changed', spec)
        spec['environment'] = {'PATH': 'python/bin/python'}
        with self.assertRaisesRegex(ValueError, 'environment'):
            publisher.inspect_runtime(data, spec)
        escaped = io.BytesIO()
        with tarfile.open(fileobj=escaped, mode='w:gz') as archive:
            info = tarfile.TarInfo('../outside'); info.size = 0
            archive.addfile(info, io.BytesIO())
        data = escaped.getvalue(); spec['sha256'] = publisher.digest(data)
        with self.assertRaisesRegex(ValueError, 'runtime entry'):
            publisher.inspect_runtime(data, spec)

    def test_unsupported_names_cannot_publish(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'not approved'):
                publisher.prepare([self.package(root, name='private-provider')], root / 'out')

    def test_incomplete_releases_fail_before_index_creation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'all approved platform builds'):
                publisher.prepare([self.package(root)], root / 'out')
            self.assertFalse((root / 'out/index.json').exists())

    def test_architecture_and_duplicate_entries_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'architecture mismatch'):
                publisher.inspect_package(self.package(root, arch='x86_64'))
            with self.assertRaisesRegex(ValueError, 'invalid package entries'):
                publisher.inspect_package(self.package(root, duplicate=True))

    def test_prune_errors_are_not_silently_ignored(self):
        # Verify cleanup is after publication and scoped to obsolete provider objects.
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary); index=root/'index.json'; index.write_text('{"providers":[]}')
            missing = publisher.subprocess.CalledProcessError(1, 'aws', stderr='NoSuchKey')
            calls=[]
            def aws(*args):
                calls.append(args)
                if args[1]=='list-objects-v2': return json.dumps({'Contents':[{'Key':'providers/old/1/linux.tar.gz'}]})
                if args[1]=='delete-objects': return '{"Errors":[{"Code":"AccessDenied"}]}'
                return '{}'
            with patch.object(publisher.subprocess, 'run', side_effect=missing), patch.object(publisher, 'aws', side_effect=aws), patch.object(publisher, 'record_publication') as record:
                with self.assertRaisesRegex(RuntimeError, 'could not be removed'):
                    publisher.publish(index, {}, {'providers': []})
            self.assertEqual(record.call_count, 2)
            self.assertEqual([args[1] for args in calls], ['put-object','list-objects-v2','delete-objects'])

    def test_publication_requires_documentation_for_each_release(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            index = root / 'index.json'
            index.write_text(json.dumps({'providers': [{'name': 'new-provider', 'version': '1.0.0'}]}))
            docs = root / 'docs.json'
            docs.write_text('{"providers": []}')
            with self.assertRaisesRegex(ValueError, 'repo_url are required'):
                publisher.publication(index, docs)
            docs.write_text(json.dumps({'providers': [{'name': 'new-provider', 'requirements': 'Linux', 'documentation': 'Use it', 'repo_url': 'https://github.com/example/new-provider'}]}))
            self.assertEqual(publisher.publication(index, docs)['providers'][0]['documentation'], 'Use it')
            self.assertEqual(publisher.publication(index, docs)['providers'][0]['repo_url'], 'https://github.com/example/new-provider')

    def test_database_failure_fails_publication_before_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            index = Path(temporary) / 'index.json'
            index.write_text('{"providers": []}')
            missing = publisher.subprocess.CalledProcessError(1, 'aws', stderr='NoSuchKey')
            failure = publisher.subprocess.CalledProcessError(1, 'magicmtn-publish-provider')
            with patch.object(publisher.subprocess, 'run', side_effect=missing), patch.object(publisher, 'aws', return_value='{}') as aws, patch.object(publisher, 'record_publication', side_effect=[None, failure]):
                with self.assertRaises(publisher.subprocess.CalledProcessError):
                    publisher.publish(index, {}, {'providers': []})
            self.assertEqual([call.args[1] for call in aws.call_args_list], ['put-object'])

    def test_selected_provider_requires_only_its_approved_platforms(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            catalog = {'providers': [
                {'name': 'firecracker', 'description': 'test', 'platforms': ['linux-aarch64']},
                {'name': 'unreleased', 'platforms': ['linux-x86_64']},
            ]}
            (root / 'registry').mkdir()
            (root / 'registry/providers.json').write_text(json.dumps(catalog))
            package = self.package(root)
            with patch.object(publisher, 'ROOT', root):
                index, uploads = publisher.prepare([package], root / 'out', providers=['firecracker'])
                self.assertEqual([p['name'] for p in json.loads(index.read_text())['providers']], ['firecracker'])
                self.assertEqual(len(uploads), 1)
                with self.assertRaisesRegex(ValueError, 'not approved'):
                    publisher.prepare([package], root / 'out', providers=['unknown'])
                with self.assertRaisesRegex(ValueError, 'not approved'):
                    publisher.prepare([package], root / 'out', providers=['unreleased'])

    def test_selected_publication_preserves_existing_providers_and_objects(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            index = root / 'index.json'
            release = {'name': 'mxc', 'version': '0.1.0', 'platforms': {'macos-aarch64': {'sha256': 'new'}}}
            previous = {'name': 'apple-container', 'version': '0.1.2', 'platforms': {'macos-aarch64': {'sha256': 'old'}}}
            index.write_text(json.dumps({'schema_version': 1, 'providers': [release]}))
            calls = []
            def get(command, **kwargs):
                Path(command[command.index('index.json') + 1]).write_text(json.dumps({'schema_version': 1, 'providers': [previous]}))
                return publisher.subprocess.CompletedProcess(command, 0, stdout='{"ETag":"original"}')
            def aws(*args):
                calls.append(args)
                if args[args.index('--key') + 1] == 'index.json':
                    written = json.loads(Path(args[args.index('--body') + 1]).read_text())
                    self.assertEqual(written['providers'], [previous, release])
                    self.assertEqual(args[args.index('--if-match') + 1], 'original')
                return '{}'
            metadata = {'providers': [release]}
            with patch.object(publisher.subprocess, 'run', side_effect=get), \
                    patch.object(publisher, 'aws', side_effect=aws), \
                    patch.object(publisher, 'record_publication') as record:
                publisher.publish(index, {'providers/mxc/0.1.0/macos-aarch64.tar.gz': root / 'package'},
                                  metadata, preserve_existing=True)
            self.assertEqual([call[1] for call in calls], ['put-object', 'put-object'])
            self.assertEqual(record.call_args_list[0].args[0], metadata)
            self.assertEqual(record.call_args_list[1].args[0], metadata)

    def test_same_version_cannot_change_archive_even_if_executable_matches(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            index = root / 'index.json'
            old = {'name': 'mxc', 'version': '0.1.0', 'platforms': {
                'macos-aarch64': {'sha256': 'old-archive', 'executable_sha256': 'same-executable'}}}
            new = json.loads(json.dumps(old))
            new['platforms']['macos-aarch64']['sha256'] = 'new-archive'
            index.write_text(json.dumps({'schema_version': 1, 'providers': [new]}))
            def get(command, **kwargs):
                Path(command[command.index('index.json') + 1]).write_text(json.dumps({'schema_version': 1, 'providers': [old]}))
                return publisher.subprocess.CompletedProcess(command, 0, stdout='{"ETag":"original"}')
            with patch.object(publisher.subprocess, 'run', side_effect=get), \
                    patch.object(publisher, 'aws') as aws, patch.object(publisher, 'record_publication'):
                with self.assertRaisesRegex(ValueError, 'increment provider version'):
                    publisher.publish(index, {}, {'providers': [new]}, preserve_existing=True)
            aws.assert_not_called()

    def test_index_conflict_prevents_database_commit(self):
        with tempfile.TemporaryDirectory() as temporary:
            index = Path(temporary) / 'index.json'
            index.write_text('{"schema_version":1,"providers":[]}')
            missing = publisher.subprocess.CalledProcessError(1, 'aws', stderr='NoSuchKey')
            conflict = publisher.subprocess.CalledProcessError(1, 'aws', stderr='PreconditionFailed')
            with patch.object(publisher.subprocess, 'run', side_effect=missing), \
                    patch.object(publisher, 'aws', side_effect=conflict), \
                    patch.object(publisher, 'record_publication') as record:
                with self.assertRaises(publisher.subprocess.CalledProcessError):
                    publisher.publish(index, {}, {'providers': []}, preserve_existing=True)
            self.assertEqual(record.call_count, 1)
            self.assertTrue(record.call_args.kwargs['check'])


if __name__ == '__main__':
    unittest.main()
