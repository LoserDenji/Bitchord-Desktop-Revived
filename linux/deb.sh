#!/bin/sh
# Build the .deb: the app in /usr/lib/bitchord, a `bitchord` command, and a menu entry
# with its icon. Beside the app goes deb.txt, which has it tell you when a new version
# is out instead of updating itself (apt owns the files). A new .deb installs over the
# old one.
#
#   linux/deb.sh <binary> <version> <out dir>
#
# Needs dpkg-deb and dpkg-shlibdeps (dpkg-dev), which work out the libraries it depends on.
set -e

bin=$1
version=$2
out=$3
[ -f "$bin" ] && [ -n "$version" ] && [ -n "$out" ] || { echo "usage: $0 <binary> <version> <out dir>"; exit 1; }
root=$(cd "$(dirname "$0")/.." && pwd)

# A nightly (2.1.0-nightly.20261111) comes after its version, and before the next, as
# in the app's own updater.
debver=$(echo "$version" | sed 's/-nightly\./+nightly./')
case $(uname -m) in
    x86_64) arch=amd64 ;;
    aarch64) arch=arm64 ;;
    *) arch=$(uname -m) ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
pkg=$work/pkg

install -Dm755 "$bin" "$pkg/usr/lib/bitchord/bitchord"
echo "Installed from the .deb. This file has bitchord tell you about new versions instead of updating itself." > "$pkg/usr/lib/bitchord/deb.txt"
mkdir -p "$pkg/usr/bin"
ln -s ../lib/bitchord/bitchord "$pkg/usr/bin/bitchord"
install -Dm644 "$root/linux/bitchord.desktop" "$pkg/usr/share/applications/bitchord.desktop"
install -Dm644 "$root/assets/icon/icon.png" "$pkg/usr/share/icons/hicolor/256x256/apps/bitchord.png"
install -Dm644 "$root/LICENSE" "$pkg/usr/share/doc/bitchord/copyright"
chmod 644 "$pkg/usr/lib/bitchord/deb.txt"

# The libraries it links, at the versions it was built against. The tray's
# appindicator is loaded at run time, so it's added by hand.
mkdir -p "$work/debian"
printf 'Source: bitchord\n\nPackage: bitchord\nArchitecture: any\n' > "$work/debian/control"
shlibs=$(cd "$work" && dpkg-shlibdeps -O "$pkg/usr/lib/bitchord/bitchord" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
[ -n "$shlibs" ] || { echo "dpkg-shlibdeps found no libraries"; exit 1; }
depends="$shlibs, libayatana-appindicator3-1 | libappindicator3-1"

mkdir -p "$pkg/DEBIAN"
cat > "$pkg/DEBIAN/control" <<EOF
Package: bitchord
Version: $debver
Architecture: $arch
Maintainer: LoserDenji <LoserDenji@users.noreply.github.com>
Installed-Size: $(du -sk "$pkg/usr" | cut -f1)
Depends: $depends
Section: sound
Priority: optional
Homepage: https://github.com/LoserDenji/Bitchord-Desktop-Revived
Description: Native BitChord client
 A YouTube Music desktop client: search, browse and play music from
 YouTube Music, with word-synced lyrics, themes, Listen Together and
 Spotify transfer.
EOF

mkdir -p "$out"
deb="$out/bitchord_${debver}_$arch.deb"
dpkg-deb --build --root-owner-group "$pkg" "$deb"
echo "Built $deb"
