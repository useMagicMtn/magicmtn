"""Release security checks use temporary keys and artifacts, never production credentials."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest import mock

import cli_release as release
import importlib.util

spec = importlib.util.spec_from_file_location("publish_cli", Path(__file__).with_name("publish-cli.py"))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class ReleaseTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="magic-release-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.private = self.root / "private.pem"
        self.private.write_bytes(release.command("openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256").stdout)
        self.private.chmod(0o600)
        self.configuration = {"public_key_pem": release.command("openssl", "pkey", "-in", self.private, "-pubout").stdout.decode(), "apple_team_id": "ABCDE12345"}
        self.trust = self.root / "trust.json"
        self.trust.write_text(json.dumps(self.configuration))
        now = int(time.time())
        self.binary = b"#!/bin/sh\ncase \"$1\" in --version) echo 'magic test';; completion) printf '%s\\n' \"$@\" > \"$HOME/completion-args\";; *) exit 1;; esac\n"
        self.payload = {"schema": 1, "release": "test-release", "channel": "beta", "source_commit": "a" * 40,
                        "issued_at": now - 1, "expires_at": now + 3600, "apple_team_id": "ABCDE12345",
                        "platforms": {name: hashlib.sha256(self.binary).hexdigest() for name in release.PLATFORMS}}
        self.signed = release.sign_payload(self.payload, self.private, self.configuration)

    def test_signature_tampering_and_wrong_keys_are_rejected_in_every_channel(self):
        for channel in release.CHANNELS:
            with self.subTest(channel=channel):
                payload = {**self.payload, "channel": channel}
                signed = release.sign_payload(payload, self.private, self.configuration)
                self.assertEqual(release.verify_envelope(signed, self.configuration)["channel"], channel)
                envelope = json.loads(signed)
                envelope["payload"] = base64.b64encode(release.canonical({**payload, "release": "attacker"})).decode()
                with self.assertRaises(subprocess.CalledProcessError):
                    release.verify_envelope(json.dumps(envelope), self.configuration)
        other = self.root / "other.pem"
        other.write_bytes(release.command("openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256").stdout)
        with self.assertRaises(subprocess.CalledProcessError):
            release.sign_payload(self.payload, other, self.configuration)

    def test_unsigned_expired_future_and_wrong_publisher_metadata_are_rejected(self):
        with self.assertRaises(ValueError):
            release.verify_envelope(json.dumps(self.payload), self.configuration)
        with self.assertRaises(ValueError):
            release.verify_envelope(self.signed, self.configuration, now=self.payload["expires_at"])
        with self.assertRaises(ValueError):
            release.verify_envelope(self.signed, self.configuration, now=self.payload["issued_at"] - 1000)
        with self.assertRaises(ValueError):
            release.sign_payload({**self.payload, "apple_team_id": "OTHER12345"}, self.private, self.configuration)

    def test_artifact_substitution_is_rejected(self):
        for platform in release.PLATFORMS:
            (self.root / ("magic-" + platform)).write_bytes(self.binary)
        release.verify_artifacts(self.payload, self.root)
        (self.root / "magic-linux-arm64").write_bytes(b"attacker")
        with self.assertRaises(ValueError):
            release.verify_artifacts(self.payload, self.root)

    def test_unconfigured_or_invalid_production_trust_cannot_publish(self):
        self.assertEqual(release.trust(self.trust), self.configuration)
        for configuration in [{"public_key_pem": "", "apple_team_id": ""}, {**self.configuration, "public_key_pem": "invalid"}]:
            self.trust.write_text(json.dumps(configuration))
            with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                release.trust(self.trust)

    def test_checksum_publication_requires_all_artifacts_before_upload(self):
        with mock.patch("sys.argv", ["publish-cli.py", "--skip-build", "--artifacts", str(self.root), "--output", str(self.root / "output")]), \
                mock.patch.object(publisher.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "Missing or empty artifact"):
                publisher.main()
            run.assert_not_called()

    def test_checksum_publication_promotes_only_a_complete_release(self):
        import types
        for platform in publisher.PLATFORMS:
            (self.root / ("magic-" + platform)).write_bytes(self.binary)
        output = self.root / "output"
        process = types.SimpleNamespace(
            run=mock.Mock(), check_output=mock.Mock(side_effect=["a" * 40, b"", "magic 1.0.0"]),
            CalledProcessError=subprocess.CalledProcessError, DEVNULL=subprocess.DEVNULL,
        )
        with mock.patch("sys.argv", ["publish-cli.py", "--skip-build", "--artifacts", str(self.root), "--output", str(output)]), \
                mock.patch.object(publisher, "subprocess", process), \
                mock.patch.object(publisher, "release_objects", return_value=[]), \
                mock.patch.object(publisher, "prune_releases"), \
                mock.patch.object(publisher, "publish_legacy_bridge"):
            publisher.main()
        destinations = [call.args[0][call.args[0].index("--key") + 1] if "--key" in call.args[0] else call.args[0][4] for call in process.run.call_args_list]
        self.assertTrue(any(url.endswith("/LICENSE") for url in destinations[:-2]))
        self.assertEqual((output / "LICENSE").read_bytes(), (publisher.ROOT / "LICENSE").read_bytes())
        for call in process.run.call_args_list[:-2]:
            self.assertIn("--if-none-match", call.args[0])
        self.assertTrue(destinations[-2].endswith("/latest.json"))
        self.assertTrue(destinations[-1].endswith("/install.sh"))
        manifest = json.loads((output / "release.json").read_text())
        for platform in publisher.PLATFORMS:
            digest = hashlib.sha256(self.binary).hexdigest()
            self.assertEqual(manifest["platforms"][platform], digest)
            self.assertEqual((output / ("magic-" + platform + ".sha256")).read_text(), digest + "  magic\n")
        self.assertIn(manifest["release"], (output / "install.sh").read_text())
        self.assertNotIn("@RELEASE@", (output / "install.sh").read_text())

    def test_failed_artifact_upload_does_not_promote_release(self):
        import types
        for platform in publisher.PLATFORMS:
            (self.root / ("magic-" + platform)).write_bytes(self.binary)
        process = types.SimpleNamespace(
            run=mock.Mock(side_effect=subprocess.CalledProcessError(1, "aws")),
            check_output=mock.Mock(side_effect=["a" * 40, b"", "magic 1.0.0"]),
            CalledProcessError=subprocess.CalledProcessError, DEVNULL=subprocess.DEVNULL,
        )
        with mock.patch("sys.argv", ["publish-cli.py", "--skip-build", "--artifacts", str(self.root), "--output", str(self.root / "output")]), \
                mock.patch.object(publisher, "subprocess", process), \
                mock.patch.object(publisher, "release_objects", return_value=[]), \
                mock.patch.object(publisher, "prune_releases"), \
                mock.patch.object(publisher, "publish_legacy_bridge"):
            with self.assertRaises(subprocess.CalledProcessError):
                publisher.main()
        for call in process.run.call_args_list:
            self.assertNotIn(call.args[0][4].rsplit("/", 1)[-1], ("latest.json", "install.sh"))

    def test_checksum_installer_preserves_existing_binary_on_mismatch(self):
        home = self.root / "checksum-home"
        target = home / ".local/bin"
        target.mkdir(parents=True)
        (target / "magic").write_bytes(b"existing installation")
        fixture = self.root / "checksum-fixture"
        fixture.mkdir()
        (fixture / "binary").write_bytes(b'#!/bin/sh\ntouch "$HOME/executed"\n')
        (fixture / "checksum").write_text("0" * 64 + "  magic\n")
        tools = self.root / "checksum-tools"
        tools.mkdir()
        (tools / "uname").write_text('#!/bin/sh\n[ "$1" = -s ] && echo Linux || echo x86_64\n')
        (tools / "curl").write_text(
            '#!/bin/sh\n'
            'case "$2" in\n'
            '  *.sha256) cp "$FIXTURE_DIR/checksum" "$4";;\n'
            '  */releases/*) cp "$FIXTURE_DIR/binary" "$4";;\n'
            '  *)\n'
            '    test "$*" = "-q -fsS --max-time 2 -o /dev/null -X POST -H X-MagicMtn-Installer: shell https://magicmtn.ai/install-completed" || exit 2\n'
            '    cmp "$HOME/.local/bin/magic" "$FIXTURE_DIR/binary" || exit 2\n'
            '    echo reported >> "$HOME/install-events"\n'
            '    exit 22;;\n'
            'esac\n'
        )
        for tool in tools.iterdir():
            tool.chmod(0o755)
        script = self.root / "checksum-install.sh"
        script.write_text((publisher.ROOT / "scripts/install-checksum.sh").read_text().replace("@RELEASE@", "test-release"))
        env = {**os.environ, "HOME": str(home), "FIXTURE_DIR": str(fixture), "PATH": str(tools) + os.pathsep + os.environ["PATH"]}
        result = subprocess.run(["sh", str(script), "--no-modify-shell"], env=env, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((target / "magic").read_bytes(), b"existing installation")
        self.assertFalse((home / "executed").exists())
        self.assertFalse((home / "install-events").exists())
        (fixture / "binary").write_bytes(self.binary)
        (fixture / "checksum").write_text(hashlib.sha256(self.binary).hexdigest() + "  magic\n")
        result = subprocess.run(["sh", str(script), "--no-modify-shell"], env=env, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((target / "magic").read_bytes(), self.binary)
        self.assertEqual((home / "install-events").read_text(), "reported\n")
        (target / "magic").unlink()
        (target / "magic").mkdir()
        result = subprocess.run(["sh", str(script), "--no-modify-shell"], env=env, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list((target / "magic").iterdir()), [])
        self.assertEqual((home / "install-events").read_text(), "reported\n")

    def test_bootstrap_must_match_the_independent_application_deployment(self):
        for body, location in [(b"attacker", release.INSTALLER_URL), (release.render_installer(self.configuration).encode(), "https://untrusted.example/install.sh")]:
            response = mock.MagicMock()
            response.__enter__.return_value = response
            response.read.return_value = body
            response.geturl.return_value = location
            with mock.patch.object(release.urllib.request, "urlopen", return_value=response):
                with self.assertRaises(ValueError):
                    release.verify_bootstrap(self.configuration)
        response = mock.MagicMock()
        response.__enter__.return_value = response
        response.read.return_value = release.render_installer(self.configuration).encode()
        response.geturl.return_value = release.INSTALLER_URL
        with mock.patch.object(release.urllib.request, "urlopen", return_value=response):
            release.verify_bootstrap(self.configuration)

    def test_local_notarization_receipt_cannot_replace_apple_acceptance(self):
        import zipfile
        import types
        binary = self.root / "magic-darwin-arm64"
        binary.write_bytes(self.binary)
        archive = self.root / "magic-darwin-arm64.zip"
        with zipfile.ZipFile(archive, "w") as zipped:
            zipped.write(binary, "magic")
        receipt = self.root / "notarization.json"
        receipt.write_text(json.dumps({"id":"submission", "status":"Accepted", "binary_sha256":release.digest(binary), "archive_sha256":release.digest(archive)}))
        for status, archive_hash, accepted in [("Invalid", release.digest(archive), False), ("Accepted", "0"*64, False), ("Accepted", release.digest(archive), True)]:
            def apple(*arguments):
                data = {"status":status, "sha256":archive_hash}
                return types.SimpleNamespace(stdout=json.dumps(data).encode())
            with mock.patch.object(release, "verify_macos_signature"), mock.patch.object(release, "notary_credentials", return_value=[]), mock.patch.object(release, "command", side_effect=apple):
                if accepted:
                    release.verify_notarization(binary, receipt, self.configuration)
                else:
                    with self.assertRaises(ValueError):
                        release.verify_notarization(binary, receipt, self.configuration)

    def test_relative_install_directory_is_rejected(self):
        script = self.root / "install.sh"
        script.write_text(release.render_installer(self.configuration))
        env = {**os.environ, "HOME": str(self.root), "MAGIC_INSTALL_DIR":"relative"}
        result = subprocess.run(["sh",str(script)], env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("absolute path", result.stderr)

    def installer(self, signed=None, binary=None, installed=b"old installation\n", consent="--no-modify-shell", directory=None):
        home = self.root / ("home-" + str(time.monotonic_ns()))
        home.mkdir()
        target = Path(directory) if directory else home / ".local/bin"
        target.mkdir(parents=True, exist_ok=True)
        (target / "magic").write_bytes(installed)
        tools = self.root / "tools"
        tools.mkdir(exist_ok=True)
        (tools / "uname").write_text("#!/bin/sh\n[ \"$1\" = -s ] && echo Linux || echo x86_64\n")
        (tools / "curl").write_text("#!/bin/sh\ncase \"$2\" in */latest.json) cp \"$FIXTURE_DIR/latest.json\" \"$4\";; */releases/test-release/magic-linux-x86_64) cp \"$FIXTURE_DIR/binary\" \"$4\";; *) exit 5;; esac\n")
        for path in tools.iterdir():
            path.chmod(0o755)
        fixture = self.root / "fixture"
        fixture.mkdir(exist_ok=True)
        (fixture / "latest.json").write_text(self.signed if signed is None else signed)
        (fixture / "binary").write_bytes(self.binary if binary is None else binary)
        script = self.root / "install.sh"
        script.write_text(release.render_installer(self.configuration))
        env = {**os.environ, "HOME": str(home), "PATH": str(tools) + os.pathsep + os.environ["PATH"], "FIXTURE_DIR": str(fixture)}
        if directory:
            env["MAGIC_INSTALL_DIR"] = str(directory)
        result = subprocess.run(["sh", str(script), *([consent] if consent else [])], env=env, capture_output=True, text=True)
        return result, home, target

    def test_installer_authenticates_before_execution_and_preserves_existing_binary(self):
        attacks = [json.dumps(self.payload), self.signed.replace('"signature":"', '"signature":"AAAA'),
                   json.dumps({**json.loads(self.signed), "payload": base64.b64encode(b"{}").decode()})]
        for signed in attacks:
            result, home, target = self.installer(signed=signed)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual((target / "magic").read_bytes(), b"old installation\n")
            self.assertFalse((home / "completion-args").exists())
        result, home, target = self.installer(binary=b"#!/bin/sh\ntouch \"$HOME/executed\"\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((home / "executed").exists())
        self.assertEqual((target / "magic").read_bytes(), b"old installation\n")

    def test_installer_signed_success_paths_and_shell_consent(self):
        result, home, target = self.installer()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((target / "magic").read_bytes(), self.binary)
        self.assertFalse((home / "completion-args").exists())
        for consent, suffix in [(None, ""), ("--yes", "--yes\n")]:
            result, home, target = self.installer(consent=consent)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual((home / "completion-args").read_text(), "completion\n--path\n" + str(target) + "\n" + suffix)
        result, _, target = self.installer(directory=self.root / "custom directory")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((target / "magic").exists())

    def test_installer_rejects_expired_signed_metadata(self):
        result, _, target = self.installer(signed=self.signed.replace('"payload":"', '"payload":"AAAA'))
        self.assertNotEqual(result.returncode, 0)
        with mock.patch.object(release.time, "time", return_value=self.payload["expires_at"] - 3601):
            expired = release.sign_payload({**self.payload, "issued_at": int(time.time()), "expires_at": int(time.time()) + 1}, self.private, self.configuration)
        result, _, target = self.installer(signed=expired)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((target / "magic").read_bytes(), b"old installation\n")


if __name__ == "__main__":
    unittest.main()
