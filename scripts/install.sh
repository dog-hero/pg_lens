#!/bin/sh
#
# pg_lens installer — https://github.com/dog-hero/pg_lens
#
#   curl -fsSL https://raw.githubusercontent.com/dog-hero/pg_lens/main/scripts/install.sh | sh
#
# Downloads the prebuilt release tarball for this platform, verifies it
# against the `.sha256` sidecar published by the release workflow, and
# installs the `pg_lens` binary into ~/.local/bin. No sudo, no rc-file
# edits, nothing left behind on failure.
#
# Deliberately POSIX sh (runs under dash/busybox ash) and deliberately
# unminified: a script people pipe into a shell should be readable in one
# sitting. Re-running it is the upgrade path.

set -eu

REPO="dog-hero/pg_lens"
BIN="pg_lens"
API_LATEST="https://api.github.com/repos/${REPO}/releases/latest"
DOCS_URL="https://github.com/${REPO}#installation"
DEMO_URL="https://dog-hero.github.io/pg_lens/demo/"

# Overridable by env; flags win over env (parsed below).
VERSION="${PG_LENS_VERSION:-}"
INSTALL_DIR="${PG_LENS_INSTALL_DIR:-$HOME/.local/bin}"
DRY_RUN=0

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }
die() {
  err "$*"
  exit 1
}

usage() {
  cat <<EOF
pg_lens installer

Usage:
  install.sh [--version <vX.Y.Z>] [--dir <path>] [--dry-run] [--help]

Options:
  --version <v>   Install a specific release tag (default: latest).
                  "0.15.0" and "v0.15.0" are both accepted.
  --dir <path>    Install directory (default: \$HOME/.local/bin).
  --dry-run       Print what would be downloaded and where, then exit.
  --help          Show this help.

Environment:
  PG_LENS_VERSION       Same as --version.
  PG_LENS_INSTALL_DIR   Same as --dir.

Platforms: macOS (arm64, x86_64) and Linux musl (x86_64, aarch64).
Anything else: see ${DOCS_URL}
EOF
}

# ---------------------------------------------------------------- arguments

while [ $# -gt 0 ]; do
  case "$1" in
    --help | -h)
      usage
      exit 0
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --version)
      [ $# -ge 2 ] || die "--version needs an argument (e.g. --version v0.15.0)"
      VERSION="$2"
      shift 2
      ;;
    --version=*)
      VERSION="${1#--version=}"
      shift
      ;;
    --dir)
      [ $# -ge 2 ] || die "--dir needs an argument (e.g. --dir /opt/bin)"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --dir=*)
      INSTALL_DIR="${1#--dir=}"
      shift
      ;;
    *)
      err "unknown option: $1"
      usage >&2
      exit 2
      ;;
  esac
done

# ------------------------------------------------------------ http fetching

# curl is the norm, but minimal Linux images often ship only wget. Resolve
# one downloader up front so every later fetch is a single call.
if command -v curl >/dev/null 2>&1; then
  HTTP=curl
elif command -v wget >/dev/null 2>&1; then
  HTTP=wget
else
  die "neither curl nor wget found — install one of them and re-run"
fi

# fetch_stdout <url>  — body on stdout, non-zero exit on HTTP error.
fetch_stdout() {
  if [ "$HTTP" = curl ]; then
    curl -fsSL "$1"
  else
    wget -qO- "$1"
  fi
}

# fetch_file <url> <dest>
fetch_file() {
  if [ "$HTTP" = curl ]; then
    curl -fsSL -o "$2" "$1"
  else
    wget -qO "$2" "$1"
  fi
}

# ------------------------------------------------------- platform detection

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin)
    case "$arch" in
      arm64 | aarch64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) target="" ;;
    esac
    ;;
  Linux)
    # musl targets only: one static binary that runs on glibc and musl
    # distros alike (that is why the release builds are musl).
    case "$arch" in
      x86_64 | amd64) target="x86_64-unknown-linux-musl" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-musl" ;;
      *) target="" ;;
    esac
    ;;
  *) target="" ;;
esac

if [ -z "$target" ]; then
  err "unsupported platform: os=${os} arch=${arch}"
  err "pg_lens ships prebuilt binaries for macOS (arm64, x86_64) and Linux musl (x86_64, aarch64)."
  err "For anything else — including Windows — build from source: ${DOCS_URL}"
  exit 1
fi

# ------------------------------------------------------- version resolution

# Accept "0.15.0" and "v0.15.0"; the release assets are named with the tag.
normalize_version() {
  case "$1" in
    v*) printf '%s\n' "$1" ;;
    *) printf 'v%s\n' "$1" ;;
  esac
}

if [ -n "$VERSION" ]; then
  VERSION="$(normalize_version "$VERSION")"
else
  say "Resolving the latest pg_lens release..."
  # The releases API returns JSON; parsing "tag_name" with sed keeps jq out
  # of the dependency list. `head -1` because the payload also carries a
  # tag_name for the author/uploader-free fields on some proxies.
  latest="$(fetch_stdout "$API_LATEST" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -1)" || die "could not reach ${API_LATEST}"
  [ -n "$latest" ] || die "could not parse the latest release tag from ${API_LATEST}"
  VERSION="$(normalize_version "$latest")"
fi

