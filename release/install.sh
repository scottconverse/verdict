#!/bin/sh
# Verdict installer for Linux and macOS. Run it from the downloaded release folder:
#   sh install.sh
# It verifies the binary against the checksum beside it, installs it in ~/.local/bin, and starts
# the local page. No terminal step is needed after it finishes.
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

bin="$here/vdt-$version-$arch_part-$os_part"
[ -f "$bin" ] || { echo "cannot find $bin beside the installer" >&2; exit 1; }
if [ -f "$bin.sha256" ]; then
  want=$(cut -d' ' -f1 "$bin.sha256")
  if command -v sha256sum >/dev/null 2>&1; then
    got=$(sha256sum "$bin" | cut -d' ' -f1)
  else
    got=$(shasum -a 256 "$bin" | cut -d' ' -f1)
  fi
  [ "$want" = "$got" ] || { echo "checksum mismatch: the download is damaged or was changed" >&2; exit 1; }
else
  echo "warning: no checksum beside $bin; the download is unverified" >&2
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

# Register the MCP server with the project the installer was run in. Failures are warnings: the
# binary is installed and the page still works.
if [ "${VDT_NO_MCP:-0}" != "1" ]; then
  "$dir/vdt" mcp-install claude --project "$PWD" --i-am-the-human --vdt-path "$dir/vdt" \
    || echo "warning: could not register the MCP server with Claude Code" >&2
  "$dir/vdt" mcp-install cursor --project "$PWD" --i-am-the-human --vdt-path "$dir/vdt" \
    || echo "warning: could not register the MCP server with Cursor" >&2
fi

echo "Starting the Verdict page..."
"$dir/vdt" serve >"$HOME/.verdict-serve.log" 2>&1 &
sleep 1
if command -v xdg-open >/dev/null 2>&1; then xdg-open "http://127.0.0.1:4717/" >/dev/null 2>&1 || true
elif command -v open >/dev/null 2>&1; then open "http://127.0.0.1:4717/" || true
fi
echo "Verdict is running at http://127.0.0.1:4717/"
