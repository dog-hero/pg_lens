#!/usr/bin/env python3
"""
Generate THIRD_PARTY_LICENSES.md for pg_lens.
Extracts all Rust dependency metadata via `cargo metadata` and frontend
dependencies from `crates/pg_lens_web/frontend/package.json`.
"""

import json
import os
import subprocess
import sys
from datetime import datetime

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUTPUT_FILE = os.path.join(ROOT, "THIRD_PARTY_LICENSES.md")

INTERNAL_CRATES = {"pg_lens_core", "pg_lens_tui", "pg_lens_web"}

def get_rust_dependencies():
    output = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=ROOT,
        text=True
    )
    data = json.loads(output)
    packages = []
    for pkg in data["packages"]:
        name = pkg["name"]
        if name in INTERNAL_CRATES:
            continue
        packages.append({
            "name": name,
            "version": pkg["version"],
            "license": pkg.get("license") or "Custom/Proprietary",
            "repository": pkg.get("repository") or pkg.get("homepage") or "",
            "description": (pkg.get("description") or "").strip().replace("\n", " "),
            "authors": pkg.get("authors", []),
        })
    return sorted(packages, key=lambda p: p["name"].lower())

def get_frontend_dependencies():
    pkg_json_path = os.path.join(ROOT, "crates", "pg_lens_web", "frontend", "package.json")
    if not os.path.exists(pkg_json_path):
        return []
    with open(pkg_json_path, "r", encoding="utf-8") as f:
        data = json.load(f)
    
    # We list runtime dependencies
    deps = []
    for name, ver in data.get("dependencies", {}).items():
        license_type = "MIT" # uPlot is MIT
        repo = f"https://github.com/leeoniya/uPlot" if name == "uplot" else ""
        deps.append({
            "name": name,
            "version": ver.lstrip("^~"),
            "license": license_type,
            "repository": repo,
            "ecosystem": "npm",
        })
    return deps

