#!/usr/bin/env python3
"""Publish versioned manifests only after both native image jobs pass."""
import json
import os
from pathlib import Path
import re
import subprocess
from release import metadata


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def main():
    repo = os.environ["GITHUB_REPOSITORY"]
    info = metadata(tag=os.environ["RELEASE_TAG"], repository=repo)
    image = info["image"]
    if image != os.environ["IMAGE"]:
        raise ValueError("image repository mismatch")
    sha = os.environ["GITHUB_SHA"]
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("invalid revision")
    digests = {arch: Path(f"digests/{arch}.txt").read_text().strip() for arch in ("amd64", "arm64")}
    if not all(re.fullmatch(r"sha256:[0-9a-f]{64}", value) for value in digests.values()):
        raise ValueError("invalid image digest")
    owner, package = repo.lower().split("/")
    details = None
    for prefix in ("orgs", "users"):
        try:
            details = json.loads(run("gh", "api", f"/{prefix}/{owner}/packages/container/{package}"))
            break
        except Exception:
            continue
    if not details:
        raise ValueError(f"could not fetch GHCR package details for {owner}/{package}")
    if (details.get("visibility") != "public"
            or details.get("repository", {}).get("full_name", "").lower() != repo.lower()):
        raise ValueError("GHCR package must be public and linked to this repository; change package visibility in GitHub settings, then rerun this job")
    refs = [f"{image}@{value}" for value in digests.values()]
    version_ref = f"{image}:{info['version']}"
    run("docker", "buildx", "imagetools", "create", "--tag", version_ref,
        "--tag", f"{image}:sha-{sha}", *refs)
    manifest = json.loads(run("docker", "buildx", "imagetools", "inspect", "--raw", version_ref))
    platforms = {(m["platform"]["os"], m["platform"]["architecture"]): m["digest"] for m in manifest["manifests"]}
    if len(manifest["manifests"]) != 2 or platforms != {("linux", arch): digest for arch, digest in digests.items()}:
        raise ValueError("published manifest does not match tested platforms/digests")
    index_digest = json.loads(run("docker", "buildx", "imagetools", "inspect", "--format", "{{json .Manifest}}", version_ref))["digest"]
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", index_digest):
        raise ValueError("invalid index digest")
    release = {"version": info["version"], "revision": sha, "image": image,
               "digest": index_digest, "platforms": digests}
    Path("image-digests.json").write_text(json.dumps(release, indent=2) + "\n")
    notes = info["notes"] + f"\n\n## Container\n\nLinux amd64 and arm64. Public package; no registry login is required.\n\n```sh\ndocker pull {version_ref}\n# Immutable reference:\ndocker pull {image}@{index_digest}\n```\n\nNo game credentials are included. Protocol redistribution rights are not established; see proto/NOTICE.md.\n"
    Path("release-notes.md").write_text(notes)
    # Stage release notes/assets in a draft; only expose the release when complete.
    exists = subprocess.run(["gh", "release", "view", info["tag"], "--repo", repo],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
    if not exists:
        run("gh", "release", "create", info["tag"], "--repo", repo, "--verify-tag", "--draft",
            "--title", info["tag"], "--notes-file", "release-notes.md")
    run("gh", "release", "upload", info["tag"], "image-digests.json", "--repo", repo, "--clobber")
    run("gh", "release", "edit", info["tag"], "--repo", repo, "--draft=false",
        f"--prerelease={str(info['prerelease']).lower()}", "--title", info["tag"],
        "--notes-file", "release-notes.md", "--latest=false")
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a") as stream:
            stream.write(f"Published `{version_ref}`\n\nDigest: `{index_digest}`\n")


if __name__ == "__main__":
    main()