ASSET="${BIN}-${VERSION}-${target}.tar.gz"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"
ASSET_URL="${BASE}/${ASSET}"
SHA_URL="${ASSET_URL}.sha256"
TARGET_PATH="${INSTALL_DIR}/${BIN}"

say ""
say "  platform : ${os} ${arch}  ->  ${target}"
say "  version  : ${VERSION}"
say "  archive  : ${ASSET_URL}"
say "  checksum : ${SHA_URL}"
say "  install  : ${TARGET_PATH}"
say ""

if [ "$DRY_RUN" -eq 1 ]; then
  say "--dry-run: nothing downloaded, nothing installed."
  exit 0
fi

# ------------------------------------------------------------ checksum tool

if command -v sha256sum >/dev/null 2>&1; then
  sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  die "neither sha256sum nor shasum found — cannot verify the download"
fi

# ------------------------------------------------- download, verify, unpack

tmp="$(mktemp -d)"
# Any exit path (success, failure, Ctrl-C) removes the scratch dir, so a
# failed verification never leaves a half-trusted binary on disk.
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT INT TERM

say "Downloading ${ASSET} ..."
fetch_file "$ASSET_URL" "$tmp/$ASSET" ||
  die "download failed: ${ASSET_URL} (is ${VERSION} a published release?)"

say "Downloading the published SHA-256 checksum ..."
fetch_file "$SHA_URL" "$tmp/$ASSET.sha256" ||
  die "checksum download failed: ${SHA_URL}"

# The sidecar is `shasum -a 256` output: "<hex>  <filename>". Compare only
# the digest — the recorded filename is the CI runner's, not ours.
expected="$(cut -d' ' -f1 "$tmp/$ASSET.sha256")"
actual="$(sha256_of "$tmp/$ASSET")"

if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  err "CHECKSUM MISMATCH — refusing to install."
  err "  expected: ${expected:-<empty>}"
  err "  actual  : ${actual}"
  err "  asset   : ${ASSET_URL}"
  err "The download was discarded. Retry; if it persists, open an issue at"
  err "https://github.com/${REPO}/issues — do not run the downloaded file."
  exit 1
fi
say "Checksum OK (${actual})"

say "Unpacking ..."
tar xzf "$tmp/$ASSET" -C "$tmp" || die "could not unpack ${ASSET}"

# The release tarballs wrap everything in a pg_lens-<tag>-<triple>/ dir
# (binary + README + LICENSE). Fall back to a root-level binary so a
# future layout change does not silently break the installer.
if [ -f "$tmp/${BIN}-${VERSION}-${target}/${BIN}" ]; then
  src="$tmp/${BIN}-${VERSION}-${target}/${BIN}"
elif [ -f "$tmp/${BIN}" ]; then
  src="$tmp/${BIN}"
else
  die "could not find the ${BIN} binary inside ${ASSET}"
fi

# ------------------------------------------------------------------ install

mkdir -p "$INSTALL_DIR" || die "could not create ${INSTALL_DIR}"

# Upgrade path: re-running the script overwrites in place, so report the
# version being replaced. An old/broken binary must not abort the install.
old_version=""
if [ -e "$TARGET_PATH" ]; then
  old_version="$("$TARGET_PATH" --version 2>/dev/null || true)"
fi

chmod +x "$src"
# Copy-then-move keeps the replacement atomic-ish and works across the
# tmpfs -> home filesystem boundary that `mv` alone can trip on.
cp "$src" "$TARGET_PATH.tmp$$" || die "could not write to ${INSTALL_DIR}"
chmod +x "$TARGET_PATH.tmp$$"
mv -f "$TARGET_PATH.tmp$$" "$TARGET_PATH" || die "could not install to ${TARGET_PATH}"

new_version="$("$TARGET_PATH" --version 2>/dev/null || echo "$BIN $VERSION")"

say ""
if [ -n "$old_version" ]; then
  say "Upgraded: ${old_version}  ->  ${new_version}"
else
  say "Installed: ${new_version}"
fi
say "  at ${TARGET_PATH}"

# ---------------------------------------------------------------- PATH hint

# Print the line, never edit an rc file — rc files belong to the user.
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    case "${SHELL:-}" in
      */zsh) rc="${HOME}/.zshrc" ;;
      */bash) rc="${HOME}/.bashrc (or ${HOME}/.bash_profile on macOS)" ;;
      */fish) rc="${HOME}/.config/fish/config.fish — use: fish_add_path ${INSTALL_DIR}" ;;
      *) rc="your shell's startup file" ;;
    esac
    say ""
    say "NOTE: ${INSTALL_DIR} is not on your PATH. Add this line to ${rc}:"
    say ""
    say "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    say ""
    ;;
esac

# ------------------------------------------------------------- next steps

say ""
say "Next:"
say "  ${BIN} --mock        # try it with no database at all"
say "  ${BIN} --dsn \"host=localhost user=postgres dbname=postgres\""
say "  Live demo in the browser: ${DEMO_URL}"

if [ "$os" = Darwin ]; then
  say ""
  say "macOS: binaries fetched with curl/wget are NOT Gatekeeper-quarantined,"
  say "so this install just runs. (Only if you later re-download a release"
  say "through a browser: xattr -d com.apple.quarantine ${TARGET_PATH})"
fi

say ""
say "Uninstall:"
say "  rm ${TARGET_PATH}"
say "  rm -rf ~/.config/pg_lens ~/.local/state/pg_lens   # config and history"
