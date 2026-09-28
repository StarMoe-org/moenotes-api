#!/usr/bin/env python3
"""Test a built container with synthetic config; no game endpoint is called."""
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request
import uuid


def run(*args, **kwargs):
    return subprocess.check_output(args, **kwargs)


def wait_healthy(opener, port):
    for attempt in range(50):
        try:
            with opener.open(f"http://{port}/healthz", timeout=1) as response:
                assert json.load(response) == {"status": "ok"}
            return
        except (urllib.error.URLError, TimeoutError):
            if attempt == 49:
                raise
            time.sleep(0.2)


def unconfigured(image, template=False):
    name = "moenotes-empty-smoke-" + uuid.uuid4().hex
    run("docker", "create", "--name", name, "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
        "-p", "127.0.0.1::8080", image)
    try:
        if template:
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode="w") as tar:
                directory = tarfile.TarInfo("etc/moenotes")
                directory.type, directory.mode = tarfile.DIRTYPE, 0o755
                tar.addfile(directory)
                data = (Path(__file__).resolve().parents[1] / "config.example.toml").read_bytes()
                entry = tarfile.TarInfo("etc/moenotes/config.toml")
                entry.size, entry.mode = len(data), 0o644
                tar.addfile(entry, io.BytesIO(data))
            subprocess.run(["docker", "cp", "-a", "-", f"{name}:/"], input=archive.getvalue(), check=True)
        run("docker", "start", name)
        port = run("docker", "port", name, "8080/tcp", text=True).strip()
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        wait_healthy(opener, port)
        with opener.open(f"http://{port}/health", timeout=2) as response:
            assert json.load(response) == {"status": "ok"}
        for path in ("/readyz", "/v1/profile?playerProfileId=1", "/v1/status", "/openapi.json"):
            try:
                opener.open(f"http://{port}{path}", timeout=2)
                raise AssertionError("unconfigured service exposed a business route")
            except urllib.error.HTTPError as error:
                assert error.code == 503
                assert error.headers["Cache-Control"] == "no-store"
        logs = run("docker", "logs", name, stderr=subprocess.STDOUT, text=True)
        warnings = [json.loads(line) for line in logs.splitlines() if line.startswith("{")]
        warning = next(row for row in warnings if row.get("event") == "configuration_missing")
        assert {"api_key", "session.origin", "login.context.device_identifier",
                "login.sdk_http.app_key"} <= set(warning["missing"])
        actual = json.loads(run("docker", "exec", name, "moenotes-server", "config-path", text=True))
        expected = "/etc/moenotes/config.toml" if template else "/var/lib/moenotes/config.toml"
        assert actual["config_path"] == expected and actual["exists"]
        if not template:
            assert run("docker", "exec", name, "stat", "-c", "%a", expected, text=True).strip() == "600"
        assert warning["mode"] == "health_only"
        check = subprocess.run(["docker", "exec", name, "moenotes-server", "check-config",
                                expected], capture_output=True)
        assert check.returncode != 0
        run("docker", "stop", "--time", "5", name)
        assert run("docker", "inspect", "--format", "{{.State.ExitCode}}", name, text=True).strip() == "0"
    finally:
        subprocess.run(["docker", "rm", "--force", name], check=False, stdout=subprocess.DEVNULL)


def main(image, version):
    assert run("docker", "run", "--rm", "--network", "none", image, "--version", text=True).strip() == f"moenotes-server {version}"
    assert run("docker", "image", "inspect", "--format", "{{.Config.User}}", image, text=True).strip() == "65532:65532"
    unconfigured(image)
    unconfigured(image, template=True)
    configured(image, accounts=False)
    configured(image, accounts=True)
    configured(image, accounts=True, inline=True)


