#!/usr/bin/env python3
"""Check the repository's relative links: every Markdown link `[x](path)` and every `docs/...` path named in a Markdown file, a
code comment, a script or a workflow must exist. Exit status 1 and a list if anything is broken. `python3 tools/check-links.py`"""
import os, re, subprocess, sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(root)
files = subprocess.run(["git", "ls-files"], capture_output=True, text=True).stdout.split()
bad = []
link = re.compile(r"\]\(([^)\s]+?)(#[^)\s]*)?\)")
docpath = re.compile(r"\bdocs/[A-Za-z0-9_./-]+\.(?:md|png)\b|\bdocs/(?:screenshots|planning|reference|release|audits|reviews)/?(?![A-Za-z0-9_.-])")
skip = ("third_party/", "tests/e2e/node_modules/")
# Paths that belong to the sibling repositories (../rust-os and ../rusty-bucket-aws), named in our docs and comments.
external = (
    "docs/developer/", "docs/operations/", "docs/testing/", "docs/planning/README.md", "docs/planning/architecture.md",
    "docs/planning/rusty-video-player.md", "docs/planning/secrets.md", "docs/planning/web-browsing.md",
)
for f in files:
    if f.startswith(skip) or not f.endswith((".md", ".rs", ".yml", ".toml", ".sh", ".js", ".in", ".xml", ".py", ".iss")):
        continue
    try:
        text = open(f, encoding="utf-8").read()
    except (UnicodeDecodeError, FileNotFoundError):
        continue
    if f.endswith(".md"):
        for m in link.finditer(text):
            t = m.group(1)
            if re.match(r"^(https?:|mailto:|#|/)", t):
                continue
            if not os.path.exists(os.path.normpath(os.path.join(os.path.dirname(f), t))):
                bad.append(f"{f}: link to {t} is broken")
    for m in docpath.finditer(text):
        p = m.group(0).rstrip("/")
        if "*" in p or not p or p.startswith(external):
            continue
        # A path in a generated-file description may name a pattern; only plain paths count.
        if not os.path.exists(p):
            bad.append(f"{f}: names {p}, which does not exist")
print("\n".join(sorted(set(bad))) if bad else "links ok")
sys.exit(1 if bad else 0)
