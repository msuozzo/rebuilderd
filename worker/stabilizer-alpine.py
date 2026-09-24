#!/usr/bin/env python3
"""Stabilizer for Alpine Linux .apk packages.

Usage: stabilizer-alpine.py <input.apk> <output.apk>

An .apk file is three concatenated gzip streams, each containing a segment of
a tar archive:

1. the signature: .SIGN.RSA.<key>.rsa.pub, an RSA signature over the sha1 of
   the (compressed) control segment
2. the control segment: .PKGINFO and friends, .PKGINFO contains the datahash,
   the sha256 of the (compressed) data segment
3. the data segment: the files installed by the package

abuild makes the control and data segments reproducible, but a rebuilder can't
reproduce the signature because it doesn't have the private key of upstream.

This drops the leading signature segment(s) and writes the remaining gzip
streams to the output file unmodified, without recompressing them. Nothing
that is covered by the signature is lost: the output is exactly the bytes
upstream's key signed, the control segment directly and the data segment
through the datahash in .PKGINFO. If the input doesn't start with a signature
segment the output is identical to the input.
"""

import io
import sys
import tarfile
import zlib


def signature_names(member):
    """Returns the tar entries of a gzip member if it's a signature segment."""
    with tarfile.open(fileobj=io.BytesIO(member), mode="r:") as tar:
        names = tar.getnames()
    if names and all(name.startswith(".SIGN.") for name in names):
        return names
    return None


def strip_signatures(data):
    """Returns the data following the leading signature segment(s)."""
    offset = 0
    while offset < len(data):
        gz = zlib.decompressobj(16 + zlib.MAX_WBITS)
        member = gz.decompress(data[offset:])
        if not gz.eof:
            raise ValueError(f"Truncated gzip stream at offset {offset}")

        names = signature_names(member)
        if not names:
            break
        for name in names:
            print(f"Removing signature {name}")

        offset = len(data) - len(gz.unused_data)
    return data[offset:]


def main():
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <input.apk> <output.apk>", file=sys.stderr)
        return 2
    src, dst = sys.argv[1:]

    with open(src, "rb") as f:
        data = f.read()
    try:
        data = strip_signatures(data)
    except (ValueError, zlib.error, tarfile.TarError) as err:
        print(f"Failed to parse {src}: {err}", file=sys.stderr)
        return 1
    with open(dst, "wb") as f:
        f.write(data)
    return 0


if __name__ == "__main__":
    sys.exit(main())
