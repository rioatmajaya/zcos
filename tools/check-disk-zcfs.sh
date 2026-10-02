#!/usr/bin/env sh
# Verify the zcfs data partition with the host implementation, independently of
# the driver.
#
# The block domain parses the MBR, mounts the volume, reads the host-planted
# /probe, and creates /written; the shell then rewrites /probe through the
# kernel VFS. This check reads the same image with the independent Python
# format implementation in tools/zcfs.py and replays the log itself, so the
# driver is not the only thing validating the volume: the host wrote what the
# guest read, and the host reads back what the guest wrote. Run after
# tools/build-efi.sh and a boot.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
disk=build/disk.img

if [ ! -f "$disk" ]; then
    echo "error: $disk not found; run tools/build-efi.sh first" >&2
    exit 1
fi

python3 - "$disk" <<'EOF'
import struct
import sys

sys.path.insert(0, "tools")
import zcfs

image = open(sys.argv[1], "rb").read()
if image[510:512] != b"\x55\xaa":
    sys.exit("error: missing MBR signature at 510")

# The third partition entry must be the zcfs volume.
base = 446 + 2 * 16
kind = image[base + 4]
if kind != 0x7F:
    sys.exit("error: partition type %#x is not zcfs" % kind)
lba, sectors = struct.unpack_from("<II", image, base + 8)
if lba == 0 or sectors == 0:
    sys.exit("error: empty zcfs partition entry")
print("mbr: zcfs lba %d, %d sectors" % (lba, sectors), file=sys.stderr)

# Parse both superblock copies. select_superblock rejects a bad CRC, so a
# successful parse is itself the checksum proof.
a = image[lba * 512:lba * 512 + 512]
b = image[lba * 512 + 512:lba * 512 + 1024]
try:
    selected = zcfs.select_superblock(a, b)
except ValueError as error:
    sys.exit("error: superblock: %s" % error)
print("sb: head_seq %d, tail_seq %d, flags %#x"
      % (selected["head_seq"], selected["tail_seq"], selected["flags"]),
      file=sys.stderr)

# Replay the log exactly as the guest does, from the durable prefix only.
_, nodes = zcfs.replay(image, lba)

# The host planted /probe and the guest read it at boot, so a successful
# `blk: zcfs probe ok` already proves the host-to-guest direction. The shell
# then rewrites the same path through the VFS, so after a boot the durable
# content is the shell's pattern: replaying it proves the guest-to-host
# direction and that both implementations agree on the DATA record.
_, probe = zcfs.find(nodes, zcfs.ROOT_NODE, b"probe")
if probe is None:
    sys.exit("error: /probe missing from the replayed tree")
if bytes(probe["content"]) != zcfs.PERSIST_PATTERN:
    sys.exit("error: /probe content is %r, not %r"
             % (bytes(probe["content"]), zcfs.PERSIST_PATTERN))
print("file: /probe ok", file=sys.stderr)

# The guest created and wrote /written. Only the host can confirm this: the
# guest cannot see its own append as durable without dropping its cache.
_, written = zcfs.find(nodes, zcfs.ROOT_NODE, b"written")
if written is None:
    sys.exit("error: /written missing from the replayed tree")
expected = b"ZCGUEST1\n"
if bytes(written["content"]) != expected:
    sys.exit("error: /written content is %r, not %r"
             % (bytes(written["content"]), expected))
print("file: /written ok", file=sys.stderr)
EOF

echo "disk zcfs verified: /probe and /written replayed by the host"
