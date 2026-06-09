#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Assemble a self-contained, relocatable backend tarball for one accelerator
# using uv. Pure wheel assembly — no CUDA toolkit, no GPU, no native build.
#
# Layout produced (the tarball extracts into the daemon's backend dir):
#   bin/qwen3-asr     launcher: sets PYTHONPATH=runtime/site, execs the python
#   runtime/python/   relocatable CPython (uv-managed standalone build)
#   runtime/site/     installed packages (torch, qwen-asr, numpy, starlette, …)
#   app/              server.py, inference.py
#
# Usage: scripts/build_bundle.sh <cpu|cuda13> <output-dir>
set -euo pipefail

ACCEL="${1:?usage: build_bundle.sh <cpu|cuda13> <output-dir>}"
OUT="${2:?usage: build_bundle.sh <cpu|cuda13> <output-dir>}"

PY_SERIES="3.11"
TARGET="x86_64-unknown-linux-gnu"

case "$ACCEL" in
  cpu)    TORCH_BACKEND="cpu" ;;
  cuda13) TORCH_BACKEND="cu130" ;;
  *) echo "unknown accel: $ACCEL (expected cpu|cuda13)" >&2; exit 1 ;;
esac

command -v uv >/dev/null 2>&1 || {
  echo "uv is required to build the bundle: https://docs.astral.sh/uv/" >&2
  exit 1
}

# The staging dir is in /tmp; uv's cache is usually on another filesystem, so
# hardlinking into the bundle fails. Copy instead (silences the warning).
export UV_LINK_MODE=copy

HERE="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# 1. Relocatable CPython, installed INTO the bundle. uv resolves a valid patch
#    release of the series, so there is no hardcoded download URL to go stale.
export UV_PYTHON_INSTALL_DIR="$STAGE/runtime/pythons"
uv python install --no-bin "$PY_SERIES"
# Exactly one interpreter was installed; give it a stable, version-independent
# path so the launcher can hardcode runtime/python.
mv "$STAGE"/runtime/pythons/cpython-*/ "$STAGE/runtime/python"
rm -rf "$STAGE/runtime/pythons"
PY="$STAGE/runtime/python/bin/python3"

# 2. Dependencies into a relocatable site dir (the launcher adds it to
#    PYTHONPATH). --torch-backend selects the matching PyTorch wheel index;
#    everything else resolves from PyPI in the same pass.
uv pip install \
    --python "$PY" \
    --target "$STAGE/runtime/site" \
    --torch-backend "$TORCH_BACKEND" \
    torch qwen-asr numpy starlette uvicorn

# 3. App + launcher.
cp -r "$HERE/app" "$STAGE/app"
mkdir -p "$STAGE/bin"
cp "$HERE/bin/qwen3-asr" "$STAGE/bin/qwen3-asr"
chmod +x "$STAGE/bin/qwen3-asr"

# 4. Prune to shrink the tarball.
find "$STAGE/runtime" -depth -type d -name "__pycache__" -exec rm -rf {} +
find "$STAGE/runtime/site" -depth -type d \( -name tests -o -name test \) -exec rm -rf {} +
rm -rf "$STAGE/runtime/python/lib/python"*/idlelib \
       "$STAGE/runtime/python/lib/python"*/tkinter \
       "$STAGE/runtime/python/lib/python"*/ensurepip 2>/dev/null || true

# 5. Tarball; print the size (mind the GitHub release per-asset limit for cuda).
mkdir -p "$OUT"
TARBALL="$OUT/qwen3-asr-${TARGET}-${ACCEL}.tar.gz"
tar -C "$STAGE" -czf "$TARBALL" bin runtime app
ls -lh "$TARBALL"
echo "built $TARBALL"
