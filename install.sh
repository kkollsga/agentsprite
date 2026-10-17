#!/bin/sh
# Install pxl (AgentSprite) from a GitHub Release: macOS and Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/kkollsga/agentsprite/main/install.sh | sh -s -- --version v0.1.0
#
# Options (or the environment variable in brackets):
#   --version vX.Y.Z   install that release instead of the latest      [PXL_VERSION]
#   --dir DIR          install the binary into DIR                      [PXL_INSTALL_DIR, default ~/.local/bin]
#   --no-skills        do not run `pxl skill install` afterwards        [PXL_INSTALL_SKILLS=0]
#   --help
#
# Testing and mirrors: PXL_RELEASE_BASE_URL replaces https://github.com/kkollsga/agentsprite/releases and must hold
# download/<tag>/<asset> (and LATEST, a text file with the newest tag, when no version is given), file:// works.
# PXL_TARGET forces the target triple instead of detecting it.
set -eu

REPO="kkollsga/agentsprite"
BASE="${PXL_RELEASE_BASE_URL:-https://github.com/$REPO/releases}"
VERSION="${PXL_VERSION:-}"
DIR="${PXL_INSTALL_DIR:-${HOME:-.}/.local/bin}"
SKILLS="${PXL_INSTALL_SKILLS:-1}"

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: error: %s\n' "$*" >&2; exit 1; }

usage() {
  sed -n '2,15p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || true
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
    --dir) [ $# -ge 2 ] || die "--dir needs a value"; DIR="$2"; shift 2 ;;
    --no-skills) SKILLS=0; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required but not found"; }
need curl
need tar
need uname

detect_target() {
  if [ -n "${PXL_TARGET:-}" ]; then
    printf '%s' "$PXL_TARGET"
    return
  fi
  os=$(uname -s)
  arch=$(uname -m)
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported CPU architecture: $arch (releases exist for x86_64 and aarch64)" ;;
  esac
  case "$os" in
    Linux) printf '%s-unknown-linux-musl' "$arch" ;;
    Darwin) printf '%s-apple-darwin' "$arch" ;;
    MINGW* | MSYS* | CYGWIN*) die "on Windows use install.ps1 (PowerShell): irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
    *) die "unsupported operating system: $os" ;;
  esac
}

# First 64-hex token of the input (GNU sha256sum prefixes the line with a backslash when the file name has one).
first_hex() {
  grep -Eo '[0-9a-fA-F]{64}' | head -n 1 | tr 'A-F' 'a-f'
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | first_hex
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | first_hex
  else
    die "sha256sum or shasum is required to verify the download"
  fi
}

resolve_latest() {
  case "$BASE" in
    https://github.com/*)
      url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$BASE/latest") || die "cannot reach $BASE/latest"
      tag=${url##*/}
      ;;
    *)
      tag=$(curl -fsSL "$BASE/LATEST") || die "cannot read $BASE/LATEST"
      tag=$(printf '%s' "$tag" | tr -d '[:space:]')
      ;;
  esac
  [ -n "$tag" ] || die "could not determine the latest release"
  printf '%s' "$tag"
}

TARGET=$(detect_target)
if [ -z "$VERSION" ]; then
  VERSION=$(resolve_latest)
fi
case "$VERSION" in v*) ;; *) VERSION="v$VERSION" ;; esac

ASSET="pxl-$VERSION-$TARGET.tar.gz"
URL="$BASE/download/$VERSION/$ASSET"
say "pxl installer: $VERSION for $TARGET"

TMP=$(mktemp -d 2>/dev/null || mktemp -d -t pxl-install)
trap 'rm -rf "$TMP"' EXIT INT TERM

say "downloading $URL"
curl -fsSL -o "$TMP/$ASSET" "$URL" || die "download failed: $URL (is $VERSION a release with a $TARGET build?)"
curl -fsSL -o "$TMP/$ASSET.sha256" "$URL.sha256" || die "download failed: $URL.sha256"

want=$(first_hex < "$TMP/$ASSET.sha256")
have=$(sha256_of "$TMP/$ASSET")
[ -n "$want" ] || die "empty checksum file"
if [ "$want" != "$have" ]; then
  die "checksum mismatch for $ASSET: expected $want, got $have; nothing was installed"
fi
say "sha256 verified: $have"

mkdir -p "$TMP/x"
tar -xzf "$TMP/$ASSET" -C "$TMP/x" || die "cannot unpack $ASSET"
[ -f "$TMP/x/pxl" ] || die "the archive has no pxl binary"

mkdir -p "$DIR" || die "cannot create $DIR (use --dir or PXL_INSTALL_DIR)"
cp "$TMP/x/pxl" "$DIR/pxl.new" || die "cannot write to $DIR"
chmod 755 "$DIR/pxl.new"
mv -f "$DIR/pxl.new" "$DIR/pxl"
say "installed: $DIR/pxl ($("$DIR/pxl" --version 2>/dev/null || echo 'version unknown'))"

case ":${PATH:-}:" in
  *":$DIR:"*) ;;
  *)
    say ""
    say "$DIR is not on your PATH. Add it, for example:"
    say "  echo 'export PATH=\"$DIR:\$PATH\"' >> ~/.profile   # then open a new shell"
    ;;
esac

if [ "$SKILLS" = "0" ]; then
  say ""
  say "skills not installed (--no-skills). Install them later with: pxl skill install"
else
  say ""
  say "installing the agent skills for Claude Code and Codex (disable with --no-skills):"
  "$DIR/pxl" skill install || say "skill install failed; run 'pxl skill install' yourself (an unmanaged skill folder is never replaced)"
fi

say ""
say "next: pxl --version, pxl console list, pxl guide"
