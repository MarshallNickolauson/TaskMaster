#!/bin/bash
# TaskMaster installer for macOS.
#   curl -fsSL https://taskmaster.mdnlabs.dev/install.sh | bash
#
# Downloads the latest release from GitHub with curl (so macOS does not tag it as "downloaded from the
# internet"), installs it and opens it. Read it first if you like: it only fetches a release asset from
# github.com/MarshallNickolauson/TaskMaster and copies it into place.
set -euo pipefail

BASE="https://github.com/MarshallNickolauson/TaskMaster/releases/latest/download"
os="$(uname -s)"; arch="$(uname -m)"
tmp="$(mktemp -d)"; mnt=""
cleanup() { [ -n "$mnt" ] && hdiutil detach -quiet "$mnt" 2>/dev/null || true; rm -rf "$tmp"; }
trap cleanup EXIT

case "$os" in
  Darwin)
    [ "$arch" = "arm64" ] && name="taskmaster-macos-arm64.dmg" || name="taskmaster-macos-intel.dmg"
    echo "Downloading TaskMaster..."
    curl -fL --progress-bar -o "$tmp/os.dmg" "$BASE/$name" || { echo "Could not download $name. See https://taskmaster.mdnlabs.dev/download" >&2; exit 1; }
    mnt="$(hdiutil attach -nobrowse -readonly -noautoopen "$tmp/os.dmg" | awk -F'\t' '/\/Volumes\//{print $NF}' | tail -1)"
    [ -d "$mnt/TaskMaster.app" ] || { echo "The disk image did not contain TaskMaster.app." >&2; exit 1; }
    osascript -e 'tell application "TaskMaster" to quit' >/dev/null 2>&1 || true
    dest="/Applications"; [ -w "$dest" ] || { dest="$HOME/Applications"; mkdir -p "$dest"; }
    rm -rf "$dest/TaskMaster.app"; cp -R "$mnt/TaskMaster.app" "$dest/"
    xattr -dr com.apple.quarantine "$dest/TaskMaster.app" 2>/dev/null || true
    echo "Installed to $dest/TaskMaster.app"
    open "$dest/TaskMaster.app"
    ;;
  *) echo "Unsupported system: $os. TaskMaster is macOS-only for now (Windows is planned)." >&2; exit 1 ;;
esac
