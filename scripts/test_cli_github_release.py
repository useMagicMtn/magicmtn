"""GitHub release checks use temporary keys and mock every GitHub write."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest import mock

import cli_release as release

spec = importlib.util.spec_from_file_location("github_cli_release", Path(__file__).with_name("github-cli-release.py"))
github = importlib.util.module_from_spec(spec)
spec.loader.exec_module(github)


class GitHubReleaseTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="magic-github-test-")
        self.addCleanup(temporary.cleanup)
        private = Path(temporary.name) / "private.pem"
        private.write_bytes(release.command("openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256").stdout)
        self.configuration = {"public_key_pem": release.command("openssl", "pkey", "-in", private, "-pubout").stdout.decode(), "apple_team_id": ""}
        now = int(time.time())
        self.payload = {"schema": 1, "release": "1.1.2", "channel": "stable", "source_commit": "a" * 40,
                        "issued_at": now - 1, "expires_at": now + 3600, "apple_team_id": "",
                        "platforms": {platform: str(index) * 64 for index, platform in enumerate(release.PLATFORMS)}}
        self.signed = release.sign_payload(self.payload, private, self.configuration).encode()
        self.published = {**self.payload, "version": "1.1.2", "source_dirty": False}
        self.checksums = "".join(digest + "  magic-" + platform + "\n" for platform, digest in self.payload["platforms"].items()).encode()

    def prepare(self, version="1.1.2", signed=None, metadata=None, checksums=None):
        responses = {"latest.signed.json": self.signed if signed is None else signed,
                     "releases/1.1.2/release.json": json.dumps(self.published if metadata is None else metadata).encode(),
                     "releases/1.1.2/SHA256SUMS": self.checksums if checksums is None else checksums}
        with mock.patch.object(github, "download", side_effect=responses.__getitem__), \
                mock.patch.object(release, "trust", return_value=self.configuration):
            return github.prepare(version, "User-visible changes.")

    def test_notes_link_all_platforms_and_the_authenticated_source(self):
        payload, body = self.prepare()
        self.assertEqual(payload, self.payload)
        for platform, digest in self.payload["platforms"].items():
            self.assertIn(release.DOWNLOADS + "/releases/1.1.2/magic-" + platform, body)
            self.assertIn(digest, body)
        self.assertIn("/commit/" + "a" * 40, body)
        self.assertIn("User-visible changes.", body)
        self.assertIn("manifest.json", body)

    def test_tampered_expired_and_wrong_version_metadata_are_rejected(self):
        with self.assertRaises((ValueError, subprocess.CalledProcessError)):
            self.prepare(signed=self.signed.replace(b'"signature":"', b'"signature":"AAAA'))
        with mock.patch.object(release.time, "time", return_value=self.payload["expires_at"]):
            with self.assertRaises(ValueError):
                self.prepare()
        with self.assertRaises(ValueError):
            self.prepare(version="1.1.3")

    def test_unsigned_metadata_and_checksum_substitution_are_rejected(self):
        for key, value in [("source_commit", "b" * 40), ("platforms", {}), ("source_dirty", True), ("version", "1.1.3")]:
            with self.subTest(field=key), self.assertRaises(ValueError):
                self.prepare(metadata={**self.published, key: value})
        for checksums in [self.checksums.replace(b"0" * 64, b"f" * 64), self.checksums + self.checksums, b""]:
            with self.assertRaises(ValueError):
                self.prepare(checksums=checksums)

    def test_create_uses_signed_source_commit_and_reviewed_notes_file(self):
        def create(arguments, **kwargs):
            self.assertEqual(arguments[:4], ["gh", "release", "create", "v1.1.2"])
            self.assertEqual(arguments[arguments.index("--target") + 1], "a" * 40)
            self.assertEqual(Path(arguments[arguments.index("--notes-file") + 1]).read_text(), "Reviewed notes.")
        with mock.patch.object(github, "api", return_value=None), \
                mock.patch.object(subprocess, "run", side_effect=create) as run:
            github.publish(self.payload, "Reviewed notes.")
            self.assertEqual(run.call_count, 1)

    def test_conflicting_lightweight_and_annotated_tags_are_rejected(self):
        wrong = {"type": "commit", "sha": "b" * 40}
        for answers in [[{"object": wrong}], [{"object": {"type": "tag", "sha": "c" * 40}}, {"object": wrong}]]:
            with mock.patch.object(github, "api", side_effect=answers), \
                    mock.patch.object(subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    github.publish(self.payload, "notes")
                run.assert_not_called()

    def test_matching_release_is_a_no_op_and_changed_notes_are_rejected(self):
        reference = {"object": {"type": "commit", "sha": "a" * 40}}
        existing = {"draft": False, "prerelease": False, "body": "notes", "html_url": "https://github.com/example/release"}
        for body in ["notes", "different notes"]:
            with mock.patch.object(github, "api", side_effect=[reference, existing]), \
                    mock.patch.object(subprocess, "run") as run:
                if body == "notes":
                    github.publish(self.payload, body)
                else:
                    with self.assertRaises(ValueError):
                        github.publish(self.payload, body)
                run.assert_not_called()

    def test_api_errors_are_not_treated_as_missing_releases(self):
        for message, missing in [("gh: Not Found (HTTP 404)", True), ("gh: Forbidden (HTTP 403)", False), ("connection failed", False)]:
            result = subprocess.CompletedProcess(["gh"], 1, "", message)
            with mock.patch.object(subprocess, "run", return_value=result):
                if missing:
                    self.assertIsNone(github.api("releases/tags/v1.1.2"))
                else:
                    with self.assertRaises(subprocess.CalledProcessError):
                        github.api("releases/tags/v1.1.2")

    def test_dry_run_previews_without_calling_github(self):
        with mock.patch("sys.argv", ["github-cli-release.py", "--version", "1.1.2", "--dry-run"]), \
                mock.patch.object(github, "notes_for", return_value="notes"), \
                mock.patch.object(github, "prepare", return_value=(self.payload, "preview")), \
                mock.patch.object(github, "publish") as publish, mock.patch("builtins.print") as output:
            github.main()
            output.assert_called_once_with("preview")
            publish.assert_not_called()

    def test_release_notes_must_exist_and_be_nonempty(self):
        with tempfile.TemporaryDirectory() as temporary, mock.patch.object(github, "ROOT", Path(temporary)):
            with self.assertRaises(FileNotFoundError):
                github.notes_for("1.1.3")
            directory = Path(temporary) / "release/notes"
            directory.mkdir(parents=True)
            (directory / "1.1.3.md").write_text("\n")
            with self.assertRaises(ValueError):
                github.notes_for("1.1.3")


if __name__ == "__main__":
    unittest.main()
