import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('builder', Path(__file__).with_name('build-provider-registry.py'))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class LinuxPackageTests(unittest.TestCase):
    def test_linux_builds_match_catalog_and_embed_runtime(self):
        catalog = json.loads((builder.ROOT / 'registry/providers.json').read_text())
        for provider in catalog['providers']:
            arches = [platform.removeprefix('linux-') for platform in provider['platforms']
                      if platform.startswith('linux-')]
            if not arches:
                continue
            name = provider['name']
            with self.subTest(provider=name):
                commands = []
                def run(command, **kwargs):
                    commands.append(command)
                    if command[1] == 'build':
                        context = Path(command[-1])
                        containerfile = (context / 'Containerfile').read_text()
                        runtime = {
                            'aws': ['aws_bridge.py', 'nitro_eks.py'],
                            'modal': ['modal_bridge.py'],
                        }.get(name, ['runtime.py'])
                        for filename in runtime:
                            self.assertTrue((context / 'provider/src' / filename).is_file())
                        self.assertTrue((context / 'sdk/magicmtn-sdk/Cargo.toml').is_file())
                        self.assertEqual(containerfile.count('COPY --from=builder'), len(arches))
                        for arch in arches:
                            self.assertIn(f'--target {arch}-unknown-linux-musl', containerfile)
                            self.assertIn(f'/magicmtn-provider-{name} /{name}-{arch}', containerfile)
                    return None
                with patch.object(builder.subprocess, 'run', side_effect=run), \
                        patch.object(builder.subprocess, 'check_output', return_value='container-id\n') as create, \
                        patch.object(builder, 'package') as package:
                    builder.build_linux(name, arches)
                self.assertEqual([(call.args[0], call.args[2], call.args[3])
                                  for call in package.call_args_list],
                                 [(name, 'linux', arch) for arch in arches])
                self.assertEqual(create.call_args.args[0][-1], f'/{name}-{arches[0]}')
                self.assertEqual(commands[-1], ['podman', 'rm', 'container-id'])

    def test_release_builder_covers_every_catalog_platform(self):
        catalog = json.loads((builder.ROOT / 'registry/providers.json').read_text())
        with patch.object(builder.subprocess, 'run'), patch.object(builder, 'package') as package, \
                patch.object(builder, 'build_linux') as linux, patch('sys.stdout', new_callable=io.StringIO):
            builder.main()
        actual = {(call.args[0], f'{call.args[2]}-{call.args[3]}')
                  for call in package.call_args_list}
        actual.update((call.args[0], f'linux-{arch}')
                      for call in linux.call_args_list for arch in call.args[1])
        expected = {(provider['name'], platform)
                    for provider in catalog['providers'] for platform in provider['platforms']}
        self.assertEqual(actual, expected)


if __name__ == '__main__':
    unittest.main()
