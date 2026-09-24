# Rebuilding Alpine Linux

Alpine support is experimental. rebuilderd ships the sync
(`rebuildctl pkgs sync alpine`), the backend script
(`worker/rebuilder-alpine.sh`) and a stabilizer for .apk files
(`worker/stabilizer-alpine.py`). It doesn't ship the two other pieces a
rebuilder needs:

- an Alpine archive, an HTTP endpoint that serves the routes listed below
- a rebuild command that turns a build input into .apk files

## How Alpine maps onto rebuilderd

| rebuilderd     | Alpine                                              |
|----------------|-----------------------------------------------------|
| release        | branch as spelled on the mirror: `v3.24`, `edge`    |
| component      | repo: `main`, `community`                           |
| architecture   | the arch directory: `x86_64`, `aarch64`             |
| source package | origin (`o:` in `APKINDEX`), the aport directory    |
| version        | `<pkgver>-r<pkgrel>`                                |
| binary package | one `APKINDEX` record                               |
| build input    | `apkbuildinfo.json` from the archive                |

The same name and version is usually published with different bytes on
different branches, because every branch has its own builders and toolchain.
The release is part of every URL the sync emits, so rebuilderd never copies a
result from one branch to another.

Don't use the `latest-stable` alias as a release, it moves with every Alpine
release and the release is part of a package's identity in rebuilderd.

## The archive

The Alpine mirrors only keep the newest version of each package, so a package
can disappear before a worker gets to it. The sync reads from an archive
instead. `source` in the sync profile is its base URL, and it needs to serve:

| Route | Response |
|---|---|
| `GET <source>/latest/<branch>/<repo>/<arch>/APKINDEX.tar.gz` | the current index, as published by Alpine |
| `GET <source>/archive/<branch>/<repo>/<arch>/<name>-<version>.apk?c=<C:>` | the package whose index record has this `C:` checksum, also after it was removed from the mirror |
| `GET <source>/buildinfo/<branch>/<repo>/<arch>/<origin>/<version>/apkbuildinfo.json` | the build input of that origin |

rebuilderd doesn't parse the build input, it's passed to the rebuild command
as is. Its format is a contract between the archive and the rebuild command,
and it should contain everything the rebuild needs: the aports commit, the
build environment and the URLs of the packages to install.

The checksum is in the query string because the worker names its download
after the last path segment. The same filename can be published twice with
different bytes, for example after a move from `testing` to `community`.

## Sync

```toml
[profile."alpine-v3.24"]
distro = "alpine"
components = ["main", "community"]
architectures = ["x86_64", "aarch64"]
releases = ["v3.24"]
source = "https://archive.example.com/alpine"
```

Use one profile per branch. A component whose index can't be fetched is
skipped with a warning.

## Worker

```toml
[backend."alpine"]
path = "/usr/libexec/rebuilderd/rebuilder-alpine.sh"
stabilizer = "/usr/libexec/rebuilderd/stabilizer-alpine.py"
```

`rebuilder-alpine.sh` runs the command in the `REBUILDERD_ALPINE_REBUILDER`
environment variable of the worker as

```
$REBUILDERD_ALPINE_REBUILDER --output <dir> -- <apkbuildinfo.json>
```

The command has to write every .apk of the build to `<dir>` under its
published filename, `<name>-<pkgver>-r<pkgrel>.apk`. Whether the worker can
run in a container, and with which privileges, depends on this command.
`worker/Dockerfile.alpine` contains the backend script, the stabilizer and
python3, but no rebuild command.

The worker advertises its native architecture by default, which uses the
same names as Alpine for `x86_64` and `aarch64`. Workers for `x86`, `armv7`,
`armhf` or `ppc64le` need `supported_architectures` set.

## Comparison

An .apk starts with a signature segment made with Alpine's private key, so a
rebuilt package is never bit-for-bit identical to the published one. The
stabilizer drops the leading signature segment and keeps the control and data
segments byte for byte. These are exactly the bytes the signature covers, so
a match after stabilizing means the rebuild produced the signed content. Such
a package is reported as GOOD with the stabilizer recorded, see
`rebuilderd-worker.conf(5)`.

Packages with signatures embedded in their files, like `apk-tools-static`,
still differ after stabilizing and are reported as BAD.
