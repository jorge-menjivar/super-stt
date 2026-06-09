#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Generate a local `index.json` for offline registry testing.

Reads one or more backend `backend.toml` files, finds each declared
`[assets].wasm` / `[[assets.subprocess]]` artifact already staged in the
output directory, computes its real SHA-256 + size, and writes an
`index.json` whose asset URLs point at a local static server. Serve the
output directory over HTTP and point the daemon at it:

    just serve-test-registry          # builds, stages, generates, serves
    # then, in the daemon's environment:
    export SUPER_STT_REGISTRY_URL=http://localhost:8787/index.json

This exercises the daemon's real fetch + install pipeline (download, SHA-256
verification, extraction) without any GitHub release or Pages setup.

Requires Python 3.11+ (stdlib `tomllib`).
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import sys
import tomllib
from pathlib import Path

ONLINE_PROVIDERS = {"openai", "mistral", "deepgram", "anthropic"}
GPU_DEVICES = {"cuda", "metal", "rocm"}


def sha256_and_size(path: Path) -> tuple[str, int]:
    h = hashlib.sha256()
    size = 0
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 16), b""):
            h.update(chunk)
            size += len(chunk)
    return h.hexdigest(), size


def model_entry(m: dict) -> dict:
    return {
        "name": m["name"],
        "provider": m["provider"],
        "supported_devices": m.get("supported_devices", []),
    }


PLACEHOLDER_SHA = "0" * 64


def build_entry(toml_path: Path, out_dir: Path, base_url: str, allow_missing: bool) -> dict:
    with toml_path.open("rb") as f:
        manifest = tomllib.load(f)

    backend = manifest["backend"]
    models = manifest.get("models", [])
    assets = manifest.get("assets", {})

    online = any(m.get("provider") in ONLINE_PROVIDERS for m in models)
    supports_gpu = any(
        d in GPU_DEVICES for m in models for d in m.get("supported_devices", [])
    )
    supports_cpu = any(
        "cpu" in m.get("supported_devices", []) for m in models
    )

    index_assets: dict = {}
    if backend["kind"] == "wasm":
        asset_name = assets["wasm"]
        staged = out_dir / asset_name
        if staged.exists():
            digest, size = sha256_and_size(staged)
        elif allow_missing:
            # No real binary on disk; emit a placeholder so the index is still
            # readable/listable. Install would fail hash verification, which is
            # fine for listing-only / read tests.
            digest, size = PLACEHOLDER_SHA, 0
        else:
            sys.exit(f"error: staged asset {staged} not found (build + stage it first)")
        index_assets["wasm"] = {
            "url": f"{base_url}/{asset_name}",
            "size": size,
            "sha256": digest,
        }
    elif backend["kind"] == "subprocess":
        sub = []
        for a in assets.get("subprocess", []):
            staged = out_dir / a["file"]
            if staged.exists():
                digest, size = sha256_and_size(staged)
            elif allow_missing:
                digest, size = PLACEHOLDER_SHA, 0
            else:
                # Skip variants whose artifact isn't staged locally; a test box
                # rarely has every CUDA build on disk.
                continue
            entry = {
                "target": a["target"],
                "accel": a["accel"],
                "url": f"{base_url}/{a['file']}",
                "size": size,
                "sha256": digest,
            }
            for k in ("cuda_major", "cuda_sm"):
                if k in a:
                    entry[k] = a[k]
            if a.get("cudnn"):
                entry["cudnn"] = True
            sub.append(entry)
        if not sub:
            sys.exit(
                f"error: no subprocess assets for {backend['source']} staged in {out_dir}"
            )
        index_assets["subprocess"] = sub
    else:
        sys.exit(f"error: unknown kind {backend['kind']!r} in {toml_path}")

    version = backend["version"]
    entry = {
        # Use the last path segment of source as the id, matching how the
        # registry keys entries.
        "id": backend["source"].rstrip("/").rsplit("/", 1)[-1],
        "source": backend["source"],
        "version": version,
        "tag": f"v{version}",
        "name": backend["name"],
        "license": backend.get("license", "GPL-3.0-only"),
        "kind": backend["kind"],
        "contract": backend["contract"],
        "entrypoint": backend["entrypoint"],
        "allowed_hosts": manifest.get("network", {}).get("allowed_hosts", []),
        "online": online,
        "supports_gpu": supports_gpu,
        "supports_cpu": supports_cpu,
        "models": [model_entry(m) for m in models],
        "secrets": [
            {"name": s["name"], "label": s["label"], "required": s.get("required", False)}
            for s in manifest.get("secrets", [])
        ],
        "options": [
            {
                "name": o["name"],
                "label": o["label"],
                "type": o["type"],
                **({"default": o["default"]} if "default" in o else {}),
            }
            for o in manifest.get("options", [])
        ],
        "assets": index_assets,
    }
    if "description" in backend:
        entry["description"] = backend["description"]
    return entry


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, required=True, help="output dir (also where assets are staged)")
    ap.add_argument("--base-url", default="http://localhost:8787", help="URL the static server will serve the out dir at")
    ap.add_argument(
        "--allow-missing-assets",
        action="store_true",
        help="emit placeholder size/sha for assets not staged on disk (listing/read tests)",
    )
    ap.add_argument("manifests", nargs="+", type=Path, help="backend.toml paths to include")
    args = ap.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    base = args.base_url.rstrip("/")
    backends = [
        build_entry(m, args.out, base, args.allow_missing_assets) for m in args.manifests
    ]

    index = {
        "schema_version": 1,
        "generated_at": datetime.datetime.now(datetime.timezone.utc)
        .strftime("%Y-%m-%dT%H:%M:%SZ"),
        "min_client": "0.0.0",
        "backends": backends,
    }
    out_path = args.out / "index.json"
    out_path.write_text(json.dumps(index, indent=2) + "\n")
    print(f"wrote {out_path} ({len(backends)} backends)")


if __name__ == "__main__":
    main()
