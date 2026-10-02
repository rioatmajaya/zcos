#!/usr/bin/env sh
# Build the virtio test disk: a 128 MiB MBR disk with two real filesystems, a
# FAT32 volume carrying HELLO.TXT and an ext2 volume carrying EXT2.TXT.
#
# Usage: tools/mkdisk.sh
#   Writes build/disk.img. Sector zero keeps the ZCDISK01 magic the raw probe
#   checks and gains two MBR partition entries: type 0x0C (FAT32 LBA) at LBA
#   2048 and type 0x83 (Linux, ext2) after it. The write and cache test markers
#   at sectors 8 and 16..21 sit in the gap before the first partition, so they
#   stay untouched.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

mkdir -p build

disk_size=$((128 * 1024 * 1024))
fat_lba=2048
fat_sectors=$((40 * 1024 * 1024 / 512))
ext2_lba=$((fat_lba + fat_sectors))
ext2_sectors=$((64 * 1024 * 1024 / 512))
fat=build/data.fat
ext2=build/data.ext2

# A FAT32 partition in its own file. One sector per cluster keeps the cluster
# math trivial and clears the >= 65525 cluster floor FAT32 requires.
rm -f "$fat"
python3 - "$fat" "$fat_sectors" <<'EOF'
import sys

path, sectors = sys.argv[1], int(sys.argv[2])
with open(path, "wb") as handle:
    handle.truncate(sectors * 512)
EOF
mkfs.vfat -F 32 -s 1 -n ZCDATA "$fat" >/dev/null

# One known file, forced to the 8.3 name the driver looks up.
printf 'ZC FAT32 OK\n' > build/hello.txt
mcopy -i "$fat" build/hello.txt ::HELLO.TXT

# An ext2 partition in its own file, populated at creation from a staging
# directory. 1024-byte blocks keep the parser's buffers small; pinning the
# orphan-file feature off keeps the on-disk layout stable across e2fsprogs
# versions, so the driver sees the same volume on every machine.
rm -f "$ext2"
rm -rf build/ext2root
mkdir -p build/ext2root
printf 'ZC EXT2 OK\n' > build/ext2root/EXT2.TXT
python3 - "$ext2" "$ext2_sectors" <<'EOF'
import sys

path, sectors = sys.argv[1], int(sys.argv[2])
with open(path, "wb") as handle:
    handle.truncate(sectors * 512)
EOF
mke2fs -t ext2 -b 1024 -I 256 -L ZCEXT2 -O ^orphan_file -F -q -d build/ext2root "$ext2"

# Assemble the disk: magic and capacity, two MBR entries, the boot signature,
# then both partitions at their LBAs.
python3 - "$fat" "$ext2" "$disk_size" "$fat_lba" "$fat_sectors" "$ext2_lba" "$ext2_sectors" <<'EOF'
import sys

fat_path, ext2_path = sys.argv[1], sys.argv[2]
disk_size, fat_lba, fat_sectors, ext2_lba, ext2_sectors = (
    int(value) for value in sys.argv[3:]
)

disk = bytearray(disk_size)
disk[0:8] = b"ZCDISK01"
disk[8:12] = (disk_size // 512).to_bytes(4, "little")


def entry(index, kind, lba, sectors):
    base = 446 + index * 16
    disk[base + 4] = kind
    disk[base + 8:base + 12] = lba.to_bytes(4, "little")
    disk[base + 12:base + 16] = sectors.to_bytes(4, "little")


entry(0, 0x0C, fat_lba, fat_sectors)  # FAT32 with LBA addressing
entry(1, 0x83, ext2_lba, ext2_sectors)  # Linux, the ext2 volume
disk[510:512] = b"\x55\xaa"

for path, lba in ((fat_path, fat_lba), (ext2_path, ext2_lba)):
    with open(path, "rb") as handle:
        data = handle.read()
    start = lba * 512
    disk[start:start + len(data)] = data

with open("build/disk.img", "wb") as handle:
    handle.write(bytes(disk))
print(
    "wrote build/disk.img (%d bytes, FAT32 at LBA %d, ext2 at LBA %d)"
    % (disk_size, fat_lba, ext2_lba)
)
EOF
