"""Pytest fixtures for the MCP regression suite."""

from __future__ import annotations

import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.error
import urllib.request
import uuid

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
DOCKER_DIR = REPO_ROOT / "docker"
SQLITE_PATH = "/data/.rlm/memory/memory_bridge_v2.db"
DEFAULT_MODEL = "paraphrase-multilingual-MiniLM-L12-v2"


class RLMClient:
    """Tiny MCP client for the streamable-http transport."""

    def __init__(self, base_url: str, timeout: int = 30):
        self.base_url = base_url
        self.timeout = timeout
        self.session_id: str | None = None
        self._call_id = 1
        self._created_ids: list[str] = []

    def _post(self, payload: dict) -> str:
        body = json.dumps(payload).encode("utf-8")
        headers: dict[str, str] = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self.session_id:
            headers["Mcp-Session-Id"] = self.session_id
        req = urllib.request.Request(
            self.base_url,
            data=body,
            headers=headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                sid = resp.headers.get("Mcp-Session-Id")
                if sid:
                    self.session_id = sid
                return resp.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            return exc.read().decode("utf-8")

    @staticmethod
    def _parse_response(raw: str) -> dict | None:
        for line in raw.splitlines():
            if line.startswith("data:"):
                data = line[5:].strip()
                if data and data != "[DONE]":
                    return json.loads(data)
        if raw.strip():
            try:
                return json.loads(raw)
            except json.JSONDecodeError:
                return None
        return None

    def initialize(self) -> None:
        payload = {
            "jsonrpc": "2.0",
            "id": self._call_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "pytest-rlm", "version": "1.0"},
            },
        }
        self._call_id += 1
        self._post(payload)
        self._post({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def call(self, tool_name: str, **kwargs) -> dict:
        payload = {
            "jsonrpc": "2.0",
            "id": self._call_id,
            "method": "tools/call",
            "params": {"name": tool_name, "arguments": kwargs},
        }
        self._call_id += 1
        raw = self._post(payload)
        msg = self._parse_response(raw)
        if msg and "result" in msg:
            for item in msg["result"].get("content", []):
                if item.get("type") == "text":
                    try:
                        return json.loads(item["text"])
                    except json.JSONDecodeError:
                        return {"text": item["text"]}
            structured = msg["result"].get("structuredContent")
            if isinstance(structured, dict):
                return structured.get("result", structured)
        return msg or {}


def _run(command: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        capture_output=True,
        text=True,
        check=check,
    )


def _find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        sock.listen(1)
        return int(sock.getsockname()[1])


def _wait_for_server(base_url: str, timeout: int = 180) -> None:
    client = RLMClient(base_url=base_url, timeout=10)
    deadline = time.time() + timeout
    last_error = ""
    while time.time() < deadline:
        try:
            client.initialize()
            return
        except Exception as exc:  # pragma: no cover - best effort retry loop
            last_error = str(exc)
        time.sleep(2)
    raise RuntimeError(f"Timed out waiting for MCP server: {last_error}")


@pytest.fixture(scope="session")
def rlm_env():
    """Build and run an isolated Docker-backed RLM instance for the suite."""

    external_url = os.environ.get("RLM_TEST_URL")
    if external_url:
        yield {
            "base_url": external_url,
            "container_name": os.environ.get("RLM_DOCKER_CONTAINER", ""),
            "image_name": os.environ.get("RLM_TEST_IMAGE", ""),
            "sqlite_path": os.environ.get("RLM_SQLITE_PATH", SQLITE_PATH),
            "expected_model": os.environ.get("RLM_EXPECTED_MODEL", DEFAULT_MODEL),
        }
        return

    probe = _run(["docker", "version"], check=False)
    if probe.returncode != 0:
        pytest.skip(f"Docker is unavailable: {probe.stderr.strip()}")

    suffix = uuid.uuid4().hex[:8]
    image_name = os.environ.get("RLM_TEST_IMAGE", f"rlm-workflow-pytest:{suffix}")
    container_name = f"rlm-pytest-{suffix}"
    port = int(os.environ.get("RLM_TEST_PORT", _find_free_port()))

    if "RLM_TEST_IMAGE" not in os.environ:
        build = _run(["docker", "build", "-t", image_name, str(DOCKER_DIR)], check=False)
        if build.returncode != 0:
            pytest.skip(f"Failed to build Docker image:\n{build.stderr.strip()}")

    run_command = [
        "docker",
        "run",
        "-d",
        "--rm",
        "--name",
        container_name,
        "-p",
        f"{port}:8200",
        "-e",
        "RLM_DATA_DIR=/data",
        "-e",
        "RLM_HOST=0.0.0.0",
        "-e",
        "RLM_PORT=8200",
        "-e",
        "RLM_TRANSPORT=streamable-http",
        "-e",
        f"RLM_EMBEDDING_MODEL={DEFAULT_MODEL}",
        "-e",
        "RLM_EMBEDDING_PROVIDER=fastembed",
        image_name,
    ]
    started = _run(run_command, check=False)
    if started.returncode != 0:
        pytest.skip(f"Failed to start test container:\n{started.stderr.strip()}")

    base_url = f"http://127.0.0.1:{port}/mcp"
    start_timeout = int(os.environ.get("RLM_TEST_START_TIMEOUT", "600"))
    try:
        _wait_for_server(base_url, timeout=start_timeout)
    except Exception as exc:  # pragma: no cover - setup failure path
        logs = _run(["docker", "logs", container_name], check=False)
        _run(["docker", "rm", "-f", container_name], check=False)
        pytest.fail(f"Failed to start MCP server: {exc}\n{logs.stdout}\n{logs.stderr}")

    yield {
        "base_url": base_url,
        "container_name": container_name,
        "image_name": image_name,
        "sqlite_path": SQLITE_PATH,
        "expected_model": DEFAULT_MODEL,
    }

    _run(["docker", "rm", "-f", container_name], check=False)
    if "RLM_TEST_IMAGE" not in os.environ:
        _run(["docker", "rmi", image_name], check=False)


def docker_logs(container: str) -> str:
    result = _run(["docker", "logs", container], check=False)
    if result.returncode != 0:
        pytest.fail(f"docker logs failed: {result.stderr.strip()}")
    return result.stdout + result.stderr


def query_rlm_db(container: str, sql: str, sqlite_path: str = SQLITE_PATH) -> str:
    script = (
        "import json, sqlite3; "
        f"conn = sqlite3.connect({sqlite_path!r}); "
        f"print(json.dumps(conn.execute({sql!r}).fetchall(), ensure_ascii=False))"
    )
    result = _run(
        ["docker", "exec", container, "python3", "-c", script],
        check=False,
    )
    if result.returncode != 0:
        pytest.fail(f"query_rlm_db failed: {result.stderr.strip()}")
    return result.stdout.strip()


@pytest.fixture(scope="session")
def client(rlm_env):
    c = RLMClient(base_url=rlm_env["base_url"])
    c.initialize()
    return c


@pytest.fixture
def rlm(client):
    client._created_ids = []
    client.call("rlm_start_session", restore=False)
    yield client
    for fact_id in list(client._created_ids):
        try:
            client.call("rlm_delete_fact", fact_id=fact_id)
        except Exception:
            pass


@pytest.fixture
def causal_seed(rlm):
    marker = f"PYTEST_CAUSAL_{uuid.uuid4().hex}"
    result = rlm.call(
        "rlm_record_causal_decision",
        decision=f"Test decision {marker}",
        reasons=[f"Test reasoning {marker}"],
        alternatives=["none"],
        consequences=[f"Test only {marker}"],
    )
    assert result.get("decision_id") or result.get("status") == "success"
    return marker


@pytest.fixture
def fact_seed(rlm):
    marker = f"PYTEST_SEED_{uuid.uuid4().hex[:8]}"
    for content in [
        f"PENDING tasks next session: {marker}",
        f"Unfinished tasks for the next session: {marker}",
    ]:
        result = rlm.call(
            "rlm_add_hierarchical_fact",
            content=content,
            domain="workflow",
            level=1,
        )
        if result.get("fact_id"):
            rlm._created_ids.append(result["fact_id"])
    return marker
