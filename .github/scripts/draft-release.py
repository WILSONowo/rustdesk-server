#!/usr/bin/env python3
"""Validate a complete artifact set and upload it to an unpublished release."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--expected", type=int, required=True)
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    repo = os.environ["GITHUB_REPOSITORY"]
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    files = sorted(p for p in args.assets.iterdir() if p.is_file()
                   and p.name not in {"SHA256SUMS", "build-info.json"})
    if len(files) != args.expected or any(p.stat().st_size == 0 for p in files):
        raise SystemExit(f"Expected {args.expected} nonempty release files, got {len(files)}")
    metadata = {"repository": repo, "tag": args.tag, "commit": commit}
    sources = Path(".github/release-sources.json")
    if sources.exists():
        metadata["dependencies"] = json.loads(sources.read_text())
    metadata["submodules"] = subprocess.check_output(
        ["git", "submodule", "status", "--recursive"], text=True).splitlines()
    info = args.assets / "build-info.json"
    info.write_text(json.dumps(metadata, indent=2) + "\n")
    sums = args.assets / "SHA256SUMS"
    sums.write_text("".join(f"{hashlib.file_digest(p.open('rb'), 'sha256').hexdigest()}  {p.name}\n"
                            for p in sorted(files + [info])))
    if args.prepare_only:
        print(f"Validated {args.expected} files and prepared checksums")
        return
    tag_commit = subprocess.check_output(
        ["git", "rev-parse", f"{args.tag}^{{commit}}"], text=True).strip()
    if tag_commit != commit:
        raise SystemExit("Release tag must point at the checked-out commit")
    # Listing includes authenticated drafts. Network/auth errors fail the job.
    releases = json.loads(gh("release", "list", "--repo", repo, "--limit", "1000",
                             "--json", "tagName,isDraft"))
    release = next((r for r in releases if r["tagName"] == args.tag), None)
    if release and not release["isDraft"]:
        raise SystemExit("Refusing to replace an already published release")
    if release:
        existing = json.loads(gh("release", "view", args.tag, "--repo", repo,
                                 "--json", "targetCommitish"))
        if existing["targetCommitish"] != commit:
            raise SystemExit("Existing draft targets a different commit")
    else:
        gh("release", "create", args.tag, "--repo", repo, "--verify-tag",
           "--target", commit, "--draft", "--prerelease", "--title", args.tag,
           "--notes", "")
    gh("release", "upload", args.tag, "--repo", repo, "--clobber",
       *(str(p) for p in files))
    print(f"Draft {repo}@{args.tag}: {args.expected} release files (checksums kept in build output)")


if __name__ == "__main__":
    main()
