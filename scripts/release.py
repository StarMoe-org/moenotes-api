#!/usr/bin/env python3
"""Validate the version/tag/lockfile contract without network access."""
import argparse
import json
import os
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-(alpha|beta|rc)\.(0|[1-9][0-9]*))?")


def metadata(root=ROOT, tag="", repository="StarMoe-org/moenotes-api"):
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    version = manifest["workspace"]["package"]["version"]
    if not VERSION.fullmatch(version) or tag and tag != f"v{version}":
        raise ValueError("expected a vMAJOR.MINOR.PATCH[-alpha.N|-beta.N|-rc.N] tag matching Cargo.toml")
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    local = {p["name"]: p["version"] for p in lock["package"] if "source" not in p}
    if local != {f"moenotes-{name}": version for name in ("proto", "client", "server")}:
        raise ValueError("workspace package versions in Cargo.lock do not match")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("invalid repository name")
    changelog = (root / "CHANGELOG.md").read_text()
    match = re.search(rf"^## {re.escape(version)} - \d{{4}}-\d{{2}}-\d{{2}}\n(.*?)(?=^## |\Z)", changelog, re.M | re.S)
    if not match or not match[1].strip():
        raise ValueError("dated nonempty changelog entry required")
    return {"version": version, "tag": f"v{version}", "prerelease": "-" in version,
            "image": "ghcr.io/" + repository.lower(), "notes": match[1].strip()}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", default="")
    args = parser.parse_args()
    result = metadata(tag=args.tag, repository=os.environ.get("GITHUB_REPOSITORY", "StarMoe-org/moenotes-api"))
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a") as stream:
            for key in ("version", "image"):
                stream.write(f"{key}={result[key]}\n")
    print(json.dumps({k: v for k, v in result.items() if k != "notes"}))
