#!/usr/bin/env sh
# Verify the FAT32 data partition with mtools, independently of the driver.
#
# The block domain parses the MBR and mounts the volume itself; this check
# reads the same image with an external tool so the driver is not the only
# thing validating it. Run after tools/build-efi.sh.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
disk="$root/build/disk.img"

if [ ! -f "$disk" ]; then
    echo "error: $disk not found; run tools/build-efi.sh first" >&2
    exit 1
fi

# Validate the MBR entry and the FAT32 VBR, then print the partition's byte
# offset on stdout so mtools can address it.
base=$(python3 - "$disk" <<'EOF'
import sys

data = open(sys.argv[1], "rb").read()
if data[510:512] != b"\x55\xaa":
    sys.exit("error: missing MBR signature at 510")

entry = 446
kind = data[entry + 4]
if kind not in (0x0B, 0x0C):
    sys.exit("error: partition type %#x is not FAT32" % kind)
lba = int.from_bytes(data[entry + 8:entry + 12], "little")
sectors = int.from_bytes(data[entry + 12:entry + 16], "little")
if lba == 0 or sectors == 0:
    sys.exit("error: empty FAT32 partition entry")

vbr = data[lba * 512:lba * 512 + 512]
if len(vbr) != 512 or vbr[510:512] != b"\x55\xaa":
    sys.exit("error: missing FAT32 VBR signature")
if vbr[82:90] != b"FAT32   ":
    sys.exit("error: unexpected fs type %r" % vbr[82:90])

print("mbr: fat32 lba %d, %d sectors" % (lba, sectors), file=sys.stderr)
print("vbr: FAT32 signature ok", file=sys.stderr)
print(lba * 512)
EOF
)

# mdir prints the short name as "HELLO    TXT", not "HELLO.TXT".
mdir -i "$disk@@$base" :: | grep -qiE 'HELLO +TXT' \
    || { echo "error: HELLO.TXT not listed" >&2; exit 1; }

content=$(mtype -i "$disk@@$base" ::HELLO.TXT)
[ "$content" = "ZC FAT32 OK" ] \
    || { echo "error: unexpected content: [$content]" >&2; exit 1; }

echo "disk fs verified: HELLO.TXT readable via mtools at byte offset $base"
