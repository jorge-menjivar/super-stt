# SPDX-License-Identifier: GPL-3.0-only
"""ASGI server implementing the /v1 backend contract over a Unix socket."""

from __future__ import annotations

import asyncio
import json
import os
import threading

from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, StreamingResponse
from starlette.routing import Route

from inference import (
    MODELS,
    PROVIDER,
    SUPPORTED_LANGUAGES,
    DeviceUnavailable,
    Engine,
)


class State:
    """Readiness state machine for one backend process."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.state = "starting"  # starting | loading | ready | error
        self.progress: float | None = None
        self.model: dict[str, str] | None = None
        self.device: str | None = None
        self.reason: str | None = None
        self.busy = False  # a transcription is in flight
        # Best-effort cancellation signal. /v1/cancel sets this; an in-flight
        # Engine.transcribe call running in the thread pool is NOT interrupted
        # mid-call (it does not poll this flag), so a cancel arriving during
        # inference lets that call finish and its result is still returned. The
        # daemon's repeated-pass model tolerates such a stale pass.
        self.cancel = threading.Event()


def _sse(event: str, data: dict) -> str:
    return f"event: {event}\ndata: {json.dumps(data)}\n\n"


def create_app(engine: Engine | None = None) -> Starlette:
    backend_dir = os.environ.get("SUPER_STT_BACKEND_DIR", ".")
    eng = engine if engine is not None else Engine(backend_dir)
    st = State()
    infer_lock = asyncio.Lock()

    async def ping(_: Request) -> JSONResponse:
        return JSONResponse({"status": "success", "message": "pong"})

    async def status(_: Request) -> JSONResponse:
        with st.lock:
            body: dict = {"status": "success", "state": st.state}
            if st.state == "loading":
                body["progress"] = st.progress if st.progress is not None else 0.0
            if st.model is not None:
                body["model"] = st.model
            if st.device is not None:
                body["device"] = st.device
            body["reason"] = st.reason
        return JSONResponse(body)

    async def load(request: Request) -> JSONResponse:
        data = await request.json()
        name = data.get("name")
        provider = data.get("provider")
        device = data.get("device", "cpu")
        if name not in MODELS or provider != PROVIDER:
            return JSONResponse(
                {"status": "error", "message": "invalid_model"}, status_code=400
            )
        with st.lock:
            if st.state == "loading":
                return JSONResponse(
                    {"status": "error", "message": "already_loading"},
                    status_code=409,
                )
            st.state = "loading"
            st.progress = 0.0
            st.model = {"name": name, "provider": provider}
            st.device = None
            st.reason = None

        def do_load() -> None:
            try:
                actual = eng.load(name, device)
                with st.lock:
                    st.device = actual
                    st.state = "ready"
                    st.progress = None
            except DeviceUnavailable:
                with st.lock:
                    st.state = "error"
                    st.reason = "device_unavailable"
                    st.progress = None
            except Exception:  # noqa: BLE001  (any load failure -> error state)
                with st.lock:
                    st.state = "error"
                    st.reason = "load_failed"
                    st.progress = None

        threading.Thread(target=do_load, daemon=True).start()
        return JSONResponse(
            {"status": "success", "message": "Loading started"}, status_code=202
        )

    async def transcribe(request: Request) -> JSONResponse | StreamingResponse:
        # State only advances starting -> loading -> ready/error (no reload
        # path), so a one-shot read of `state` here needs no further
        # coordination with /v1/load. Revisit if a reload endpoint is added.
        with st.lock:
            ready = st.state == "ready"
        if not ready:
            return JSONResponse(
                {"status": "error", "message": "not_ready"}, status_code=409
            )
        data = await request.json()
        audio = data.get("audio_data")
        if not audio:
            return JSONResponse(
                {"status": "error", "message": "invalid_audio"}, status_code=400
            )
        sample_rate = data.get("sample_rate", 16000)
        language = data.get("language")
        if language is not None and language not in SUPPORTED_LANGUAGES:
            return JSONResponse(
                {"status": "error", "message": "unsupported_language"},
                status_code=400,
            )
        options = data.get("options") or {}
        stream = bool(options.get("stream_realtime"))

        async def run() -> str:
            async with infer_lock:
                st.cancel.clear()
                with st.lock:
                    st.busy = True
                try:
                    result = await asyncio.to_thread(
                        eng.transcribe, audio, sample_rate, language
                    )
                    return result.text
                finally:
                    with st.lock:
                        st.busy = False

        if stream:

            async def gen():
                try:
                    text = await run()
                    yield _sse("done", {"transcription": text})
                except Exception:  # noqa: BLE001
                    yield _sse("error", {"message": "inference_failed"})

            return StreamingResponse(gen(), media_type="text/event-stream")

        try:
            text = await run()
        except Exception:  # noqa: BLE001
            return JSONResponse(
                {"status": "error", "message": "inference_failed"}, status_code=500
            )
        return JSONResponse({"status": "success", "transcription": text})

    async def cancel(_: Request) -> JSONResponse:
        # Best-effort (see State.cancel): acknowledges with 200 when a
        # transcription is in flight, 409 when nothing is running. The signal
        # cannot interrupt an Engine.transcribe call already in the thread pool.
        with st.lock:
            busy = st.busy
        if not busy:
            return JSONResponse(
                {"status": "error", "message": "nothing_in_progress"},
                status_code=409,
            )
        st.cancel.set()
        return JSONResponse({"status": "success", "message": "Cancelled"})

    return Starlette(
        routes=[
            Route("/v1/ping", ping, methods=["GET"]),
            Route("/v1/status", status, methods=["GET"]),
            Route("/v1/load", load, methods=["POST"]),
            Route("/v1/transcribe", transcribe, methods=["POST"]),
            Route("/v1/cancel", cancel, methods=["POST"]),
        ]
    )


def main() -> None:
    import uvicorn  # noqa: PLC0415

    socket = os.environ["SUPER_STT_BACKEND_SOCKET"]
    uvicorn.run(create_app(), uds=socket, log_level="warning")


if __name__ == "__main__":
    main()
