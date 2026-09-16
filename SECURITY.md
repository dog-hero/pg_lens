# Security Policy

The `pg_lens` team takes the security of our software, dependencies, and our users' database environments seriously. This document outlines our security procedures, supported versions, and how to report vulnerabilities responsibly.

---

## Supported Versions

Only the latest release series receives active security updates and vulnerability patches. We recommend all users upgrade to the latest stable release.

| Version | Supported          |
| :------ | :----------------- |
| 0.23.x  | :white_check_mark: |
| < 0.23  | :x:                |

---

## Reporting a Vulnerability

If you discover a security vulnerability in `pg_lens`, **please do not report it in a public GitHub issue**. Public disclosure exposes active users to potential exploitation before a fix is available.

Instead, please report vulnerabilities using one of the following private channels:

### 1. GitHub Private Vulnerability Reporting (Recommended)
You can privately open a draft security advisory directly through GitHub:
1. Navigate to the repository's [Security Tab](https://github.com/dog-hero/pg_lens/security).
2. Click on **Report a vulnerability** under the Advisories section.
3. Provide details according to the guidelines below.

### 2. Direct Security Contact
If you cannot use GitHub Security Advisories, send an email to:
* **Email**: [benedetlabs@gmail.com](mailto:benedetlabs@gmail.com)
* **Subject**: `[SECURITY] Vulnerability in pg_lens`

---

## What to Include in a Report

To help us triage and resolve the issue quickly, please include:
* **Description**: A clear description of the vulnerability and its potential impact.
* **Component**: Which layer is affected (`pg_lens_core`, `pg_lens_tui`, `pg_lens_web`, or CLI/scripts).
* **Steps to Reproduce**: A minimal, reproducible example or proof-of-concept (PoC).
* **Environment**: Operating system, terminal emulator, PostgreSQL version, and `pg_lens --version`.
* **Remediation**: Any proposed fix or mitigation, if you have one.

> [!CAUTION]
> **Credential Safety**: When providing logs, configurations, or PoCs, **always sanitize and redact** database passwords, connection strings, hostnames, and proprietary query text.

---

## Response and Disclosure Process

1. **Initial Response**: We will acknowledge receipt of your vulnerability report within **48 hours**.
2. **Assessment & Triage**: We will confirm the vulnerability, determine its severity, and keep you informed of progress.
3. **Patch & Testing**: A fix will be developed in a private branch or fork and tested against our quality gates and matrix test suites.
4. **Coordinated Release**: A patched release will be published alongside a GitHub Security Advisory crediting your discovery (unless you prefer to remain anonymous).

---

## Security Best Practices for Operators

When monitoring PostgreSQL clusters with `pg_lens`:
1. **Least-Privilege Role**: Connect using a dedicated monitoring user (e.g. member of `pg_monitor` or `pg_read_all_stats`) rather than a superuser account. See [The pg_lens monitoring role documentation](https://dog-hero.github.io/pg_lens/docs/connection-user.html) for exact grants.
2. **Read-Only Mode**: In production environments where administrative actions (`c` cancel backend, `K` terminate backend) should never be executed, pass `--read-only` or set `PG_LENS_READ_ONLY=true`.
3. **Safe Credential Handling**: Use `services.toml` with `password_cmd` or standard PostgreSQL environment variables (`PGPASSWORD`, `PGPASSFILE`) rather than embedding credentials in plaintext CLI arguments or command history.
4. **Local State Directory**: Diagnostic logs are stored in `~/.local/state/pg_lens/error.log` (or `$PG_LENS_STATE_DIR/error.log`). Restrict file system permissions on this directory if running in shared multi-user environments.

