#!/usr/bin/env bash
# PulseDeck installer for Arch Linux and derivatives
# (CachyOS, EndeavourOS, Manjaro, Garuda, Artix, ...).
#
# Builds a native pacman package with makepkg and installs it, so PulseDeck
# can later be removed cleanly with `sudo pacman -R pulsedeck`.
#
# Usage:
#   packaging/arch/install.sh              build from this checkout (committed HEAD)
#   packaging/arch/install.sh --worktree   build from this checkout incl. uncommitted changes
#   packaging/arch/install.sh --release    build the tagged release matching Cargo.toml from GitHub
#   packaging/arch/install.sh --uninstall  remove the installed package
#
# Extra arguments after `--` are passed to makepkg (e.g. `-- --noconfirm`).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
PKGNAME=pulsedeck

die() { printf '\e[1;31merror:\e[0m %s\n' "$*" >&2; exit 1; }
info() { printf '\e[1;32m==>\e[0m %s\n' "$*"; }

[[ $EUID -ne 0 ]] || die "do not run as root; makepkg will call sudo when needed"
command -v pacman >/dev/null || die "pacman not found: this installer is for Arch-based distributions"
command -v makepkg >/dev/null || die "makepkg not found: install it with 'sudo pacman -S --needed base-devel'"

mode=head
makepkg_args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --worktree) mode=worktree ;;
    --release) mode=release ;;
    --uninstall) exec sudo pacman -R "$PKGNAME" ;;
    -h|--help) sed -n '2,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    --) shift; makepkg_args=("$@"); break ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
  shift
done

pkgver="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$REPO_DIR/Cargo.toml" | head -n1)"
[[ -n $pkgver ]] || die "could not read version from Cargo.toml"

pacman -Q base-devel >/dev/null 2>&1 || {
  info "Installing base-devel (needed by makepkg)"
  sudo pacman -S --needed base-devel
}

builddir="$(mktemp -d -t pulsedeck-build.XXXXXX)"
trap 'rm -rf "$builddir"' EXIT
sed "s/^pkgver=.*/pkgver=$pkgver/" "$SCRIPT_DIR/PKGBUILD" > "$builddir/PKGBUILD"

tarball="$builddir/$PKGNAME-$pkgver.tar.gz"
case "$mode" in
  head)
    info "Packaging committed HEAD of $REPO_DIR as $PKGNAME $pkgver"
    git -C "$REPO_DIR" archive --format=tar.gz --prefix="$PKGNAME-$pkgver/" -o "$tarball" HEAD
    ;;
  worktree)
    info "Packaging working tree of $REPO_DIR as $PKGNAME $pkgver"
    tar -C "$REPO_DIR" --exclude=./target --exclude=./.git \
        --transform "s,^\.,$PKGNAME-$pkgver," -czf "$tarball" .
    ;;
  release)
    info "Using GitHub release v$pkgver"
    ;;
esac

info "Building and installing with makepkg"
(cd "$builddir" && makepkg --syncdeps --install --clean "${makepkg_args[@]}")

info "Done. Launch 'pulsedeck' (GUI) or 'pulsedeck --tui' (terminal)."
