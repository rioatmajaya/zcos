#!/usr/bin/env sh
# Build a tiny virtio test disk with a known magic in sector zero.
#
# Usage: tools/mkdisk.sh
#   Writes build/disk.img (1 MiB, 2048 sectors). The kernel reads sector
#   zero through virtio-blk and checks the magic plus the capacity.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

mkdir -p build
python3 - <<'EOF'
size = 1024 * 1024
disk = bytearray(size)
disk[0:8] = b"ZCDISK01"
disk[8:12] = (2048).to_bytes(4, "little")
with open("build/disk.img", "wb") as handle:
    handle.write(bytes(disk))
print("wrote build/disk.img (%d bytes)" % size)
EOF
