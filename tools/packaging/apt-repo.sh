#!/usr/bin/env bash
# Build a small apt repository (no apt-utils needed: only ar, tar, gzip and the *sum tools) for the .deb files given on the command line.
#
#   tools/packaging/apt-repo.sh <repo-dir> <deb>...
#
# Layout: <repo-dir>/pool/main/r/rusty-video-player/*.deb, <repo-dir>/dists/stable/main/binary-amd64/Packages{,.gz} and
# <repo-dir>/dists/stable/Release. `cargo xtask dist apt-repo` then signs Release (InRelease and Release.gpg) with the release key.
# Files are overwritten in place; nothing is deleted.
set -euo pipefail
repo=${1:?usage: apt-repo.sh <repo-dir> <deb>...}
shift
[ "$#" -ge 1 ] || { echo "no .deb given"; exit 2; }
suite=stable comp=main arch=amd64
pooldir=$repo/pool/$comp/r/rusty-video-player
bindir=$repo/dists/$suite/$comp/binary-$arch
mkdir -p "$pooldir" "$bindir"
: > "$bindir/Packages"
for deb in "$@"; do
  name=$(basename "$deb")
  cp "$deb" "$pooldir/$name"
  # The control file inside control.tar.* (xz, gz or zst).
  member=$(ar t "$deb" | grep '^control\.tar')
  case "$member" in
    *.xz) dec=(xz -dc) ;; *.gz) dec=(gzip -dc) ;; *.zst) dec=(zstd -dc) ;; *) dec=(cat) ;;
  esac
  ctl=$(ar p "$deb" "$member" | "${dec[@]}" | tar -xO ./control)
  {
    printf '%s\n' "$ctl" | sed -e '/^[[:space:]]*$/d'
    echo "Filename: pool/$comp/r/rusty-video-player/$name"
    echo "Size: $(stat -c %s "$deb")"
    echo "MD5sum: $(md5sum "$deb" | cut -d' ' -f1)"
    echo "SHA1: $(sha1sum "$deb" | cut -d' ' -f1)"
    echo "SHA256: $(sha256sum "$deb" | cut -d' ' -f1)"
    echo
  } >> "$bindir/Packages"
done
gzip -9 -n -c "$bindir/Packages" > "$bindir/Packages.gz"
date=$(date -u -R -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" | sed 's/+0000/UTC/')
{
  echo "Origin: Rusty Video Player"
  echo "Label: Rusty Video Player"
  echo "Suite: $suite"
  echo "Codename: $suite"
  echo "Date: $date"
  echo "Architectures: $arch"
  echo "Components: $comp"
  echo "Description: Rusty Video Player (local test repository)"
  for h in MD5Sum:md5sum SHA1:sha1sum SHA256:sha256sum; do
    echo "${h%%:*}:"
    for f in "$comp/binary-$arch/Packages" "$comp/binary-$arch/Packages.gz"; do
      p=$repo/dists/$suite/$f
      printf ' %s %16d %s\n' "$(${h##*:} "$p" | cut -d' ' -f1)" "$(stat -c %s "$p")" "$f"
    done
  done
} > "$repo/dists/$suite/Release"
echo "$repo/dists/$suite/Release"
