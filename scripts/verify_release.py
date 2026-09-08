#!/usr/bin/env bash
# This script can be run with python3 or as an executable.
""":"
exec python3 "$0" "$@"
"""
import os
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

def fail(msg: str):
    print(f"❌ FAIL: {msg}", file=sys.stderr)
    sys.exit(1)

def pass_check(msg: str):
    print(f"✅ PASS: {msg}")

def check_file_exists(p: Path, desc: str):
    if not p.is_file():
        fail(f"{desc} not found at {p}")
    pass_check(f"{desc} exists")

def main():
    print("=== pg_lens Release Gate Verification ===\n")
    os.chdir(REPO_ROOT)

    # 1. Parse workspace version from Cargo.toml
    cargo_toml = REPO_ROOT / "Cargo.toml"
    check_file_exists(cargo_toml, "Root Cargo.toml")
    content = cargo_toml.read_text(encoding="utf-8")
    m = re.search(r'\[workspace\.package\][^\[]*?version\s*=\s*"([^"]+)"', content)
    if not m:
        fail("Could not find [workspace.package].version in Cargo.toml")
    version = m.group(1)
    pass_check(f"Target workspace release version: v{version}")

    # 2. Check crate dependency pins in crates/pg_lens_tui/Cargo.toml
    tui_cargo = REPO_ROOT / "crates" / "pg_lens_tui" / "Cargo.toml"
    check_file_exists(tui_cargo, "pg_lens_tui Cargo.toml")
    tui_content = tui_cargo.read_text(encoding="utf-8")
    m_core = re.search(r'pg_lens_core\s*=\s*\{\s*version\s*=\s*"([^"]+)"', tui_content)
    if not m_core or m_core.group(1) != version:
        fail(f"crates/pg_lens_tui/Cargo.toml: pg_lens_core pin is '{m_core.group(1) if m_core else 'missing'}', expected '{version}'")
    m_web = re.search(r'pg_lens_web\s*=\s*\{\s*version\s*=\s*"([^"]+)"', tui_content)
    if not m_web or m_web.group(1) != version:
        fail(f"crates/pg_lens_tui/Cargo.toml: pg_lens_web pin is '{m_web.group(1) if m_web else 'missing'}', expected '{version}'")
    pass_check("crates/pg_lens_tui/Cargo.toml dependency pins match workspace version")

    # 3. Check crate dependency pin in crates/pg_lens_web/Cargo.toml
    web_cargo = REPO_ROOT / "crates" / "pg_lens_web" / "Cargo.toml"
    check_file_exists(web_cargo, "pg_lens_web Cargo.toml")
    web_content = web_cargo.read_text(encoding="utf-8")
    m_core_web = re.search(r'pg_lens_core\s*=\s*\{\s*version\s*=\s*"([^"]+)"', web_content)
    if not m_core_web or m_core_web.group(1) != version:
        fail(f"crates/pg_lens_web/Cargo.toml: pg_lens_core pin is '{m_core_web.group(1) if m_core_web else 'missing'}', expected '{version}'")
    pass_check("crates/pg_lens_web/Cargo.toml dependency pin matches workspace version")

    # 4. Check README.md demo.gif query string cache-buster
    readme = REPO_ROOT / "README.md"
    check_file_exists(readme, "README.md")
    readme_content = readme.read_text(encoding="utf-8")
    expected_readme_gif = f"docs/demo.gif?v={version}"
    if expected_readme_gif not in readme_content:
        fail(f"README.md: Demo GIF link does not contain '?v={version}' cache buster (found stale query parameter or missing link)")
    pass_check(f"README.md has cache-busted demo GIF link ({expected_readme_gif})")

    # 5. Check site/index.html version pill and demo.gif query string
    site_index = REPO_ROOT / "site" / "index.html"
    check_file_exists(site_index, "site/index.html")
    site_content = site_index.read_text(encoding="utf-8")
    expected_site_pill = f"v{version} — Changelog"
    if expected_site_pill not in site_content:
        fail(f"site/index.html: Hero version pill does not contain '{expected_site_pill}'")
    expected_site_gif = f"assets/demo.gif?v={version}"
    if expected_site_gif not in site_content:
        fail(f"site/index.html: Demo GIF asset tag does not contain '{expected_site_gif}'")
    pass_check(f"site/index.html reflects v{version} in version pill and demo asset tag")

    # 6. Check CHANGELOG.md entry
    changelog = REPO_ROOT / "CHANGELOG.md"
    check_file_exists(changelog, "CHANGELOG.md")
    changelog_content = changelog.read_text(encoding="utf-8")
    expected_cl_header = f"## [{version}]"
    if expected_cl_header not in changelog_content:
        fail(f"CHANGELOG.md: Missing release header '{expected_cl_header}'")
    pass_check(f"CHANGELOG.md contains release entry for [{version}]")

    # 7. Check ROADMAP.md shipped entry
    roadmap = REPO_ROOT / "ROADMAP.md"
    check_file_exists(roadmap, "ROADMAP.md")
    roadmap_content = roadmap.read_text(encoding="utf-8")
    expected_rm_entry = f"**v{version}**"
    if expected_rm_entry not in roadmap_content:
        fail(f"ROADMAP.md: Missing shipped entry '{expected_rm_entry}'")
    pass_check(f"ROADMAP.md Shipped section contains {expected_rm_entry}")

    # 8. Check demo GIF asset existence, size, and header
    demo_gif = REPO_ROOT / "docs" / "demo.gif"
    check_file_exists(demo_gif, "docs/demo.gif")
    gif_size = demo_gif.stat().st_size
    if gif_size < 500000:
        fail(f"docs/demo.gif is suspiciously small ({gif_size} bytes, expected > 500 KB)")
    with open(demo_gif, "rb") as f:
        header = f.read(6)
    if header not in (b"GIF89a", b"GIF87a"):
        fail(f"docs/demo.gif does not have a valid GIF header (found {header!r})")
    pass_check(f"docs/demo.gif is a valid GIF ({gif_size / (1024 * 1024):.2f} MB)")

    # 9. Verify licenses generator
    print("\nVerifying license compliance...")
    orig_licenses = (REPO_ROOT / "THIRD_PARTY_LICENSES.md").read_text(encoding="utf-8") if (REPO_ROOT / "THIRD_PARTY_LICENSES.md").exists() else ""
    res = subprocess.run([sys.executable, "scripts/generate_licenses.py"], capture_output=True, text=True)
    if res.returncode != 0:
        fail(f"scripts/generate_licenses.py failed: {res.stderr}")
    new_licenses = (REPO_ROOT / "THIRD_PARTY_LICENSES.md").read_text(encoding="utf-8")
    if orig_licenses != new_licenses:
        fail("THIRD_PARTY_LICENSES.md has unstaged/uncommitted diffs vs scripts/generate_licenses.py")
    pass_check("THIRD_PARTY_LICENSES.md is clean and matches dependencies")

    # 10. Verify site build
    print("\nVerifying site documentation build...")
    site_res = subprocess.run(["node", "site/build.mjs"], capture_output=True, text=True)
    if site_res.returncode != 0:
        fail(f"node site/build.mjs failed: {site_res.stderr}")
    site_gif = REPO_ROOT / "_site" / "assets" / "demo.gif"
    if not site_gif.is_file():
        fail("_site/assets/demo.gif was not produced by site/build.mjs")
    pass_check("site/build.mjs assembled _site/ cleanly with assets")

    print("\n=======================================================")
    print(f"🎉 ALL RELEASE VERIFICATION CHECKS PASSED FOR v{version}!")
    print("=======================================================")

if __name__ == "__main__":
    main()