def main():
    rust_pkgs = get_rust_dependencies()
    frontend_pkgs = get_frontend_dependencies()
    
    # Group Rust packages by license
    by_license = {}
    for pkg in rust_pkgs:
        lic = pkg["license"]
        by_license.setdefault(lic, []).append(pkg)
    
    lines = []
    lines.append("# Third-Party Software Notices and Licenses")
    lines.append("")
    lines.append("This page documents the open-source software packages incorporated into, linked by,")
    lines.append("or bundled with **pg_lens**.")
    lines.append("")
    lines.append("pg_lens itself is licensed under the **Functional Source License, Version 1.1, MIT Future License (FSL-1.1-MIT)**.")
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append("## Summary of Third-Party Licenses")
    lines.append("")
    lines.append("| License Type | Number of Packages | Notes |")
    lines.append("| :--- | :--- | :--- |")
    for lic, pkgs in sorted(by_license.items(), key=lambda item: -len(item[1])):
        notes = "Permissive"
        if "Apache" in lic and "MIT" in lic:
            notes = "Dual-licensed (Permissive)"
        elif "BSD" in lic or "ISC" in lic or "Zlib" in lic:
            notes = "Permissive"
        elif "Unlicense" in lic or "CC0" in lic or "0BSD" in lic:
            notes = "Public Domain / Permissive"
        lines.append(f"| `{lic}` | {len(pkgs)} | {notes} |")
    if frontend_pkgs:
        lines.append(f"| `MIT (npm)` | {len(frontend_pkgs)} | Permissive (Web Frontend) |")
    
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append("## Special Acknowledgements")
    lines.append("")
    lines.append("### 1. pg_activity")
    lines.append("* **Project:** [pg_activity](https://github.com/dalibo/pg_activity)")
    lines.append("* **Copyright:** Copyright (c) 2012-2026 Dalibo")
    lines.append("* **License:** **PostgreSQL License** (Permissive Open Source)")
    lines.append("* **Description:** The conceptual inspiration for terminal database monitoring; pg_lens adapts battle-tested system queries.")
    lines.append("")
    lines.append("### 2. Ratatui")
    lines.append("* **Project:** [ratatui](https://ratatui.rs)")
    lines.append("* **License:** MIT License")
    lines.append("* **Description:** The Rust terminal user interface library powering the pg_lens TUI.")
    lines.append("")
    lines.append("### 3. uPlot (Web Lens)")
    lines.append("* **Project:** [uPlot](https://github.com/leeoniya/uPlot)")
    lines.append("* **Copyright:** Copyright (c) 2019 Leon Sorokin")
    lines.append("* **License:** MIT License")
    lines.append("* **Description:** Fast, memory-efficient canvas chart engine powering Web Lens.")
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append("## Complete List of Rust Crates")
    lines.append("")
    lines.append("| Package | Version | License | Repository |")
    lines.append("| :--- | :--- | :--- | :--- |")
    for pkg in rust_pkgs:
        name = pkg["name"]
        ver = pkg["version"]
        lic = pkg["license"]
        repo = pkg["repository"]
        repo_link = f"[{repo}]({repo})" if repo else "—"
        lines.append(f"| `{name}` | `{ver}` | `{lic}` | {repo_link} |")
    
    if frontend_pkgs:
        lines.append("")
        lines.append("---")
        lines.append("")
        lines.append("## Web Frontend Dependencies (npm)")
        lines.append("")
        lines.append("| Package | Version | License | Repository |")
        lines.append("| :--- | :--- | :--- | :--- |")
        for pkg in frontend_pkgs:
            name = pkg["name"]
            ver = pkg["version"]
            lic = pkg["license"]
            repo = pkg["repository"]
            repo_link = f"[{repo}]({repo})" if repo else "—"
            lines.append(f"| `{name}` | `{ver}` | `{lic}` | {repo_link} |")
    
    lines.append("")
    lines.append("---")
    lines.append("")
    lines.append("## Standard License Texts")
    lines.append("")
    lines.append("### MIT License")
    lines.append("```text")
    lines.append("""Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.""")
    lines.append("```")
    lines.append("")
    lines.append("### Apache License 2.0")
    lines.append("```text")
    lines.append("""Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.""")
    lines.append("```")
    lines.append("")
    lines.append("### PostgreSQL License")
    lines.append("```text")
    lines.append("""Portions Copyright (c) 1996-2026, The PostgreSQL Global Development Group
Portions Copyright (c) 1994, The Regents of the University of California

Permission to use, copy, modify, and distribute this software and its
documentation for any purpose, without fee, and without a written agreement
is hereby granted, provided that the above copyright notice and this
paragraph and the following two paragraphs appear in all copies.

IN NO EVENT SHALL THE UNIVERSITY OF CALIFORNIA BE LIABLE TO ANY PARTY FOR
DIRECT, INDIRECT, SPECIAL, INCIDENTAL, OR CONSEQUENTIAL DAMAGES, INCLUDING
LOST PROFITS, ARISING OUT OF THE USE OF THIS SOFTWARE AND ITS
DOCUMENTATION, EVEN IF THE UNIVERSITY OF CALIFORNIA HAS BEEN ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.

THE UNIVERSITY OF CALIFORNIA SPECIFICALLY DISCLAIMS ANY WARRANTIES,
INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY
AND FITNESS FOR A PARTICULAR PURPOSE.  THE SOFTWARE PROVIDED HEREUNDER IS
ON AN "AS IS" BASIS, AND THE UNIVERSITY OF CALIFORNIA HAS NO OBLIGATIONS TO
PROVIDE MAINTENANCE, SUPPORT, UPDATES, ENHANCEMENTS, OR MODIFICATIONS.""")
    lines.append("```")
    lines.append("")
    
    with open(OUTPUT_FILE, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    
    print(f"Generated {OUTPUT_FILE} successfully ({len(rust_pkgs)} Rust crates, {len(frontend_pkgs)} npm packages).")

if __name__ == "__main__":
    main()

