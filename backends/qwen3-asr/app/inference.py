# SPDX-License-Identifier: GPL-3.0-only
"""Qwen3-ASR inference wrapper.

torch / qwen_asr are imported lazily inside methods so the server can bind its
socket and answer /v1/ping before those heavy imports run.
"""

from __future__ import annotations

import os
from dataclasses import dataclass

PROVIDER = "local_qwen3_asr"

# Wire model name -> directory under the backend dir (the manifest `dest`).
MODELS: dict[str, str] = {
    "qwen3-asr-0.6b": "models/qwen3-asr-0.6b",
    "qwen3-asr-1.7b": "models/qwen3-asr-1.7b",
}

# The 30 languages the Qwen3-ASR family accepts (ISO codes from the model card).
SUPPORTED_LANGUAGES: frozenset[str] = frozenset(
    {
        "zh",
        "en",
        "yue",
        "ar",
        "de",
        "fr",
        "es",
        "pt",
        "id",
        "it",
        "ko",
        "ru",
        "th",
        "vi",
        "ja",
        "tr",
        "hi",
        "ms",
        "nl",
        "sv",
        "da",
        "fi",
        "pl",
        "cs",
        "fil",
        "fa",
        "el",
        "hu",
        "mk",
        "ro",
    }
)


class DeviceUnavailable(Exception):
    """Requested device (e.g. cuda) could not be initialized."""


@dataclass
class TranscriptionResult:
    text: str
    language: str | None


class Engine:
    """Loads and runs a Qwen3-ASR model. Serves one model at a time."""

    def __init__(self, backend_dir: str) -> None:
        self._backend_dir = backend_dir
        self._model = None

    def load(self, name: str, device: str) -> str:
        """Load `name` onto `device` ('cpu' or 'cuda'); return the actual
        device. Raises DeviceUnavailable if cuda was requested but is absent."""
        # torch / qwen_asr are bundle-only (absent from the dev env) and are
        # imported lazily so /v1/ping answers before they load.
        import qwen_asr  # noqa: PLC0415  # ty: ignore[unresolved-import]
        import torch  # noqa: PLC0415  # ty: ignore[unresolved-import]

        dest = os.path.join(self._backend_dir, MODELS[name])
        if device == "cuda" and not torch.cuda.is_available():
            raise DeviceUnavailable("cuda requested but not available")
        device_map = "cuda:0" if device == "cuda" else "cpu"
        self._model = qwen_asr.Qwen3ASRModel.from_pretrained(
            dest,
            dtype=torch.bfloat16,
            device_map=device_map,
            max_new_tokens=256,
        )
        return "cuda" if device == "cuda" else "cpu"

    def transcribe(
        self, audio: list[float], sample_rate: int, language: str | None
    ) -> TranscriptionResult:
        if self._model is None:
            raise RuntimeError("transcribe called before a model was loaded")
        import numpy as np  # noqa: PLC0415

        samples = np.asarray(audio, dtype=np.float32)
        results = self._model.transcribe(
            audio=(samples, sample_rate), language=language
        )
        first = results[0]
        return TranscriptionResult(
            text=first.text, language=getattr(first, "language", None)
        )
