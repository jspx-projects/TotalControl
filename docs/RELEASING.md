# Releasing Fedora RPMs

This note describes the release tags, the artifact names and the provenance
record for the Fedora convenience packages. It is a process document, not a
specification of the application.

## Tag naming

A Fedora artifact release is tagged for the Fedora release and the current
application series:

```text
fedora-44-0.1
```

The tag is annotated and points at the exact merge commit that was tested.
Do not reuse the old `fedora-44` tag; it points at the first packaging
release and its assets belong to an older source tree.

The release title is:

```text
Fedora 44 RPMs (TotalControl 0.1)
```

The RPM filenames keep normal RPM versioning:

```text
totalcontrol-0.1.0-1.fc44.x86_64.rpm
totalcontrol-kmod-0.1-1.fc44.noarch.rpm
```

`0.1` is the release series; `0.1.0` is the application's semantic version.
The kernel package uses its own `0.1` version.

## Release sequence

1. Merge the packaging or application change into `main`.
2. Confirm `main` is the tested commit and the worktree is clean.
3. Create and push an annotated tag on that commit:

   ```sh
   git switch main
   git pull --ff-only
   git tag -a fedora-44-0.1 -m "Fedora 44 RPMs, TotalControl 0.1"
   git push origin fedora-44-0.1
   ```

4. Create a GitHub release from the tag with the title above.
5. The `Fedora RPMs` workflow checks out the release tag, builds in a
   `fedora:44` container and attaches the artifacts.

For a local release build, use a clean checkout of the tag. Do not build a
release from a branch, a dirty worktree or a detached worktree whose `.git`
file points outside the build environment.

```sh
git clone https://github.com/jspx-projects/TotalControl
cd TotalControl
git checkout fedora-44-0.1
RELEASE_BUILD=1 ./packaging/fedora/build-rpms.sh
```

On Fedora 44, install the build dependencies listed at the top of
`packaging/fedora/build-rpms.sh`. A `fedora:44` container is also suitable.

## Provenance

Every build writes:

- `packaging/fedora/out/RELEASE-MANIFEST.txt`;
- `packaging/fedora/out/SHA256SUMS`.

The manifest records:

- release tag;
- full commit SHA and commit date;
- clean/dirty tree state;
- application and kernel package versions;
- Fedora release;
- build timestamp;
- source archive SHA-256;
- every RPM filename and SHA-256;
- the GitHub Actions run URL when the build runs in CI.

A release build (`RELEASE_BUILD=1`) fails unless `HEAD` has an exact tag and
the tracked worktree is clean. The RPMs also carry a `VCS:` field with the
commit hash. The application binary has its own build ID (`<git hash>-<UTC
stamp>`), available in About and through `--build-id`.

The GitHub release should contain:

- the two binary RPMs;
- `RELEASE-MANIFEST.txt`;
- `SHA256SUMS`.

The workflow uploads the same files as a workflow artifact for inspection.

## Checking a downloaded release

```sh
sha256sum -c SHA256SUMS
rpm -qpi totalcontrol-0.1.0-1.fc44.x86_64.rpm
rpm -qpi totalcontrol-kmod-0.1-1.fc44.noarch.rpm
```

Compare the `VCS:`/commit in the RPM metadata and the manifest with the
release tag. The installed GUI reports the shorter build ID in About, or:

```sh
totalcontrol --build-id
```

## Old releases

The pre-provenance `fedora-44` release remains as history. New Fedora 44
artifacts should use a versioned tag such as `fedora-44-0.1` rather than
replacing its assets.
