#!/bin/sh
# Build one Verdict release archive. Used by .github/workflows/release.yml and by the clean-install
# workflow, so CI installs the exact layout the release publishes.
#
#   sh release/package.sh <version> <target> <zip|tar.gz> <path-to-built-binary> <outdir>
#
# Layout inside the archive directory (vdt-<version>-<target>/):
#   vdt or vdt.exe          the binary
#   vdt.sha256              SHA-256 of the binary (the installer checks this)
#   install.sh / install.ps1
#   README.md LICENSE NOTICE
#   integrations/dsh/       the optional DSH host integration
# And beside the archive: <archive>.sha256, the download checksum.
set -eu

version="${1:?version}"
target="${2:?target}"
archive_kind="${3:?zip or tar.gz}"
binary="${4:?path to the built vdt}"
outdir="${5:?output folder}"

repo=$(cd "$(dirname "$0")/.." && pwd)
name="vdt-$version-$target"
dir="$outdir/$name"
rm -rf "$dir"
mkdir -p "$dir/integrations"
cp "$repo/README.md" "$repo/LICENSE" "$repo/NOTICE" "$dir/"
if [ -d "$repo/integrations/dsh" ]; then
  cp -r "$repo/integrations/dsh" "$dir/integrations/dsh"
fi

case "$target" in
  *windows*) exe="vdt.exe" ;;
  *) exe="vdt" ;;
esac
cp "$binary" "$dir/$exe"
chmod +x "$dir/$exe" 2>/dev/null || true

if command -v sha256sum >/dev/null 2>&1; then
  sha_cmd="sha256sum"
else
  sha_cmd="shasum -a 256"
fi
# Hash a bare file name from inside its folder: coreutils escapes a path containing a backslash
# with a leading `\`, which would end up inside the .sha256 file.
sum=$( cd "$dir" && $sha_cmd "$exe" | cut -d' ' -f1 )
printf '%s\n' "$sum" > "$dir/$exe.sha256"

if [ "$exe" = "vdt.exe" ]; then
  cp "$repo/release/install.ps1" "$dir/"
  ( cd "$outdir" && 7z a "$name.zip" "$name" >/dev/null )
  file="$outdir/$name.zip"
else
  cp "$repo/release/install.sh" "$dir/"
  chmod +x "$dir/install.sh"
  ( cd "$outdir" && tar -czf "$name.tar.gz" "$name" )
  file="$outdir/$name.tar.gz"
fi

# The download checksum covers the outer archive, the bytes a person downloads.
( cd "$outdir" && $sha_cmd "$(basename "$file")" | cut -d' ' -f1 ) > "$file.sha256"
printf 'built %s and %s.sha256\n' "$file" "$file"
