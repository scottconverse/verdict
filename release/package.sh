#!/bin/sh
# Build one Verdict release archive. Used by .github/workflows/release.yml and by the clean-install
# workflow, so CI installs the exact layout the release publishes.
#
#   sh release/package.sh <version> <target> <zip|tar.gz|dir> <path-to-built-binary> <outdir>
#
# Layout inside the archive directory (vdt-<version>-<target>/):
#   vdt or vdt.exe          the core
#   vdt.sha256              SHA-256 of the core (the installer checks this)
#   Verdict-Setup.exe       Windows double-click entry point (same program, setup mode)
#   vdt-setup               Unix double-click entry point (same program)
#   Verdict.app/            macOS app bundle (macOS targets only)
#   install.sh / install.ps1  legacy/silent scripts (the wizard is the entry point)
#   README.md LICENSE NOTICE
#   integrations/dsh/       the optional DSH host integration
# And beside the archive: <archive>.sha256, the download checksum.
set -eu

version="${1:?version}"
target="${2:?target}"
archive_kind="${3:?zip, tar.gz or dir}"
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
  *windows*) exe="vdt.exe"; setup="Verdict-Setup.exe" ;;
  *) exe="vdt"; setup="vdt-setup" ;;
esac
cp "$binary" "$dir/$exe"
cp "$binary" "$dir/$setup"
chmod +x "$dir/$exe" "$dir/$setup" 2>/dev/null || true

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
elif [ "$archive_kind" != "dir" ]; then
  cp "$repo/release/install.sh" "$dir/"
  chmod +x "$dir/install.sh"
fi

# macOS: a real double-clickable app bundle. Its executable is the setup program, which starts the
# wizard and installs the core that sits beside it.
case "$target" in
  *apple-darwin*)
    app="$dir/Verdict.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$binary" "$app/Contents/MacOS/vdt"
    cp "$binary" "$app/Contents/MacOS/vdt-setup"
    cp "$dir/$exe.sha256" "$app/Contents/MacOS/$exe.sha256"
    chmod +x "$app/Contents/MacOS/vdt" "$app/Contents/MacOS/vdt-setup"
    # Re-opening the installed app opens the project chosen at install (recorded by setup in
    # $VERDICT_STATE_DIR/last-project, default ~/.verdict). The wizard runs only when no project
    # has been recorded yet, or the recorded folder is gone.
    cat > "$app/Contents/MacOS/vdt-launch" <<'LAUNCH'
#!/bin/sh
HERE=$(dirname "$0")
state="${VERDICT_STATE_DIR:-$HOME/.verdict}/last-project"
if [ -f "$state" ]; then
  IFS= read -r project < "$state" || project=""
  if [ -n "$project" ] && [ -d "$project" ]; then
    exec "$HERE/vdt" serve --open --project "$project"
  fi
fi
exec "$HERE/vdt-setup"
LAUNCH
    chmod +x "$app/Contents/MacOS/vdt-launch"
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Verdict</string>
  <key>CFBundleDisplayName</key><string>Verdict</string>
  <key>CFBundleIdentifier</key><string>com.scottconverse.verdict</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>vdt-launch</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
PLIST
    ;;
esac

case "$archive_kind" in
  zip)
    ( cd "$outdir" && 7z a "$name.zip" "$name" >/dev/null )
    file="$outdir/$name.zip"
    ;;
  tar.gz)
    ( cd "$outdir" && tar -czf "$name.tar.gz" "$name" )
    file="$outdir/$name.tar.gz"
    ;;
  dir)
    printf 'built %s\n' "$dir"
    exit 0
    ;;
  *)
    echo "unknown archive kind: $archive_kind" >&2
    exit 1
    ;;
esac

# The download checksum covers the outer archive, the bytes a person downloads. It is written in
# the standard `<hash>  <name>` form so `sha256sum -c` reads it; the redirect stays outside the
# subshell so a relative output folder is not applied twice.
sum_line=$( cd "$outdir" && $sha_cmd "$(basename "$file")" )
printf '%s\n' "$sum_line" > "$file.sha256"
printf 'built %s and %s.sha256\n' "$file" "$file"
