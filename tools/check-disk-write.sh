#!/usr/bin/env sh
# Verify the block driver's writes reached the test disk.
#
# The driver writes two markers and flushes them (see
# libs/zc-storage/src/virtio.rs and libs/zc-storage/src/block_cache.rs).
# Reading the host image after QEMU exits proves the writes left the guest:
# the raw path and the write-back cache both persisted, not just their DMA
# buffers.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
disk="$root/build/disk.img"

if [ ! -f "$disk" ]; then
    echo "error: $disk not found; run tools/build-efi.sh first" >&2
    exit 1
fi

python3 - "$disk" <<'EOF'
import sys

sector_size = 512
# (sector, magic, how it was written)
markers = [
    (8, b"ZCWRITE1", "raw write"),          # virtio::TEST_SECTOR / WRITE_MAGIC
    (16, b"ZCCACHE1", "cache eviction"),    # block_cache::CACHE_TEST_SECTOR
    (21, b"ZCCACHE1", "cache flush"),       # CACHE_TEST_SECTOR + 5
]

data = open(sys.argv[1], "rb").read()
failed = False
for sector, magic, label in markers:
    offset = sector * sector_size
    found = data[offset:offset + len(magic)]
    if found != magic:
        print(
            f"error: {label}: expected {magic!r} at offset {offset}, found {found!r}",
            file=sys.stderr,
        )
        failed = True
    else:
        print(f"disk write verified: sector {sector} ({label})")
if failed:
    sys.exit(1)
EOF
