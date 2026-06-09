#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Tests for gen_test_index.py — the offline registry index generator.

Run: python3 -m unittest registry.scripts.test_gen_test_index
 or: python3 registry/scripts/test_gen_test_index.py
"""

from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import gen_test_index as gen

HERE = Path(__file__).resolve().parent
DUMMY = HERE / "fixtures" / "dummy-backend.toml"


class GenTestIndex(unittest.TestCase):
    def _generate(self, out: Path, *, stage_asset: bool) -> dict:
        if stage_asset:
            (out / "dummy.wasm").write_bytes(b"\x00asm\x01\x00\x00\x00")
        entry = gen.build_entry(
            DUMMY, out, "http://localhost:8787", allow_missing=not stage_asset
        )
        return entry

    def test_shape_with_missing_asset(self):
        with tempfile.TemporaryDirectory() as d:
            entry = self._generate(Path(d), stage_asset=False)

        # Identity + metadata pulled straight from the manifest.
        self.assertEqual(entry["id"], "dummy")
        self.assertEqual(entry["source"], "github.com/jorge-menjivar/dummy")
        self.assertEqual(entry["version"], "1.2.3")
        self.assertEqual(entry["tag"], "v1.2.3")
        self.assertEqual(entry["kind"], "wasm")
        self.assertEqual(entry["entrypoint"], "dummy.wasm")
        self.assertEqual(entry["license"], "GPL-3.0-only")
        self.assertEqual(entry["allowed_hosts"], ["api.example.com"])
        self.assertEqual(entry["description"], "A fixture backend for offline registry tests.")

        # Derived booleans: one model is provider "openai" (online), one model
        # supports cpu+cuda.
        self.assertTrue(entry["online"])
        self.assertTrue(entry["supports_cpu"])
        self.assertTrue(entry["supports_gpu"])

        # Secrets / options carried through.
        self.assertEqual(entry["secrets"], [
            {"name": "dummy_api_key", "label": "Dummy API key", "required": True},
        ])
        self.assertEqual(entry["options"][0]["name"], "base_url")
        self.assertEqual(entry["options"][0]["default"], "https://api.example.com")

        # Placeholder asset: real URL, zero size, all-zero sha.
        wasm = entry["assets"]["wasm"]
        self.assertEqual(wasm["url"], "http://localhost:8787/dummy.wasm")
        self.assertEqual(wasm["size"], 0)
        self.assertEqual(wasm["sha256"], "0" * 64)

    def test_real_sha_when_asset_present(self):
        with tempfile.TemporaryDirectory() as d:
            out = Path(d)
            entry = self._generate(out, stage_asset=True)
            expected = hashlib.sha256((out / "dummy.wasm").read_bytes()).hexdigest()

        wasm = entry["assets"]["wasm"]
        self.assertEqual(wasm["sha256"], expected)
        self.assertEqual(wasm["size"], 8)  # len(b"\x00asm\x01\x00\x00\x00")

    def test_index_envelope_is_well_formed(self):
        # The full document the daemon will fetch: schema_version, generated_at,
        # min_client, backends[]. Build it the way main() does so the test
        # covers the serializable shape end to end.
        with tempfile.TemporaryDirectory() as d:
            out = Path(d)
            entry = self._generate(out, stage_asset=False)
            index = {
                "schema_version": 1,
                "generated_at": "2026-05-30T00:00:00Z",
                "min_client": "0.0.0",
                "backends": [entry],
            }
            text = json.dumps(index, indent=2)
            reparsed = json.loads(text)

        self.assertEqual(reparsed["schema_version"], 1)
        self.assertEqual(len(reparsed["backends"]), 1)
        self.assertEqual(reparsed["backends"][0]["id"], "dummy")


if __name__ == "__main__":
    unittest.main()
