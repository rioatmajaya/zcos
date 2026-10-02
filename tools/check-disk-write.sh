#!/usr/bin/env sh
# Verify the block driver's write reached the test disk.
#
# The driver writes WRITE_MAGIC at TEST_SECTOR (see
# kernel/zc-kernel/src/virtio.rs) and flushes it. Reading the host image after
# QEMU exits proves the write left the guest, not just its DMA buffer.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
disk="$root/build/disk.img"

if [ ! -f "$disk" ]; then
    echo "error: $disk not found; run tools/build-efi.sh first" >&2
    exit 1
fi

python3 - "$disk" <<'EOF'
import sys

magic = b"ZCWRITE1"   # virtio::WRITE_MAGIC
sector = 8            # virtio::TEST_SECTOR
sector_size = 512

data = open(sys.argv[1], "rb").read()
offset = sector * sector_size
found = data[offset:offset + len(magic)]
if found != magic:
    print(f"error: expected {magic!r} at offset {offset}, found {found!r}", file=sys.stderr)
    sys.exit(1)
print(f"disk write verified at sector {sector}")
EOF
