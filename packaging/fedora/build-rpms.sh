#!/bin/sh
# Build the Fedora release RPMs locally.
#
# Requirements on Fedora:
#   sudo dnf install rpm-build rpmdevtools cargo rust gcc gcc-c++ make \
#       pkgconf-pkg-config alsa-lib-devel mesa-libGL-devel libX11-devel \
#       libxkbcommon-devel wayland-devel systemd-devel fontconfig-devel \
#       freetype-devel openssl-devel desktop-file-utils git tar gzip
#
# The build uses the checked-out commit as its source.  Outputs are written
# to packaging/fedora/out/ (git-ignored).  The output includes a provenance
# manifest and SHA256SUMS for the release assets.
#
# Set RELEASE_BUILD=1 for a release build.  That requires a clean tree and an
# exact annotated tag at HEAD.  Without it, local development builds are
# allowed and the manifest records the dirty/tag state.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' \
    "$ROOT/ui/totalcontrol/totalcontrol-gui/Cargo.toml" | head -n 1)
[ -n "$VERSION" ] || {
    echo "could not read the application version" >&2
    exit 1
}
KMOD_VERSION=$(sed -n 's/^%global kmod_version[[:space:]]*//p' \
    "$ROOT/packaging/fedora/totalcontrol-kmod.spec" | head -n 1)
[ -n "$KMOD_VERSION" ] || {
    echo "could not read the kernel package version" >&2
    exit 1
}

COMMIT=$(git -C "$ROOT" rev-parse HEAD)
COMMIT_SHORT=$(printf '%s' "$COMMIT" | cut -c1-12)
COMMIT_DATE=$(git -C "$ROOT" show -s --format=%cI HEAD)
TAG=$(git -C "$ROOT" describe --tags --exact-match HEAD 2>/dev/null || true)
DIRTY=$(git -C "$ROOT" status --porcelain --untracked-files=no || true)
RELEASE_BUILD=${RELEASE_BUILD:-0}

if [ "$RELEASE_BUILD" = 1 ]; then
    [ -n "$TAG" ] || {
        echo "release build requires an exact tag at HEAD" >&2
        exit 1
    }
    [ -z "$DIRTY" ] || {
        echo "release build requires a clean worktree" >&2
        printf '%s\n' "$DIRTY" >&2
        exit 1
    }
fi

FEDORA_RELEASE=$(rpm -E '%{fedora}' 2>/dev/null || true)
[ -n "$FEDORA_RELEASE" ] || FEDORA_RELEASE=unknown

TOP=${RPM_TOPDIR:-"$ROOT/packaging/fedora/.rpmbuild"}
OUT="$ROOT/packaging/fedora/out"
SPECS="$ROOT/packaging/fedora"
SOURCE="TotalControl-$VERSION.tar.gz"

rm -rf "$TOP" "$OUT"
mkdir -p "$TOP"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS} "$OUT"

git -C "$ROOT" archive --format=tar.gz \
    --prefix="TotalControl-$VERSION/" HEAD \
    > "$TOP/SOURCES/$SOURCE"
SOURCE_SHA256=$(sha256sum "$TOP/SOURCES/$SOURCE" | cut -d' ' -f1)

cp "$SPECS/totalcontrol.spec" "$SPECS/totalcontrol-kmod.spec" "$TOP/SPECS/"

export SOURCE_DATE_EPOCH=$(git -C "$ROOT" show -s --format=%ct HEAD)
rpmbuild --define "_topdir $TOP" --define "app_version $VERSION" \
    --define "commit $COMMIT" \
    -ba "$TOP/SPECS/totalcontrol.spec"
rpmbuild --define "_topdir $TOP" --define "app_version $VERSION" \
    --define "commit $COMMIT" \
    -ba "$TOP/SPECS/totalcontrol-kmod.spec"

find "$TOP/RPMS" -type f -name '*.rpm' -exec cp -p {} "$OUT/" \;

BUILT_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)
TREE_STATE=clean
if [ -n "$DIRTY" ]; then
    TREE_STATE=dirty
fi
WORKFLOW_RUN=
if [ -n "${GITHUB_RUN_ID:-}" ]; then
    WORKFLOW_RUN="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-jspx-projects/TotalControl}/actions/runs/$GITHUB_RUN_ID"
fi

{
    echo "TotalControl Fedora RPM build"
    echo "release_tag: ${TAG:-(none)}"
    echo "commit: $COMMIT"
    echo "commit_short: $COMMIT_SHORT"
    echo "commit_date: $COMMIT_DATE"
    echo "tree: $TREE_STATE"
    echo "app_version: $VERSION"
    echo "kmod_version: $KMOD_VERSION"
    echo "fedora_release: $FEDORA_RELEASE"
    echo "built_utc: $BUILT_UTC"
    echo "release_build: $RELEASE_BUILD"
    echo "source_archive: $SOURCE"
    echo "source_sha256: $SOURCE_SHA256"
    if [ -n "$WORKFLOW_RUN" ]; then
        echo "workflow_run: $WORKFLOW_RUN"
    fi
    echo "artifacts:"
    for rpm in "$OUT"/*.rpm; do
        [ -f "$rpm" ] || continue
        sha256sum "$rpm" | awk '{print "  " $1 "  " $2}'
    done
} > "$OUT/RELEASE-MANIFEST.txt"

(cd "$OUT" && sha256sum ./*.rpm > SHA256SUMS)

echo "RPMs written to $OUT:"
find "$OUT" -maxdepth 1 -type f -name '*.rpm' -printf '  %f\n' | sort
echo "Provenance: $OUT/RELEASE-MANIFEST.txt"
echo "Checksums:  $OUT/SHA256SUMS"
