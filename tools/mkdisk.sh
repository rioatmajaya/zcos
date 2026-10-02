#!/usr/bin/env sh
# Build the virtio test disk: a 64 MiB MBR disk whose first partition is a
# real FAT32 volume carrying HELLO.TXT.
#
# Usage: tools/mkdisk.sh
#   Writes build/disk.img. Sector zero keeps the ZCDISK01 magic the raw probe
#   checks and gains one MBR partition entry (type 0x0C, FAT32 LBA) pointing
#   at the FAT32 partition the block domain mounts read-only. The write and
#   cache test markers at sectors 8 and 16..21 sit in the gap before the
#   partition, so they stay untouched.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

mkdir -p build

disk_size=$((64 * 1024 * 1024))
part_lba=2048
part_sectors=$((disk_size / 512 - part_lba))
part=build/data.fat

# A FAT32 partition in its own file. One sector per cluster keeps the cluster
# math trivial and clears the >= 65525 cluster floor FAT32 requires.
rm -f "$part"
python3 - "$part" "$part_sectors" <<'EOF'
import sys

path, sectors = sys.argv[1], int(sys.argv[2])
with open(path, "wb") as handle:
    handle.truncate(sectors * 512)
EOF
mkfs.vfat -F 32 -s 1 -n ZCDATA "$part" >/dev/null

# One known file, forced to the 8.3 name the driver looks up.
printf 'ZC FAT32 OK\n' > build/hello.txt
mcopy -i "$part" build/hello.txt ::HELLO.TXT

# Assemble the disk: magic and capacity, one MBR entry, the boot signature,
# then the partition bytes at its LBA.
python3 - "$part" "$disk_size" "$part_lba" "$part_sectors" <<'EOF'
import sys

part_path, disk_size, part_lba, part_sectors = (
    sys.argv[1],
    int(sys.argv[2]),
    int(sys.argv[3]),
    int(sys.argv[4]),
)
disk = bytearray(disk_size)
disk[0:8] = b"ZCDISK01"
disk[8:12] = (disk_size // 512).to_bytes(4, "little")
entry = 446
disk[entry + 4] = 0x0C  # FAT32 with LBA addressing
disk[entry + 8:entry + 12] = part_lba.to_bytes(4, "little")
disk[entry + 12:entry + 16] = part_sectors.to_bytes(4, "little")
disk[510:512] = b"\x55\xaa"
with open(part_path, "rb") as handle:
    data = handle.read()
start = part_lba * 512
disk[start:start + len(data)] = data
with open("build/disk.img", "wb") as handle:
    handle.write(bytes(disk))
print("wrote build/disk.img (%d bytes, FAT32 at LBA %d)" % (disk_size, part_lba))
EOF
