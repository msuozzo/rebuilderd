#!/bin/sh
# rebuilderd backend for Alpine Linux, see docs/alpine.md.
#
# $1 is the build input, an apkbuildinfo.json downloaded from the Alpine
# archive. rebuilderd doesn't interpret it, it's passed to the command in
# REBUILDERD_ALPINE_REBUILDER as
#
#   $REBUILDERD_ALPINE_REBUILDER --output "$REBUILDERD_OUTDIR" -- <apkbuildinfo.json>
#
# The command has to write every .apk of the build to the output directory
# under its published filename, <name>-<pkgver>-r<pkgrel>.apk. rebuilderd
# compares them bit for bit and then through stabilizer-alpine.py, which drops
# the leading signature segment.
set -e

if [ -z "${REBUILDERD_ALPINE_REBUILDER:-}" ]; then
	echo "REBUILDERD_ALPINE_REBUILDER is not set, see docs/alpine.md" >&2
	exit 1
fi

cat <<-EOF
===============================================================================

About this build: this rebuild reproduces a package published by Alpine Linux
from the aports commit, build environment and package set recorded in its
apkbuildinfo.json.

===============================================================================

EOF
echo "Build input: $1"
echo "rebuilderd-worker node: $(hostname)"
echo
echo "+------------------------------------------------------------------------------+"
echo "| Calling the rebuild command                  $(date -u -R) |"
echo "+------------------------------------------------------------------------------+"
echo
set -x
# the command may include arguments, so it's split on purpose
# shellcheck disable=SC2086
exec ${REBUILDERD_ALPINE_REBUILDER} --output "${REBUILDERD_OUTDIR}" -- "${1}"
