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
import textwrap
import types
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
        self.payload = {"schema": 1, "release": "1.1.2", "channel": "stable", "source_commit": "a" * 40,
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
        self.assertEqual(
            release.verify_envelope(self.signed, self.configuration, now=self.payload["expires_at"], allow_expired=True)["release"],
            self.payload["release"],
        )
        with self.assertRaises(ValueError):
            release.verify_envelope(self.signed, self.configuration, now=self.payload["issued_at"] - 1000)
        with self.assertRaises(ValueError):
            release.sign_payload({**self.payload, "apple_team_id": "OTHER12345"}, self.private, self.configuration)

    def test_project_key_only_release_and_installer_keep_signature_verification(self):
        self.configuration["apple_team_id"] = ""
        self.payload["apple_team_id"] = ""
        self.trust.write_text(json.dumps(self.configuration))
        self.assertEqual(release.trust(self.trust), self.configuration)
        self.signed = release.sign_payload(self.payload, self.private, self.configuration)
        with mock.patch.object(release, "notary_credentials", side_effect=AssertionError("Apple credentials must not be requested")):
            release.verify_notarization(self.root / "unused", self.root / "unused-receipt", self.configuration)
            release.notarize(self.root / "unused", self.root / "unused-receipt", None, self.configuration)
        result, _, target = self.installer()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((target / "magic").read_bytes(), self.binary)
        self.assertIn('version "1.1.2"', self.homebrew_formula(self.signed))
        result, _, target = self.installer(signed=self.signed.replace('"signature":"', '"signature":"AAAA'))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((target / "magic").read_bytes(), b"old installation\n")

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

    def test_signed_publication_requires_all_artifacts_before_upload(self):
        with mock.patch("sys.argv", ["publish-cli.py", "--skip-build", "--artifacts", str(self.root), "--output", str(self.root / "output"), "--signing-key", str(self.private)]), \
                mock.patch.object(publisher.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "Missing or empty artifact"):
                publisher.main()
            run.assert_not_called()

    def test_signed_publication_promotes_only_a_complete_release(self):
        import types
        for platform in publisher.PLATFORMS:
            (self.root / ("magic-" + platform)).write_bytes(self.binary)
        output = self.root / "output"
        process = types.SimpleNamespace(
            run=mock.Mock(), check_output=mock.Mock(side_effect=["a" * 40, b"", f"magic {publisher.cli_version.current(publisher.ROOT)}"]),
            CalledProcessError=subprocess.CalledProcessError, DEVNULL=subprocess.DEVNULL,
        )
        with mock.patch("sys.argv", ["publish-cli.py", "--skip-build", "--artifacts", str(self.root), "--output", str(output), "--signing-key", str(self.private)]), \
                mock.patch.object(publisher, "subprocess", process), \
                mock.patch.object(publisher, "release_objects", return_value=[]), \
                mock.patch.object(publisher, "prune_releases"), \
                mock.patch.object(release, "trust", return_value=self.configuration), \
                mock.patch.object(release, "verify_notarization"), \
                mock.patch.object(release, "verify_bootstrap"):
            publisher.main()
        destinations = [call.args[0][call.args[0].index("--key") + 1] if "--key" in call.args[0] else call.args[0][4] for call in process.run.call_args_list]
        self.assertTrue(any(url.endswith("/LICENSE") for url in destinations[:-3]))
        self.assertEqual((output / "LICENSE").read_bytes(), (publisher.ROOT / "LICENSE").read_bytes())
        for call in process.run.call_args_list[:-3]:
            self.assertIn("--if-none-match", call.args[0])
        self.assertTrue(destinations[-3].endswith("/latest.signed.json"))
        self.assertTrue(destinations[-2].endswith("/latest.json"))
        self.assertTrue(destinations[-1].endswith("/install.sh"))
        manifest = json.loads((output / "release.json").read_text())
        for platform in publisher.PLATFORMS:
            digest = hashlib.sha256(self.binary).hexdigest()
            self.assertEqual(manifest["platforms"][platform], digest)
            self.assertEqual((output / ("magic-" + platform + ".sha256")).read_text(), digest + "  magic\n")
        signed = release.verify_envelope((output / "manifest.json").read_bytes(), self.configuration)
        self.assertEqual(signed["release"], manifest["release"])
        self.assertEqual(signed["platforms"], manifest["platforms"])
        self.assertEqual((output / "install.sh").read_text(), release.render_installer(self.configuration))
        self.assertTrue(any(str(output / "manifest.json") in call.args[0] for call in process.run.call_args_list[-3:]))

    def test_failed_artifact_upload_does_not_promote_release(self):
        import types
        for platform in publisher.PLATFORMS:
            (self.root / ("magic-" + platform)).write_bytes(self.binary)
        process = types.SimpleNamespace(
            run=mock.Mock(side_effect=subprocess.CalledProcessError(1, "aws")),
            check_output=mock.Mock(side_effect=["a" * 40, b"", f"magic {publisher.cli_version.current(publisher.ROOT)}"]),
            CalledProcessError=subprocess.CalledProcessError, DEVNULL=subprocess.DEVNULL,
        )
        with mock.patch("sys.argv", ["publish-cli.py", "--skip-build", "--artifacts", str(self.root), "--output", str(self.root / "output"), "--signing-key", str(self.private)]), \
                mock.patch.object(publisher, "subprocess", process), \
                mock.patch.object(publisher, "release_objects", return_value=[]), \
                mock.patch.object(publisher, "prune_releases"), \
                mock.patch.object(release, "trust", return_value=self.configuration), \
                mock.patch.object(release, "verify_notarization"), \
                mock.patch.object(release, "verify_bootstrap"):
            with self.assertRaises(subprocess.CalledProcessError):
                publisher.main()
        for call in process.run.call_args_list:
            self.assertNotIn(call.args[0][4].rsplit("/", 1)[-1], ("latest.signed.json", "latest.json", "install.sh"))

    def test_publication_fails_before_upload_when_signing_or_notarization_fails(self):
        for platform in publisher.PLATFORMS:
            (self.root / ("magic-" + platform)).write_bytes(self.binary)
        for gate in ("missing-key", "wrong-key", "notarization", "bootstrap"):
            with self.subTest(gate=gate):
                process = types.SimpleNamespace(
                    run=mock.Mock(), check_output=mock.Mock(side_effect=["a" * 40, b"", f"magic {publisher.cli_version.current(publisher.ROOT)}"]),
                    CalledProcessError=subprocess.CalledProcessError, DEVNULL=subprocess.DEVNULL,
                )
                signing_key = self.private
                if gate == "wrong-key":
                    signing_key = self.root / "other-key.pem"
                    signing_key.write_bytes(release.command("openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256").stdout)
                arguments = ["publish-cli.py", "--skip-build", "--artifacts", str(self.root), "--output", str(self.root / "output")]
                if gate != "missing-key":
                    arguments += ["--signing-key", str(signing_key)]
                with mock.patch("sys.argv", arguments), mock.patch.dict(os.environ, {"MAGICMTN_RELEASE_SIGNING_KEY_FILE":""}), \
                        mock.patch.object(publisher, "subprocess", process), \
                        mock.patch.object(release, "trust", return_value=self.configuration), \
                        mock.patch.object(release, "verify_notarization", side_effect=ValueError("notarization failed") if gate == "notarization" else None), \
                        mock.patch.object(release, "verify_bootstrap", side_effect=ValueError("bootstrap mismatch") if gate == "bootstrap" else None):
                    with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                        publisher.main()
                process.run.assert_not_called()

    def test_metadata_renewal_preserves_release_identity_and_checks_source(self):
        payload = {**self.payload, "release": publisher.cli_version.current(publisher.ROOT)}
        signed = release.sign_payload(payload, self.private, self.configuration)
        for dry_run in (True, False):
            with mock.patch.object(publisher, "subprocess", types.SimpleNamespace(check_output=mock.Mock(return_value=signed), run=mock.Mock())) as process, \
                    mock.patch.object(release, "trust", return_value=self.configuration), \
                    mock.patch.object(release, "verify_bootstrap"), \
                    mock.patch.object(release, "source_commit", return_value="a" * 40):
                publisher.refresh_metadata(["aws"], self.root / "renewed", self.private, dry_run=dry_run)
            if dry_run:
                process.run.assert_not_called()
            else:
                process.run.assert_called_once()
                self.assertEqual(process.run.call_args.args[0][4], f"s3://{publisher.BUCKET}/latest.signed.json")
        renewed = release.verify_envelope((self.root / "renewed/manifest.json").read_bytes(), self.configuration)
        for key in ("release", "source_commit", "platforms", "apple_team_id"):
            self.assertEqual(renewed[key], payload[key])
        with mock.patch.object(publisher, "subprocess", types.SimpleNamespace(check_output=mock.Mock(return_value=signed), run=mock.Mock())) as process, \
                mock.patch.object(release, "trust", return_value=self.configuration), \
                mock.patch.object(release, "source_commit", return_value="b" * 40):
            with self.assertRaisesRegex(ValueError, "published source"):
                publisher.refresh_metadata(["aws"], self.root / "rejected", self.private)
        process.run.assert_not_called()

    def homebrew_formula(self, signed):
        source = (publisher.ROOT / "release/cli-homebrew.yml").read_text()
        source = source.split("python3 - <<'PY'\n", 1)[1].split("\n                  PY", 1)[0]
        program = textwrap.dedent(source).split("def github(", 1)[0]
        def fetch(url, **kwargs):
            response = mock.MagicMock()
            response.__enter__.return_value = response
            response.geturl.return_value = url
            response.read.return_value = release.render_installer(self.configuration).encode() if url.endswith("/install.sh") else signed.encode()
            return response
        namespace = {}
        with mock.patch.dict(os.environ, {"RELEASE_URL": release.DOWNLOADS}), \
                mock.patch.object(release.urllib.request, "urlopen", side_effect=fetch):
            exec(compile(program, "cli-homebrew.yml", "exec"), namespace)
        return namespace["formula"].decode()

    def test_homebrew_authenticates_metadata_with_the_independent_bootstrap_key(self):
        formula = self.homebrew_formula(self.signed)
        self.assertIn('version "1.1.2"', formula)
        self.assertIn('version_scheme 2', formula)
        self.assertIn(hashlib.sha256(self.binary).hexdigest(), formula)
        for invalid in (json.dumps(self.payload), self.signed.replace('"signature":"', '"signature":"AAAA')):
            with self.assertRaises((ValueError, KeyError, subprocess.CalledProcessError)):
                self.homebrew_formula(invalid)

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
        (tools / "curl").write_text("#!/bin/sh\ncase \"$2\" in */latest.signed.json) cp \"$FIXTURE_DIR/latest.signed.json\" \"$4\";; */releases/1.1.2/magic-linux-x86_64) cp \"$FIXTURE_DIR/binary\" \"$4\";; *) exit 5;; esac\n")
        for path in tools.iterdir():
            path.chmod(0o755)
        fixture = self.root / "fixture"
        fixture.mkdir(exist_ok=True)
        (fixture / "latest.signed.json").write_text(self.signed if signed is None else signed)
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
