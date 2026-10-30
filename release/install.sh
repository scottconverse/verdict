#!/bin/sh
# Verdict installer for Linux and macOS. Run it from the extracted release folder:
#   sh install.sh
#
# The release archive holds the binary (`vdt`) and its SHA-256 (`vdt.sha256`); older release
# layouts named the binary `vdt-<version>-<arch>-<os>`. This installer finds either, verifies the
# binary against the checksum beside it, installs it in ~/.local/bin, registers the MCP server with
# the project it was run in, and starts the local page. No terminal step is needed after it.
#
# A missing checksum is an error, not a warning: an unverified download is not installed unless
# VDT_ALLOW_UNVERIFIED=1 is set on purpose.
set -eu

version="${VDT_VERSION:-1.0.0}"
here=$(cd "$(dirname "$0")" && pwd)

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)  os_part="unknown-linux-gnu" ;;
  Darwin) os_part="apple-darwin" ;;
  *) echo "unsupported system: $os" >&2; exit 1 ;;
esac
case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac

# The archive layout first (`vdt`), then the older versioned name, then the same in this folder's
# parent (an archive extracted one level up).
bin=""
for candidate in \
  "$here/vdt" \
  "$here/vdt-$version-$arch_part-$os_part" \
  "$(dirname "$here")/vdt" \
  "$(dirname "$here")/vdt-$version-$arch_part-$os_part"
do
  if [ -f "$candidate" ]; then bin="$candidate"; break; fi
done
[ -n "$bin" ] || { echo "cannot find the vdt binary beside the installer ($here)" >&2; exit 1; }

sum="$bin.sha256"
if [ -f "$sum" ]; then
  want=$(cut -d' ' -f1 "$sum")
  if command -v sha256sum >/dev/null 2>&1; then
    got=$(sha256sum "$bin" | cut -d' ' -f1)
  else
    got=$(shasum -a 256 "$bin" | cut -d' ' -f1)
  fi
  [ "$want" = "$got" ] || { echo "checksum mismatch: the download is damaged or was changed" >&2; exit 1; }
elif [ "${VDT_ALLOW_UNVERIFIED:-0}" = "1" ]; then
  echo "warning: no checksum beside $bin and VDT_ALLOW_UNVERIFIED=1; the download is unverified" >&2
else
  echo "no checksum beside $bin: refusing to install an unverified download (set VDT_ALLOW_UNVERIFIED=1 to override)" >&2
  exit 1
fi

dir="$HOME/.local/bin"
mkdir -p "$dir"
cp "$bin" "$dir/vdt"
chmod +x "$dir/vdt"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to PATH to use vdt from a shell (the page does not need it)." ;;
esac

echo "Installed $dir/vdt"

# Register the MCP server with the project the installer was run in. vdt asks the person at the
# terminal to type yes for each host. Failures are warnings: the binary is installed and the page
# still works.
if [ "${VDT_NO_MCP:-0}" != "1" ]; then
  "$dir/vdt" mcp-install claude --project "$PWD" --i-am-the-human --vdt-path "$dir/vdt" \
    || echo "warning: could not register the MCP server with Claude Code" >&2
  "$dir/vdt" mcp-install cursor --project "$PWD" --i-am-the-human --vdt-path "$dir/vdt" \
    || echo "warning: could not register the MCP server with Cursor" >&2
fi

echo "Starting the Verdict page for $PWD..."
# The page opens its own one-time link in the browser (--open). A plain address is refused.
"$dir/vdt" serve --open --project "$PWD" >"$HOME/.verdict-serve.log" 2>&1 &
echo "Verdict is running on 127.0.0.1:4717 (project: $PWD). Your browser opens it."
