#!/bin/sh
# Build the Linux double-clickable installer: an AppImage whose AppRun starts Verdict's setup
# wizard. Requires appimagetool (path in APPIMAGETOOL, or on PATH).
#
#   sh release/package-appimage.sh <version> <path-to-built-binary> <outdir>
set -eu

version="${1:?version}"
binary="${2:?path to the built vdt}"
outdir="${3:?output folder}"

repo=$(cd "$(dirname "$0")/.." && pwd)
tool="${APPIMAGETOOL:-appimagetool}"
command -v "$tool" >/dev/null 2>&1 || { echo "appimagetool not found (set APPIMAGETOOL)" >&2; exit 1; }

appdir="$outdir/Verdict.AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/icons/hicolor/scalable/apps"
cp "$binary" "$appdir/usr/bin/vdt"
cp "$binary" "$appdir/usr/bin/vdt-setup"
chmod +x "$appdir/usr/bin/vdt" "$appdir/usr/bin/vdt-setup"
if command -v sha256sum >/dev/null 2>&1; then
  ( cd "$appdir/usr/bin" && sha256sum vdt | cut -d' ' -f1 > vdt.sha256 )
else
  ( cd "$appdir/usr/bin" && shasum -a 256 vdt | cut -d' ' -f1 > vdt.sha256 )
fi

cat > "$appdir/usr/share/applications/verdict.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Verdict
Comment=Install Verdict and open the page for a project
Exec=vdt-setup
Terminal=false
Categories=Development;
Icon=verdict
DESKTOP

cat > "$appdir/usr/share/icons/hicolor/scalable/apps/verdict.svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <rect width="256" height="256" rx="48" fill="#111111"/>
  <path d="M52 64 L108 192 L128 192 L184 64 L150 64 L118 148 L86 64 Z" fill="#ffffff"/>
</svg>
SVG
cp "$appdir/usr/share/icons/hicolor/scalable/apps/verdict.svg" "$appdir/verdict.svg"
cp "$appdir/usr/share/applications/verdict.desktop" "$appdir/verdict.desktop"

cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
exec "$HERE/usr/bin/vdt-setup" "$@"
APPRUN
chmod +x "$appdir/AppRun"

file="$outdir/Verdict-$version-x86_64.AppImage"
# CI has no FUSE, so ask the AppImage runtime to extract and run the tool.
if ! "$tool" --appimage-extract-and-run "$appdir" "$file" 2>/dev/null; then
  "$tool" "$appdir" "$file"
fi
chmod +x "$file"
if command -v sha256sum >/dev/null 2>&1; then
  ( cd "$outdir" && sha256sum "$(basename "$file")" | cut -d' ' -f1 > "$file.sha256" )
else
  ( cd "$outdir" && shasum -a 256 "$(basename "$file")" | cut -d' ' -f1 > "$file.sha256" )
fi
printf 'built %s and %s.sha256\n' "$file" "$file"
