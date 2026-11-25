"""Version ordering and five-release retention checks without AWS credentials."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import cli_version

spec = importlib.util.spec_from_file_location("publisher", Path(__file__).with_name("publish-cli.py"))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class VersionTest(unittest.TestCase):
    def test_stable_versions_and_numeric_ordering(self):
        self.assertGreater(cli_version.parse("1.10.0"), cli_version.parse("1.9.99"))
        for version in ("01.0.0", "1.0", "v1.0.0", "1.0.0-beta", "1.0.0/evil", "1.0.0\n"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                cli_version.parse(version)

    def test_bump_updates_only_cli_package_and_lock_entry(self):
        for part, expected in (("patch", "1.2.4"), ("minor", "1.3.0"), ("major", "2.0.0")):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "cli").mkdir()
                (root / "cli/Cargo.toml").write_text('[package]\nversion = "1.2.3"\n')
                other = '[[package]]\nname = "other"\nversion = "1.2.3"\n'
                (root / "Cargo.lock").write_text('[[package]]\nname = "magicmtn-cli"\nversion = "1.2.3"\n' + other)
                self.assertEqual(cli_version.bump(part, root), expected)
                self.assertEqual(cli_version.current(root), expected)
                self.assertIn(other, (root / "Cargo.lock").read_text())

    def test_rejects_reuse_partial_uploads_and_older_versions(self):
        for key, version in (("releases/1.0.0/magic-darwin-arm64", "1.0.0"), ("releases/1.10.0/release.json", "1.9.0")):
            with self.assertRaises(ValueError):
                publisher.validate_publication(version, [{"Key": key}])
        publisher.validate_publication("1.0.0", [{"Key": "releases/20261124-beta-123/release.json"}])

    def test_retains_current_and_four_newest_completed_releases(self):
        objects = [{"Key": f"releases/1.0.{index}/release.json", "LastModified": f"2026-11-{index + 1:02d}"} for index in range(7)]
        objects.append({"Key": "releases/incomplete/magic-darwin-arm64", "LastModified": "2026-11-24"})
        with mock.patch.object(publisher, "release_objects", return_value=objects), mock.patch.object(publisher.subprocess, "check_output", return_value='{"release":"1.0.6"}'), mock.patch.object(publisher.subprocess, "run") as run:
            publisher.prune_releases(["aws"], "1.0.6")
        deleted = [call.args[0][3] for call in run.call_args_list]
        self.assertEqual(deleted, [f"s3://{publisher.BUCKET}/releases/{name}/" for name in ("1.0.0", "1.0.1", "incomplete")])

    def test_missing_current_metadata_prevents_any_deletion(self):
        with mock.patch.object(publisher, "release_objects", return_value=[]), mock.patch.object(publisher.subprocess, "check_output", return_value='{"release":"1.0.0"}'), mock.patch.object(publisher.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                publisher.prune_releases(["aws"], "1.0.0")
            run.assert_not_called()

    def test_stale_cleanup_cannot_delete_a_newer_promoted_release(self):
        with mock.patch.object(publisher.subprocess, "check_output", return_value='{"release":"1.1.0"}'), mock.patch.object(publisher.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "live latest"):
                publisher.prune_releases(["aws"], "1.0.0")
            run.assert_not_called()

    def test_wrong_built_version_and_dirty_source_cannot_prepare(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for platform in publisher.PLATFORMS:
                (root / ("magic-" + platform)).write_bytes(b"binary")
            for results, message in ((["a" * 40, b"dirty"], "Commit CLI source"), (["a" * 40, b"", "magic 0.1.0"], "expected magic")):
                with mock.patch.object(publisher.subprocess, "check_output", side_effect=results):
                    with self.assertRaisesRegex(ValueError, message):
                        publisher.prepare(root, root / "output", "stable")
                self.assertFalse((root / "output").exists())

    def test_list_uses_paginated_cli_without_max_items(self):
        with mock.patch.object(publisher.subprocess, "check_output", return_value=json.dumps({"Contents": []})) as output:
            self.assertEqual(publisher.release_objects(["aws"]), [])
        self.assertNotIn("--no-paginate", output.call_args.args[0])
        self.assertNotIn("--max-items", output.call_args.args[0])