def configured(image, accounts, inline=False):
    config = b'''listen = "0.0.0.0:8080"
response_mode = "disabled"
api_key_file = "key"
[version_sync]
enabled = false
[session]
region = "synthetic"
origin = "https://game.example.invalid"
allowed_origins = ["https://game.example.invalid"]
platform = "android"
client_version = "1.0.1"
'''
    files = {"config.toml": config, "key": b"synthetic-smoke-key-not-for-production-123456"}
    if accounts:
        files["config.toml"] = config.replace(b'response_mode = "disabled"', b'response_mode = "public"') + b'''
[accounts]
directory = "/accounts"
[login]
context_file = "context.json"
sdk_http_file = "sdk.json"
state_dir = "state"
'''
        files["context.json"] = json.dumps({
            "device_model": "synthetic", "operating_system": "synthetic",
            "device_identifier": "synthetic", "global_channel_id": 1,
            "brand_id": 1, "area_id": 1,
        }).encode()
        common = dict.fromkeys((
            "game_id", "server_id", "merchant_id", "app_ver", "sdk_ver", "channel_id",
            "platform", "platform_type", "net", "operators", "model", "pf_ver", "udid",
            "dp", "adid", "lang", "sdk_log_type", "ad_ext", "time_zone", "isRoot",
        ), "synthetic")
        files["sdk.json"] = json.dumps({
            "base_url": "https://sdk.example.invalid",
            "allowed_base_urls": ["https://sdk.example.invalid"],
            "app_key": "synthetic-key", "country_id": 1, "common": common,
        }).encode()
    if inline:
        data = (Path(__file__).resolve().parents[1] / "server/tests/fixtures/inline-config.toml").read_text()
        data = data.replace('127.0.0.1:0', '0.0.0.0:8080')
        data = data.replace('synthetic-bootstrap-api-key-1234567890', 'synthetic-smoke-key-not-for-production-123456')
        data = data.replace('directory = "accounts"', 'directory = "/accounts"').replace('state_dir = "."', 'state_dir = "state"')
        files = {"config.toml": data.encode()}
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as tar:
        for path in (["etc/moenotes", "etc/moenotes/state", "accounts"] if accounts else ["etc/moenotes"]):
            directory = tarfile.TarInfo(path)
            directory.type, directory.mode = tarfile.DIRTYPE, 0o700
            directory.uid = directory.gid = 65532
            tar.addfile(directory)
        for name, data in files.items():
            entry = tarfile.TarInfo("etc/moenotes/" + name)
            entry.uid = entry.gid = 65532
            entry.size, entry.mode = len(data), 0o600
            tar.addfile(entry, io.BytesIO(data))
    name = "moenotes-smoke-" + uuid.uuid4().hex
    run("docker", "create", "--name", name, "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
        "-p", "127.0.0.1::8080", image)
    try:
        subprocess.run(["docker", "cp", "-a", "-", f"{name}:/"], input=archive.getvalue(), check=True)
        run("docker", "start", name)
        port = run("docker", "port", name, "8080/tcp", text=True).strip()
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        wait_healthy(opener, port)
        for path, expected in (("/openapi.json", 401), ("/v1/announcements", 401 if accounts else 404)):
            try:
                opener.open(f"http://{port}{path}", timeout=2)
                raise AssertionError("unexpected success")
            except urllib.error.HTTPError as error:
                assert error.code == expected
        run("docker", "exec", name, "moenotes-server", "check-config", "/etc/moenotes/config.toml")
        if accounts:
            def authenticated(path):
                return urllib.request.Request(f"http://{port}{path}", headers={
                    "Authorization": "Bearer synthetic-smoke-key-not-for-production-123456",
                })
            with opener.open(authenticated("/v1/status"), timeout=2) as response:
                assert json.load(response)["session"]["recovery_attempts"] == 0
            try:
                opener.open(authenticated("/v1/profile?playerProfileId=1"), timeout=2)
                raise AssertionError("empty accounts returned business data")
            except urllib.error.HTTPError as error:
                assert error.code == 503
            wait_healthy(opener, port)
            logs = run("docker", "logs", name, stderr=subprocess.STDOUT, text=True)
            assert "account_source" in logs
            assert "account_sdk_login" not in logs
            assert "synthetic-never-log" not in logs
        for path in ("LICENSE", "PROTOCOL-NOTICE.md"):
            run("docker", "exec", name, "test", "-s", f"/usr/share/doc/moenotes-api/{path}")
        run("docker", "stop", "--time", "5", name)
        assert run("docker", "inspect", "--format", "{{.State.ExitCode}}", name, text=True).strip() == "0"
    finally:
        subprocess.run(["docker", "rm", "--force", name], check=False, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main(*sys.argv[1:])
