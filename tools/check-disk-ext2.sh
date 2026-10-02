#!/usr/bin/env sh
# Verify the ext2 data partition with e2fsprogs, independently of the driver.
#
# The block domain parses the MBR and mounts the volume itself; this check
# reads the same image with an external tool so the driver is not the only
# thing validating it. Run after tools/build-efi.sh.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
disk=build/disk.img

if [ ! -f "$disk" ]; then
    echo "error: $disk not found; run tools/build-efi.sh first" >&2
    exit 1
fi

# Validate the second MBR entry and the superblock, then extract the partition
# so debugfs can read it without needing an offset.
python3 - "$disk" build/ext2.part <<'EOF'
import struct
import sys

disk_path, out_path = sys.argv[1], sys.argv[2]
data = open(disk_path, "rb").read()
if data[510:512] != b"\x55\xaa":
    sys.exit("error: missing MBR signature at 510")

base = 446 + 16  # the second partition entry
kind = data[base + 4]
if kind != 0x83:
    sys.exit("error: partition type %#x is not ext2" % kind)
lba, sectors = struct.unpack_from("<II", data, base + 8)
if lba == 0 or sectors == 0:
    sys.exit("error: empty ext2 partition entry")

start = lba * 512
superblock = data[start + 1024:start + 1536]
if len(superblock) != 512:
    sys.exit("error: ext2 partition is truncated")
magic = struct.unpack_from("<H", superblock, 56)[0]
if magic != 0xEF53:
    sys.exit("error: no ext2 magic (found %#06x)" % magic)

open(out_path, "wb").write(data[start:start + sectors * 512])
print("mbr: ext2 lba %d, %d sectors" % (lba, sectors), file=sys.stderr)
print("sb: ext2 magic ok", file=sys.stderr)
EOF

content=$(debugfs -R "cat /EXT2.TXT" build/ext2.part 2>/dev/null)
[ "$content" = "ZC EXT2 OK" ] \
    || { echo "error: unexpected EXT2.TXT content: [$content]" >&2; exit 1; }

echo "disk ext2 verified: EXT2.TXT readable via debugfs"
