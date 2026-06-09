#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Generate a *bulk* dummy `index.json` with a few dozen varied backends.

Unlike `gen_test_index.py` (which builds a real index from actual
`backend.toml` files + staged binaries), this synthesizes a large, varied
catalog with placeholder assets — purely to exercise the app's Download tab:
listing, search, the online/kind filters, and the compatible/incompatible
toggle. Installs will fail hash verification (assets are placeholders); this
is for UI/listing testing only.

The mix is deliberate:
  * wasm online backends (always compatible)
  * subprocess local backends with an x86_64 CPU asset (compatible on x86_64)
  * subprocess backends that ship ONLY aarch64 or ONLY a single CUDA sm with
    no CPU fallback — these come back incompatible on a typical x86_64 host,
    so the "Show incompatible" toggle has something to reveal.

Usage:
    python3 registry/scripts/gen_dummy_registry.py --out /tmp/treg
    python3 -m http.server 8788 --directory /tmp/treg
    export SUPER_STT_REGISTRY_URL=http://localhost:8788/index.json
"""

from __future__ import annotations

import argparse
import datetime
import json
from pathlib import Path

PLACEHOLDER_SHA = "0" * 64


def wasm_asset(base: str, name: str) -> dict:
    return {"wasm": {"url": f"{base}/{name}.wasm", "size": 0, "sha256": PLACEHOLDER_SHA}}


def sub_asset(base: str, name: str, target: str, accel: str, **kw) -> dict:
    a = {"target": target, "accel": accel,
         "url": f"{base}/{name}-{target}-{accel}.tar.gz",
         "size": 0, "sha256": PLACEHOLDER_SHA}
    a.update(kw)
    return a


def backend(idx: int, name: str, *, kind: str, provider: str, online: bool,
            assets: dict, base: str, secret: bool = False,
            supports_cpu: bool = False, supports_gpu: bool = False,
            devices: list[str] | None = None) -> dict:
    slug = name.lower().replace(" ", "-").replace(".", "")
    entry = {
        "id": slug,
        "source": f"github.com/dummy/{slug}",
        "version": f"{1 + idx % 3}.{idx % 5}.{idx % 7}",
        "tag": f"v{1 + idx % 3}.{idx % 5}.{idx % 7}",
        "name": name,
        "description": f"Dummy {('online' if online else 'local')} backend #{idx} for UI testing.",
        "license": "Apache-2.0" if idx % 2 else "GPL-3.0-only",
        "kind": kind,
        "contract": "v1",
        "entrypoint": f"{slug}.wasm" if kind == "wasm" else slug,
        "allowed_hosts": [f"api.{provider}.com"] if online else [],
        "online": online,
        "supports_gpu": supports_gpu,
        "supports_cpu": supports_cpu,
        "models": [
            {"name": f"{slug}-base", "provider": provider,
             "supported_devices": devices or (["none"] if online else ["cpu"])},
        ],
        "secrets": (
            [{"name": f"{provider}_api_key", "label": f"{provider.title()} API key",
              "required": True}] if secret else []
        ),
        "options": [
            {"name": "base_url", "label": "API base URL", "type": "string",
             "default": f"https://api.{provider}.com"}
        ] if online else [],
        "assets": assets,
    }
    return entry


def build(base: str) -> list[dict]:
    out: list[dict] = []
    i = 0

    # 1) Online wasm backends — always compatible.
    online_specs = [
        ("OpenAI", "openai"), ("Mistral", "mistral"), ("Deepgram", "deepgram"),
        ("Anthropic", "anthropic"), ("OpenAI Whisper Cloud", "openai"),
        ("Mistral Voxtral Cloud", "mistral"), ("Deepgram Nova", "deepgram"),
        ("AssemblyAI", "openai"), ("Rev AI", "deepgram"), ("Speechmatics", "openai"),
        ("Gladia", "mistral"), ("Soniox", "deepgram"),
    ]
    for name, provider in online_specs:
        slug = name.lower().replace(" ", "-")
        out.append(backend(i, name, kind="wasm", provider=provider, online=True,
                           assets=wasm_asset(base, slug), base=base, secret=True))
        i += 1

    # 2) Local subprocess backends with an x86_64 CPU asset — compatible on x86_64.
    local_specs = [
        "Whisper Local", "Voxtral Mini", "Voxtral Small", "Parakeet",
        "Moonshine", "FasterWhisper", "WhisperX", "Distil-Whisper",
        "Canary", "NeMo CTC",
    ]
    for name in local_specs:
        slug = name.lower().replace(" ", "-").replace(".", "")
        assets = {"subprocess": [
            sub_asset(base, slug, "x86_64-unknown-linux-gnu", "cpu"),
            sub_asset(base, slug, "x86_64-unknown-linux-gnu", "cuda",
                      cuda_major=12, cuda_sm=86),
        ]}
        out.append(backend(i, name, kind="subprocess", provider="dummy", online=False,
                           assets=assets, base=base, supports_cpu=True, supports_gpu=True,
                           devices=["cpu", "cuda"]))
        i += 1

    # 3) Incompatible-on-x86_64 backends: aarch64-only, or CUDA-only with no CPU
    #    fallback. These let the "Show incompatible" toggle demonstrate itself.
    incompat_specs = [
        ("Whisper ARM", [sub_asset(base, "whisper-arm", "aarch64-unknown-linux-gnu", "cpu")]),
        ("Voxtral ARM", [sub_asset(base, "voxtral-arm", "aarch64-unknown-linux-gnu", "cpu")]),
        ("Parakeet ARM", [sub_asset(base, "parakeet-arm", "aarch64-unknown-linux-gnu", "cpu")]),
        ("Whisper SM75 Only", [sub_asset(base, "whisper-sm75", "x86_64-unknown-linux-gnu",
                                         "cuda", cuda_major=12, cuda_sm=75)]),
        ("Canary SM120 Only", [sub_asset(base, "canary-sm120", "x86_64-unknown-linux-gnu",
                                         "cuda", cuda_major=13, cuda_sm=120)]),
        ("NeMo ROCm Only", [sub_asset(base, "nemo-rocm", "x86_64-unknown-linux-gnu", "rocm")]),
    ]
    for name, assets in incompat_specs:
        out.append(backend(i, name, kind="subprocess", provider="dummy", online=False,
                           assets={"subprocess": assets}, base=base,
                           supports_gpu=True, devices=["cuda"]))
        i += 1

    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, required=True, help="output dir for index.json")
    ap.add_argument("--base-url", default="http://localhost:8788")
    args = ap.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    backends = build(args.base_url.rstrip("/"))
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
