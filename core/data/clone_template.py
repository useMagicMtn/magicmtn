"""Clone template repositories independently inside a sandbox."""
import base64
import json
import os
import pathlib
import re
import subprocess
import sys
import urllib.parse


def apply(config, run=subprocess.run, workspace=pathlib.Path('/workspace')):
    origin = config['origin'].rstrip('/')
    parsed = urllib.parse.urlsplit(origin)
    if (parsed.scheme not in ('http', 'https') or not parsed.hostname
            or parsed.username or parsed.password or parsed.path
            or parsed.query or parsed.fragment):
        return
    workspace = workspace.resolve()
    workspace.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ, GIT_TERMINAL_PROMPT='0', GIT_CONFIG_COUNT='1',
                       GIT_CONFIG_KEY_0='credential.helper', GIT_CONFIG_VALUE_0='')
    if token := config.get('token'):
        environment.update(GIT_CONFIG_COUNT='2', GIT_CONFIG_KEY_1=f'http.{origin}/.extraHeader',
                           GIT_CONFIG_VALUE_1='Authorization: Basic ' + base64.b64encode(
                               ('magicmtn:' + token).encode()).decode())
    for repo in config['repos'][:10]:
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9-]{0,38}/[A-Za-z0-9_.-]{1,100}', repo):
            continue
        name = repo.split('/')[1]
        if name in ('.', '..') or name.lower().endswith('.git'):
            continue
        destination = workspace / name
        # Never overwrite a directory created by the user or a previous clone.
        if destination.exists() or destination.is_symlink():
            continue
        try:
            result = run(['git', 'clone', '--', f'{origin}/{repo}.git', str(destination)],
                         env=environment, timeout=120, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
            if result.returncode == 0:
                # Preserve normal GitHub remotes for the runtime's URL rewrite.
                run(['git', '-C', str(destination), 'remote', 'set-url', 'origin',
                     f'https://github.com/{repo}.git'], env=environment, timeout=5,
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except (OSError, subprocess.TimeoutExpired):
            continue


if __name__ == '__main__':
    apply(json.loads(base64.b64decode(sys.argv[1])))
