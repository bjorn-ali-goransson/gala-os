#!/bin/sh
# mkchroot.sh BASE.tar.gz DIR PACKAGE...: a fresh Ubuntu 26.04 (resolute)
# chroot in DIR from Ubuntu's base image (cdimage.ubuntu.com/ubuntu-base/
# releases/26.04/release/; check SHA256SUMS against SHA256SUMS.gpg with
# gpgv and the archive keyring first), apt on the release pocket only (what
# LINUX_ROOTS pins), the packages installed without recommends, with strace;
# /proc, /dev and /dev/pts mounted. As root. WORKFLOW "Linux roots from
# Ubuntu 26.04".
set -e
base=$1 dir=$2
shift 2
[ -e "$dir" ] && { echo "$dir exists" >&2; exit 1; }
mkdir -p "$dir"
tar -xzf "$base" -C "$dir"
cat >"$dir/etc/apt/sources.list.d/ubuntu.sources" <<'SRC'
# The release pocket only: what gala's Linux roots pin.
Types: deb
URIs: http://archive.ubuntu.com/ubuntu/
Suites: resolute
Components: main universe
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
SRC
proxy=${http_proxy:-${HTTP_PROXY:-}}
[ -n "$proxy" ] && echo "Acquire::http::Proxy \"$proxy\";" >"$dir/etc/apt/apt.conf.d/90proxy"
cp /etc/resolv.conf "$dir/etc/resolv.conf"
mount -t proc proc "$dir/proc"
mount --bind /dev "$dir/dev"
mount --bind /dev/pts "$dir/dev/pts"
chroot "$dir" apt-get update -qq
chroot "$dir" env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends strace "$@" >/dev/null
echo "$dir: $(chroot "$dir" dpkg -l | grep -c ^ii) packages"
