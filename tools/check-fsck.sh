#!/usr/bin/env sh
# Verify the zcfs fsck recovery tool, independently of the driver.
#
# F7h's proof: a power-loss volume (a superblock that over-claims a torn tail,
# the exact state the formatter plants) must repair to a clean, consistent
# volume, and a second fsck must report it clean (idempotent). A cleanly
# formatted volume must already report clean. This mirrors the guest's
# `Volume::repair` through the independent Python implementation in
# tools/zcfs.py, so the host and the Rust side agree on what "clean" means.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

workdir=build/fsckwork
mkdir -p "$workdir"
clean_vol=$workdir/clean.img
dirty_vol=$workdir/dirty.img
repaired_vol=$workdir/repaired.img
sectors=64

echo "formatter: formatting clean and dirty power-loss volumes" >&2
python3 tools/zcfs.py format --clean "$clean_vol" "$sectors" >/dev/null
python3 tools/zcfs.py format "$dirty_vol" "$sectors" >/dev/null

# A clean volume needs no repair.
clean_out=$(python3 tools/zcfs.py fsck "$clean_vol" --check)
if [ "$clean_out" != "fsck: clean" ]; then
    echo "error: clean volume reported '$clean_out', expected 'fsck: clean'" >&2
    exit 1
fi
echo "clean: $clean_out" >&2

# The dirty power-loss volume must repair, and the clamp must reclaim exactly
# one over-claimed record (the planted torn tail).
repaired_out=$(python3 tools/zcfs.py fsck "$dirty_vol" --check)
if [ "$repaired_out" != "fsck: repaired 4 -> 3" ]; then
    echo "error: dirty volume reported '$repaired_out', expected 'fsck: repaired 4 -> 3'" >&2
    exit 1
fi
echo "power-loss: $repaired_out" >&2

# Repair in place on a copy, then the same image must now report clean.
cp "$dirty_vol" "$repaired_vol"
python3 tools/zcfs.py fsck "$repaired_vol" >/dev/null
second=$(python3 tools/zcfs.py fsck "$repaired_vol" --check)
if [ "$second" != "fsck: clean" ]; then
    echo "error: repaired volume reported '$second', expected 'fsck: clean'" >&2
    exit 1
fi
echo "repaired: $second" >&2

# The repaired image must still hold the host-planted /probe: fsck truncates
# the over-claim, it does not throw away the durable prefix.
python3 - "$repaired_vol" <<'EOF'
import sys
sys.path.insert(0, "tools")
import zcfs
image = open(sys.argv[1], "rb").read()
sb, nodes = zcfs.replay(image, 0)
_, probe = zcfs.find(nodes, zcfs.ROOT_NODE, b"probe")
if probe is None:
    sys.exit("error: /probe lost by fsck")
if bytes(probe["content"]) != zcfs.HOST_PATTERN:
    sys.exit("error: /probe content changed by fsck: %r" % bytes(probe["content"]))
if not (sb["flags"] & zcfs.FLAG_CLEAN):
    sys.exit("error: repaired superblock not clean")
print("repaired: /probe intact, flag clean", file=sys.stderr)
EOF

echo "fsck verified: power-loss repaired to clean, idempotent, /probe preserved"
