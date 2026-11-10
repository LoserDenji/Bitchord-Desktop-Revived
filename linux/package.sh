#!/bin/sh
# Build the Linux packages from a built binary:
#   linux/package.sh target/release/bitchord 2.0.2 dist
# Writes into the out dir:
#   BitChord-<v>-linux-amd64.AppImage — one file that runs on any distro: WebKitGTK
#     (sign-in windows), GTK, the tray's appindicator and ALSA are packed inside.
#   BitChord-<v>-linux-amd64.deb      — for Debian, Ubuntu, Mint and friends.
#   BitChord-<v>-linux-x86_64.rpm     — for Fedora, openSUSE and friends.
# The deb and rpm expect the runtime libraries from the system (see the
# Depends/Requires lines below); the AppImage carries them all.
#
# Needs on the build box: dpkg-deb, rpmbuild, curl, desktop-file-utils, file,
# and the WebKitGTK 4.1 runtime installed (linuxdeploy reads it from there).
set -e

bin=$1
version=$2
out=$3
[ -n "$bin" ] && [ -n "$version" ] && [ -n "$out" ] || { echo "usage: $0 <binary> <version> <out dir>"; exit 1; }
[ -f "$bin" ] || { echo "no such binary: $bin"; exit 1; }

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
icon="$root/assets/icon/icon.png"
mkdir -p "$out"
out=$(cd "$out" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The menu entry and the icon go into every package.
desktop=$work/bitchord.desktop
cp "$here/bitchord.desktop" "$desktop"
iconpath=usr/share/icons/hicolor/256x256/apps/bitchord.png

# ---------------------------------------------------------------- AppImage ----
# linuxdeploy packs the binary with every library it links (the way Tauri's
# AppImages are built), and its GTK plugin wires up the input-method, pixbuf
# and gio modules. Two passes: deploy first, then add WebKit's helper
# processes — which linuxdeploy can't see — and only then build the image.
curl -sSLo "$work/linuxdeploy.AppImage" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
curl -sSLo "$work/plugin-gtk.AppImage" \
    https://github.com/linuxdeploy/linuxdeploy-plugin-gtk/releases/download/continuous/linuxdeploy-plugin-gtk-x86_64.AppImage
chmod +x "$work/linuxdeploy.AppImage" "$work/plugin-gtk.AppImage"

appdir=$work/AppDir
mkdir -p "$appdir/usr/bin"
cp "$bin" "$appdir/usr/bin/bitchord"
chmod 755 "$appdir/usr/bin/bitchord"
cp "$desktop" "$appdir/bitchord.desktop"
mkdir -p "$appdir/$(dirname "$iconpath")"
cp "$icon" "$appdir/$iconpath"

# Sourced by the AppRun linuxdeploy generates, after its GTK hook: the tray
# and webview libraries can land in the multiarch dir, and WebKit's sign-in
# windows need a hand from outside the sandbox.
mkdir -p "$appdir/apprun-hooks"
cat > "$appdir/apprun-hooks/zz-bitchord.sh" <<'HOOK'
# WebKit looks its helpers up next to its own library, which may sit in the
# multiarch dir that the stock AppRun doesn't put on the search path.
export LD_LIBRARY_PATH="${APPDIR}/usr/lib/$(uname -m)-linux-gnu:${APPDIR}/usr/lib:${LD_LIBRARY_PATH}"
# The WebKit sandbox needs bubblewrap, which isn't inside the AppImage; turn
# it off (only the sign-in windows use WebKit) unless the user asks for it.
export WEBKIT_FORCE_SANDBOX="${WEBKIT_FORCE_SANDBOX:-0}"
# Older NVIDIA drivers crash WebKit's DMABUF renderer; the main UI never uses
# WebKit, so prefer the safe path.
export WEBKIT_DISABLE_DMABUF_RENDERER="${WEBKIT_DISABLE_DMABUF_RENDERER:-1}"
HOOK

# The runners have no FUSE; this makes linuxdeploy and the plugin self-extract.
export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH=x86_64
export LINUXDEPLOY_PLUGIN_GTK="$work/plugin-gtk.AppImage"

"$work/linuxdeploy.AppImage" \
    --appdir "$appdir" \
    --executable "$appdir/usr/bin/bitchord" \
    --desktop-file "$appdir/bitchord.desktop" \
    --icon-file "$appdir/$iconpath"

webkit_lib=$(find "$appdir/usr/lib" -name 'libwebkit2gtk-4.1.so.*' | head -n 1)
[ -n "$webkit_lib" ] || { echo "WebKitGTK was not deployed - is libwebkit2gtk-4.1-0 installed?"; exit 1; }
helper_src=/usr/lib/$(uname -m)-linux-gnu/webkit2gtk-4.1
helper_dst=$(dirname "$webkit_lib")/webkit2gtk-4.1
mkdir -p "$helper_dst"
set --
for proc in WebKitWebProcess WebKitNetworkProcess; do
    cp "$helper_src/$proc" "$helper_dst/$proc"
    set -- "$@" --deploy-deps-only "$helper_dst/$proc"
done
OUTPUT="$out/BitChord-$version-linux-amd64.AppImage" \
    "$work/linuxdeploy.AppImage" --appdir "$appdir" "$@" --output appimage

# --------------------------------------------------------------------- deb ----
deb=$work/deb
mkdir -p "$deb/DEBIAN" "$deb/usr/bin" "$deb/usr/share/applications" "$deb/$(dirname "$iconpath")"
cp "$bin" "$deb/usr/bin/bitchord"
chmod 755 "$deb/usr/bin/bitchord"
cp "$desktop" "$deb/usr/share/applications/bitchord.desktop"
cp "$icon" "$deb/$iconpath"
cat > "$deb/DEBIAN/control" <<EOF
Package: bitchord
Version: $version
Section: sound
Priority: optional
Architecture: amd64
Maintainer: LoserDenji <LoserDenji@users.noreply.github.com>
Homepage: https://github.com/LoserDenji/Bitchord-Desktop-Revived
Depends: libgtk-3-0, libwebkit2gtk-4.1-0, libayatana-appindicator3-1, libdbus-1-3, libasound2 | libasound2t64, libssl3 | libssl3t64
Description: Native BitChord client
 A YouTube Music desktop client: search, browse and play music from
 YouTube Music, with word-synced lyrics, themes, Listen Together and
 Spotify transfer.
EOF
cat > "$deb/DEBIAN/postinst" <<'EOF'
#!/bin/sh
update-desktop-database -q /usr/share/applications 2>/dev/null || true
gtk-update-icon-cache -q -t /usr/share/icons/hicolor 2>/dev/null || true
exit 0
EOF
chmod 755 "$deb/DEBIAN/postinst"
dpkg-deb --root-owner-group --build "$deb" "$out/BitChord-$version-linux-amd64.deb"

# --------------------------------------------------------------------- rpm ----
# An rpm version may not contain '-'; a pre-release tag becomes '~' (2.1~beta).
rpmv=$(printf '%s' "$version" | tr '-' '~')
top=$work/rpmbuild
mkdir -p "$top/BUILD" "$top/RPMS" "$top/SOURCES" "$top/SPECS" "$top/SRPMS"
srcdir=$top/src/bitchord-$rpmv
mkdir -p "$srcdir"
cp "$bin" "$srcdir/bitchord"
chmod 755 "$srcdir/bitchord"
cp "$desktop" "$srcdir/bitchord.desktop"
cp "$icon" "$srcdir/bitchord.png"
tar -C "$srcdir" -czf "$top/SOURCES/bitchord-$rpmv.tar.gz" bitchord bitchord.desktop bitchord.png
cat > "$top/SPECS/bitchord.spec" <<EOF
Name:           bitchord
Version:        $rpmv
Release:        1
Summary:        Native BitChord client
License:        GPL-3.0-only
URL:            https://github.com/LoserDenji/Bitchord-Desktop-Revived
Source0:        bitchord-$rpmv.tar.gz
BuildArch:      x86_64
AutoReqProv:    no
Requires:       gtk3, alsa-lib, dbus-libs
Requires:       (webkit2gtk4.1 or libwebkit2gtk-4_1-0)
Requires:       (libayatana-appindicator-gtk3 or libayatana-appindicator3-1)
Requires:       (openssl-libs or libopenssl3 or libopenssl-3)
%description
A YouTube Music desktop client: search, browse and play music from YouTube
Music, with word-synced lyrics, themes, Listen Together and Spotify transfer.
%prep
%setup -c
%install
mkdir -p %{buildroot}%{_bindir} %{buildroot}%{_datadir}/applications %{buildroot}%{_datadir}/icons/hicolor/256x256/apps
install -m 755 bitchord %{buildroot}%{_bindir}/bitchord
install -m 644 bitchord.desktop %{buildroot}%{_datadir}/applications/bitchord.desktop
install -m 644 bitchord.png %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/bitchord.png
%files
%{_bindir}/bitchord
%{_datadir}/applications/bitchord.desktop
%{_datadir}/icons/hicolor/256x256/apps/bitchord.png
EOF
rpmbuild -bb --define "_topdir $top" "$top/SPECS/bitchord.spec" > "$work/rpmbuild.log" 2>&1 \
    || { tail -40 "$work/rpmbuild.log"; exit 1; }
cp "$top"/RPMS/x86_64/bitchord-$rpmv-1.x86_64.rpm "$out/BitChord-$version-linux-x86_64.rpm"

ls -lh "$out"
