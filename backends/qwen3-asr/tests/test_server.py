# SPDX-License-Identifier: GPL-3.0-only
import json
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "app"))

from starlette.testclient import TestClient

import server
from inference import DeviceUnavailable, TranscriptionResult


class FakeEngine:
    def __init__(
        self,
        *,
        fail=None,
        text="hello world",
        lang="en",
        load_delay=0.0,
        infer_delay=0.0,
        infer_fail=False,
    ):
        self.fail = fail  # None | "device" | "load"
        self.text = text
        self.lang = lang
        self.load_delay = load_delay
        self.infer_delay = infer_delay
        self.infer_fail = infer_fail
        self.loaded = None

    def load(self, name, device):
        if self.load_delay:
            time.sleep(self.load_delay)
        if self.fail == "device":
            raise DeviceUnavailable("no cuda")
        if self.fail == "load":
            raise RuntimeError("boom")
        self.loaded = (name, device)
        return "cuda" if device == "cuda" else "cpu"

    def transcribe(self, audio, sample_rate, language):
        if self.infer_delay:
            time.sleep(self.infer_delay)
        if self.infer_fail:
            raise RuntimeError("inference boom")
        return TranscriptionResult(text=self.text, language=self.lang)


def make_client(engine):
    return TestClient(server.create_app(engine=engine))


def wait_settled(c, timeout=2.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        s = c.get("/v1/status").json()
        if s["state"] in ("ready", "error"):
            return s
        time.sleep(0.01)
    raise AssertionError("state never settled")


def load_ok(c, name="qwen3-asr-0.6b", device="cpu"):
    r = c.post(
        "/v1/load", json={"name": name, "provider": "local_qwen3_asr", "device": device}
    )
    assert r.status_code == 202, r.text
    return wait_settled(c)


def test_ping_before_load():
    c = make_client(FakeEngine())
    r = c.get("/v1/ping")
    assert r.status_code == 200
    assert r.json() == {"status": "success", "message": "pong"}


def test_status_starts_in_starting():
    c = make_client(FakeEngine())
    s = c.get("/v1/status").json()
    assert s["state"] == "starting"
    assert s["status"] == "success"


def test_load_then_ready_reports_device():
    c = make_client(FakeEngine())
    s = load_ok(c, device="cpu")
    assert s["state"] == "ready"
    assert s["device"] == "cpu"
    assert s["model"] == {"name": "qwen3-asr-0.6b", "provider": "local_qwen3_asr"}


def test_load_invalid_model():
    c = make_client(FakeEngine())
    r = c.post("/v1/load", json={"name": "nope", "provider": "local_qwen3_asr"})
    assert r.status_code == 400
    assert r.json()["message"] == "invalid_model"


def test_load_wrong_provider_invalid_model():
    c = make_client(FakeEngine())
    r = c.post("/v1/load", json={"name": "qwen3-asr-0.6b", "provider": "openai"})
    assert r.status_code == 400
    assert r.json()["message"] == "invalid_model"


def test_already_loading():
    c = make_client(FakeEngine(load_delay=0.3))
    r1 = c.post(
        "/v1/load", json={"name": "qwen3-asr-0.6b", "provider": "local_qwen3_asr"}
    )
    assert r1.status_code == 202
    assert c.get("/v1/status").json()["state"] == "loading"
    r2 = c.post(
        "/v1/load", json={"name": "qwen3-asr-0.6b", "provider": "local_qwen3_asr"}
    )
    assert r2.status_code == 409
    assert r2.json()["message"] == "already_loading"
    wait_settled(c)


def test_device_unavailable():
    c = make_client(FakeEngine(fail="device"))
    r = c.post(
        "/v1/load",
        json={
            "name": "qwen3-asr-0.6b",
            "provider": "local_qwen3_asr",
            "device": "cuda",
        },
    )
    assert r.status_code == 202
    s = wait_settled(c)
    assert s["state"] == "error"
    assert s["reason"] == "device_unavailable"


def test_load_failed():
    c = make_client(FakeEngine(fail="load"))
    c.post("/v1/load", json={"name": "qwen3-asr-0.6b", "provider": "local_qwen3_asr"})
    s = wait_settled(c)
    assert s["state"] == "error"
    assert s["reason"] == "load_failed"


def test_transcribe_not_ready():
    c = make_client(FakeEngine())
    r = c.post("/v1/transcribe", json={"audio_data": [0.1, 0.2]})
    assert r.status_code == 409
    assert r.json()["message"] == "not_ready"


def test_transcribe_ok():
    c = make_client(FakeEngine(text="hello world"))
    load_ok(c)
    r = c.post(
        "/v1/transcribe", json={"audio_data": [0.1, -0.2, 0.3], "sample_rate": 16000}
    )
    assert r.status_code == 200
    assert r.json() == {"status": "success", "transcription": "hello world"}


def test_transcribe_invalid_audio():
    c = make_client(FakeEngine())
    load_ok(c)
    r = c.post("/v1/transcribe", json={"audio_data": []})
    assert r.status_code == 400
    assert r.json()["message"] == "invalid_audio"


def test_transcribe_unsupported_language():
    c = make_client(FakeEngine())
    load_ok(c)
    r = c.post("/v1/transcribe", json={"audio_data": [0.1], "language": "xx"})
    assert r.status_code == 400
    assert r.json()["message"] == "unsupported_language"


def test_transcribe_inference_failed():
    c = make_client(FakeEngine(infer_fail=True))
    load_ok(c)
    r = c.post("/v1/transcribe", json={"audio_data": [0.1, 0.2]})
    assert r.status_code == 500
    assert r.json()["message"] == "inference_failed"


def test_transcribe_streaming_emits_done():
    c = make_client(FakeEngine(text="streamed text"))
    load_ok(c)
    with c.stream(
        "POST",
        "/v1/transcribe",
        json={"audio_data": [0.1, 0.2], "options": {"stream_realtime": True}},
    ) as r:
        assert r.status_code == 200
        assert r.headers["content-type"].startswith("text/event-stream")
        body = "".join(r.iter_text())
    assert "event: done" in body
    done = next(b for b in body.split("\n\n") if "event: done" in b)
    data_line = next(line for line in done.splitlines() if line.startswith("data:"))
    payload = json.loads(data_line[len("data:") :].strip())
    assert payload == {"transcription": "streamed text"}


def test_cancel_nothing_in_progress():
    c = make_client(FakeEngine())
    load_ok(c)
    r = c.post("/v1/cancel")
    assert r.status_code == 409
    assert r.json()["message"] == "nothing_in_progress"


def test_cancel_in_progress():
    c = make_client(FakeEngine(infer_delay=0.4))
    load_ok(c)

    results = {}

    def fire():
        results["resp"] = c.post("/v1/transcribe", json={"audio_data": [0.1, 0.2]})

    t = threading.Thread(target=fire)
    t.start()
    time.sleep(0.1)
    r = c.post("/v1/cancel")
    assert r.status_code == 200
    assert r.json()["message"] == "Cancelled"
    t.join()
    assert results["resp"].status_code == 200
