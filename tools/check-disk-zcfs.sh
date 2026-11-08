#!/usr/bin/env sh
# Verify the zcfs data partition with the host implementation, independently of
# the driver.
#
# The block domain parses the MBR, mounts the volume, reads the host-planted
# /probe, and creates /written; the shell then rewrites /probe through the
# kernel VFS and finally removes it again. Since the persistent-volume
# increments (ADR 0066/0068/0069/0074/0075) the boot script also removes,
# renames, re-owns, creates, and rmdirs names on the volume, so this check
# replays the whole log with the independent Python implementation in
# tools/zcfs.py — every record kind the guest writes — and asserts the tree
# those records must leave behind, so the driver is not the only thing
# validating the volume: the host wrote what the guest read, and the host
# reads back what the guest wrote. Run after tools/build-efi.sh and a boot.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
disk=build/disk.img
planted=build/data.zcfs

if [ ! -f "$disk" ]; then
    echo "error: $disk not found; run tools/build-efi.sh first" >&2
    exit 1
fi
if [ ! -f "$planted" ]; then
    echo "error: $planted not found; run tools/build-efi.sh first" >&2
    exit 1
fi

python3 - "$disk" "$planted" <<'EOF'
import struct
import sys

sys.path.insert(0, "tools")
import zcfs

disk_path, planted_path = sys.argv[1], sys.argv[2]
image = open(disk_path, "rb").read()
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
# Every record kind the boot writes must replay cleanly: a record that parses
# but contradicts the tree raises, which is the guest's failed mount.
try:
    _, nodes = zcfs.replay(image, lba)
except ValueError as error:
    sys.exit("error: the guest's log does not replay: %s" % error)

# Walk the raw records below the claimed head and collect the kinds the boot
# wrote. The script on this image removes, renames, re-owns, creates, and
# rmdirs names on the volume, so all six kinds must appear: a future script
# edit that drops one of those operations fails here instead of narrowing
# the proof silently.
records = []
for seq in range(selected["tail_seq"] + 1, selected["head_seq"] + 1):
    relative = selected["log_start"] + ((seq - 1) % selected["log_sectors"])
    sector = image[(lba + relative) * 512:(lba + relative) * 512 + 512]
    try:
        record = zcfs.parse_record(sector)
    except ValueError:
        break
    if record["seq"] != seq:
        break
    records.append(record)
kinds = {record["kind"] for record in records}
expected_kinds = {zcfs.KIND_CREATE, zcfs.KIND_DATA, zcfs.KIND_UNLINK,
                  zcfs.KIND_RMDIR, zcfs.KIND_RENAME, zcfs.KIND_OWNER}
if kinds != expected_kinds:
    sys.exit("error: the boot wrote kinds %s, expected all of %s"
             % (sorted(kinds), sorted(expected_kinds)))
print("log: all six record kinds over %d records" % len(records), file=sys.stderr)

# The shell rewrote /probe through the VFS before removing it, so the log
# must still carry the persist pattern in a DATA record even though the
# final tree no longer holds the name: the guest-to-host direction is proven
# by the record, the removal by the tree checks below.
persisted = any(
    record["kind"] == zcfs.KIND_DATA
    and zcfs.parse_data(record["payload"])[1] == zcfs.PERSIST_PATTERN
    for record in records
)
if not persisted:
    sys.exit("error: no DATA record carries %r" % zcfs.PERSIST_PATTERN)
print("file: the shell's /probe rewrite reached the disk", file=sys.stderr)

# The boot ends with every transient name gone: both /probe nodes were
# unlinked, /empty was renamed to /moved and then unlinked, and /atlas was
# created and rmdir'd. A replay that ignored any of those records would
# leave the name behind and fail here.
for gone in (b"probe", b"empty", b"moved", b"atlas"):
    _, leftover = zcfs.find(nodes, zcfs.ROOT_NODE, gone)
    if leftover is not None:
        sys.exit("error: /%s survived the boot's removal records"
                 % gone.decode())
names = sorted(node["name"].decode("utf-8", "replace") for node in nodes.values())
if names != ["/", "written"]:
    sys.exit("error: the replayed tree holds %r, expected exactly / and /written"
             % names)
print("tree: exactly / and /written", file=sys.stderr)

# The guest created and wrote /written. Only the host can confirm this: the
# guest cannot see its own append as durable without dropping its cache.
_, written = zcfs.find(nodes, zcfs.ROOT_NODE, b"written")
if written is None:
    sys.exit("error: /written missing from the replayed tree")
expected = b"ZCGUEST1\n"
if bytes(written["content"]) != expected:
    sys.exit("error: /written content is %r, not %r"
             % (bytes(written["content"]), expected))
# /written was created through the zcfs API, not the VFS, so no OWNER record
# names it: its owner must still be the replay default, proving the OWNER
# records the boot did write landed on the nodes they named and nowhere else.
if (written["owner_uid"], written["owner_gid"]) != (0, 0):
    sys.exit("error: /written owner is %d:%d, expected the 0:0 default"
             % (written["owner_uid"], written["owner_gid"]))
print("file: /written ok", file=sys.stderr)

# Prove the rename moved the name, not merely that the final tree lacks both
# spellings: replay only up to the RENAME record, where /moved must exist
# with the content /empty was written with, /empty must be gone, and the
# persisted owner must have survived the move.
rename_seq = min(record["seq"] for record in records
                 if record["kind"] == zcfs.KIND_RENAME)
_, prefix_nodes = zcfs.replay(image, lba, limit=rename_seq)
_, moved = zcfs.find(prefix_nodes, zcfs.ROOT_NODE, b"moved")
_, empty = zcfs.find(prefix_nodes, zcfs.ROOT_NODE, b"empty")
if moved is None or empty is not None or bytes(moved["content"]) != b"X":
    sys.exit("error: replaying to the rename did not leave /moved holding "
             "/empty's content")
if (moved["owner_uid"], moved["owner_gid"]) != (1, 1):
    sys.exit("error: /moved owner is %d:%d, expected the persisted 1:1"
             % (moved["owner_uid"], moved["owner_gid"]))
print("rename: /empty -> /moved kept content and owner", file=sys.stderr)

# The script unmounts /data before powering off, so the volume must be
# marked clean: the clean-unmount path ran over the real disk.
if not selected["flags"] & zcfs.FLAG_CLEAN:
    sys.exit("error: the volume is not marked clean after the boot's unmount")

# The formatter plants a volume whose superblock over-claims a torn record, so
# the guest had to recover before it could serve anything. Check the image it
# started from: replay must clamp the head back by exactly the over-claim and
# still find /probe, which is what proves recovery is what let the boot work.
planted = open(planted_path, "rb").read()
sector = zcfs.SECTOR
claimed = zcfs.select_superblock(planted[0:sector],
                                 planted[sector:2 * sector])
recovered_sb, recovered_nodes = zcfs.replay(planted, 0)
if claimed["head_seq"] != recovered_sb["head_seq"] + 1:
    sys.exit("error: formatter did not plant a one-record over-claim "
             "(claimed %d, replayed %d)"
             % (claimed["head_seq"], recovered_sb["head_seq"]))
_, recovered_probe = zcfs.find(recovered_nodes, zcfs.ROOT_NODE, b"probe")
if recovered_probe is None or bytes(recovered_probe["content"]) != zcfs.HOST_PATTERN:
    sys.exit("error: recovery lost /probe")
print("recovery: head %d -> %d ok"
      % (claimed["head_seq"], recovered_sb["head_seq"]), file=sys.stderr)
EOF

echo "disk zcfs verified: every record kind replayed; tree, rename, owner, and recovery proven"
