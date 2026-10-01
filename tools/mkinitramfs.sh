#!/usr/bin/env sh
# Pack initramfs/ into a deterministic cpio newc archive for the boot image.
#
# Usage: tools/mkinitramfs.sh
#   Reads initramfs/ plus any extra files passed as src:arcname pairs,
#   writes build/initramfs.cpio. Timestamps, owners, and entry order are
#   fixed so repeated builds are byte-identical.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

mkdir -p build
python3 - "$root/initramfs" "$root/build/initramfs.cpio" "$@" <<'EOF'
import os
import sys

source, output, extras = sys.argv[1], sys.argv[2], sys.argv[3:]

entries = []
for dirpath, _dirnames, filenames in os.walk(source):
    for name in filenames:
        full = os.path.join(dirpath, name)
        arc = os.path.relpath(full, source).replace(os.sep, "/")
        with open(full, "rb") as handle:
            entries.append((arc, handle.read()))
for extra in extras:
    src, arc = extra.split(":", 1)
    with open(src, "rb") as handle:
        entries.append((arc, handle.read()))
entries.sort()
entries.append(("TRAILER!!!", b""))

def field(value):
    return ("%08X" % value).encode("ascii")

blob = bytearray()
for arc, data in entries:
    name = arc.encode("utf-8") + b"\0"
    header = (
        b"070701"
        + field(1)          # ino
        + field(0o100644)   # mode
        + field(0)          # uid
        + field(0)          # gid
        + field(1)          # nlink
        + field(0)          # mtime (fixed for reproducibility)
        + field(len(data))  # filesize
        + field(0)          # devmajor
        + field(0)          # devminor
        + field(0)          # rdevmajor
        + field(0)          # rdevminor
        + field(len(name))  # namesize
        + field(0)          # check
    )
    assert len(header) == 110
    blob += header + name
    blob += b"\0" * (-len(blob) % 4)
    blob += data
    blob += b"\0" * (-len(blob) % 4)

with open(output, "wb") as handle:
    handle.write(bytes(blob))
print("wrote %s (%d bytes, %d entries)" % (output, len(blob), len(entries)))
EOF
