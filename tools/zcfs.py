#!/usr/bin/env python3
"""Host tooling for zcfs, the ZC-native log-structured filesystem.

This is the host half of the cross-implementation check. The guest implements
the same format in Rust (`libs/zc-storage/src/zcfs.rs`); the two must agree
byte for byte. That agreement is what makes a custom on-disk format verifiable
without a third-party `fsck`.

Usage:
    tools/zcfs.py format <path> <sectors>     build a volume with /probe
    tools/zcfs.py format --clean <path> <sectors>
                                              build a clean volume with /probe
    tools/zcfs.py dump <path>                 replay the log and list files
    tools/zcfs.py fsck <path> [--check]       repair (or report) the volume

The checksum is CRC-32/IEEE, which `zlib.crc32` computes, so the host and the
Rust side produce identical values.
"""

import struct
import sys
import zlib

SECTOR = 512
MAGIC = b"ZCFS0001"
VERSION = 1
SUPERBLOCK_A = 0
SUPERBLOCK_B = 1
LOG_START = 2
HEADER_LEN = 32
PAYLOAD_MAX = SECTOR - HEADER_LEN
NAME_MAX = 32
CONTENT_MAX = 512
KIND_CREATE = 1
KIND_DATA = 2
ROOT_NODE = 1
FLAG_CLEAN = 1
MODE_DIR = 0o040000
MODE_FILE = 0o100000
CREATE_NAME_OFFSET = 14
DATA_CONTENT_OFFSET = 6

# The pattern the host plants in /probe for the guest to verify.
HOST_PATTERN = b"ZCHOST1\n"
# The pattern the shell writes to /data/probe through the VFS, overwriting the
# host's copy. The host replays it back, proving the guest's write is durable
# and that both implementations agree on the DATA record it produced.
PERSIST_PATTERN = b"ZCPERSIST1"


def crc32(data):
    return zlib.crc32(data) & 0xFFFFFFFF


def encode_superblock(partition_sectors, head_seq, tail_seq=0, flags=FLAG_CLEAN,
                      generation=0):
    log_sectors = partition_sectors - LOG_START
    sector = bytearray(SECTOR)
    sector[0:8] = MAGIC
    struct.pack_into("<I", sector, 8, VERSION)
    struct.pack_into("<I", sector, 12, SECTOR)
    struct.pack_into("<Q", sector, 16, partition_sectors)
    struct.pack_into("<Q", sector, 24, LOG_START)
    struct.pack_into("<Q", sector, 32, log_sectors)
    struct.pack_into("<Q", sector, 40, head_seq)
    struct.pack_into("<Q", sector, 48, tail_seq)
    struct.pack_into("<Q", sector, 56, ROOT_NODE)
    struct.pack_into("<Q", sector, 64, generation)
    struct.pack_into("<I", sector, 72, flags)
    struct.pack_into("<I", sector, 76, crc32(bytes(sector[0:76])))
    return bytes(sector)


def parse_superblock(sector):
    if sector[0:8] != MAGIC:
        raise ValueError("bad superblock magic")
    if struct.unpack_from("<I", sector, 8)[0] != VERSION:
        raise ValueError("unsupported version")
    if struct.unpack_from("<I", sector, 12)[0] != SECTOR:
        raise ValueError("unsupported sector size")
    if struct.unpack_from("<I", sector, 76)[0] != crc32(bytes(sector[0:76])):
        raise ValueError("bad superblock checksum")
    fields = struct.unpack_from("<QQQQQQ", sector, 16)
    return {
        "partition_sectors": fields[0],
        "log_start": fields[1],
        "log_sectors": fields[2],
        "head_seq": fields[3],
        "tail_seq": fields[4],
        "root_node": fields[5],
        "generation": struct.unpack_from("<Q", sector, 64)[0],
        "flags": struct.unpack_from("<I", sector, 72)[0],
    }


def select_superblock(a, b):
    best = None
    for candidate in (a, b):
        try:
            parsed = parse_superblock(candidate)
        except ValueError:
            continue
        if best is None or parsed["head_seq"] > best["head_seq"]:
            best = parsed
    if best is None:
        raise ValueError("both superblock copies are unusable")
    return best


def encode_record(kind, seq, node, payload):
    if len(payload) > PAYLOAD_MAX:
        raise ValueError("payload too large")
    sector = bytearray(SECTOR)
    struct.pack_into("<I", sector, 0, kind)
    struct.pack_into("<I", sector, 4, 0)
    struct.pack_into("<Q", sector, 8, seq)
    struct.pack_into("<Q", sector, 16, node)
    struct.pack_into("<I", sector, 24, len(payload))
    sector[HEADER_LEN:HEADER_LEN + len(payload)] = payload
    struct.pack_into("<I", sector, 28, crc32(bytes(sector)))
    return bytes(sector)


def parse_record(sector):
    if struct.unpack_from("<I", sector, 28)[0] != crc32(
        bytes(sector[0:28]) + b"\0\0\0\0" + bytes(sector[32:])
    ):
        raise ValueError("bad record checksum")
    payload_len = struct.unpack_from("<I", sector, 24)[0]
    if payload_len > PAYLOAD_MAX:
        raise ValueError("bad payload length")
    return {
        "kind": struct.unpack_from("<I", sector, 0)[0],
        "seq": struct.unpack_from("<Q", sector, 8)[0],
        "node": struct.unpack_from("<Q", sector, 16)[0],
        "payload_len": payload_len,
        "payload": bytes(sector[HEADER_LEN:HEADER_LEN + payload_len]),
    }


def create_payload(parent, mode, name):
    if not name or len(name) > NAME_MAX:
        raise ValueError("bad name")
    payload = bytearray(CREATE_NAME_OFFSET + len(name))
    struct.pack_into("<Q", payload, 0, parent)
    struct.pack_into("<I", payload, 8, mode)
    struct.pack_into("<H", payload, 12, len(name))
    payload[CREATE_NAME_OFFSET:] = name
    return bytes(payload)


def parse_create(payload):
    parent = struct.unpack_from("<Q", payload, 0)[0]
    mode = struct.unpack_from("<I", payload, 8)[0]
    name_len = struct.unpack_from("<H", payload, 12)[0]
    if name_len == 0 or name_len > NAME_MAX or CREATE_NAME_OFFSET + name_len > len(payload):
        raise ValueError("bad create payload")
    return parent, mode, bytes(payload[CREATE_NAME_OFFSET:CREATE_NAME_OFFSET + name_len])


def data_payload(offset, data):
    if len(data) > PAYLOAD_MAX - DATA_CONTENT_OFFSET:
        raise ValueError("content too large")
    payload = bytearray(DATA_CONTENT_OFFSET + len(data))
    struct.pack_into("<I", payload, 0, offset)
    struct.pack_into("<H", payload, 4, len(data))
    payload[DATA_CONTENT_OFFSET:] = data
    return bytes(payload)


def parse_data(payload):
    offset = struct.unpack_from("<I", payload, 0)[0]
    length = struct.unpack_from("<H", payload, 4)[0]
    if DATA_CONTENT_OFFSET + length > len(payload):
        raise ValueError("bad data payload")
    return offset, bytes(payload[DATA_CONTENT_OFFSET:DATA_CONTENT_OFFSET + length])


def replay(image, partition_lba=0):
    """Replays the log and returns (superblock, nodes).

    `image` is the whole disk; `partition_lba` locates the volume inside it.
    A torn or corrupt tail simply ends the replay, matching the guest.
    """
    base = partition_lba * SECTOR

    def sector_at(relative):
        start = base + relative * SECTOR
        return image[start:start + SECTOR]

    sb = select_superblock(sector_at(SUPERBLOCK_A), sector_at(SUPERBLOCK_B))
    nodes = {}
    seq = sb["tail_seq"] + 1
    while seq <= sb["head_seq"]:
        relative = sb["log_start"] + ((seq - 1) % sb["log_sectors"])
        try:
            header = parse_record(sector_at(relative))
        except ValueError:
            break
        if header["seq"] != seq:
            break
        if header["kind"] == KIND_CREATE:
            try:
                parent, mode, name = parse_create(header["payload"])
            except ValueError:
                break
            nodes[seq] = {
                "parent": parent,
                "name": name,
                "mode": mode,
                "kind": "dir" if mode & MODE_DIR else "file",
                "content": bytearray(),
            }
        elif header["kind"] == KIND_DATA:
            node = nodes.get(header["node"])
            if node is None:
                break
            try:
                offset, data = parse_data(header["payload"])
            except ValueError:
                break
            end = offset + len(data)
            if end > CONTENT_MAX:
                break
            if len(node["content"]) < end:
                node["content"].extend(b"\0" * (end - len(node["content"])))
            node["content"][offset:end] = data
        else:
            break
        seq += 1
    # Recovery, mirroring the guest: the log decides the head. A superblock may
    # claim records a crash never made durable, so the head is clamped back to
    # the last record that actually replayed. Without this the host would
    # disagree with `Volume::mount_into` on any over-claiming image.
    sb["head_seq"] = seq - 1
    return sb, nodes


def find(nodes, parent, name):
    for node_id, node in nodes.items():
        if node["parent"] == parent and node["name"] == name:
            return node_id, node
    return None, None


def format_volume(path, sectors, pattern=HOST_PATTERN, clean=False):
    """Writes a volume holding the root directory, /probe, and, unless
    `clean`, a torn tail.

    The torn tail is deliberate: it is the shape a power loss leaves behind, so
    every boot has to recover before it can serve the volume. The superblock
    claims the record the crash never made durable, which is exactly the
    over-claim that `Volume::mount_into` must clamp. With `clean`, the volume
    is written as a cleanly unmounted one, with no torn tail and `FLAG_CLEAN`
    set, which is what `fsck` is supposed to produce.
    """
    image = bytearray(sectors * SECTOR)

    def put(relative, sector):
        start = relative * SECTOR
        image[start:start + SECTOR] = sector

    seq = 0

    def append(kind, node, payload):
        nonlocal seq
        seq += 1
        put(LOG_START + ((seq - 1) % (sectors - LOG_START)),
            encode_record(kind, seq, node, payload))
        return seq

    root = append(KIND_CREATE, 0, create_payload(0, MODE_DIR | 0o755, b"/"))
    if root != ROOT_NODE:
        raise AssertionError("the root must be node %d" % ROOT_NODE)
    # World-writable: the shell runs as an unprivileged user (F7j) and
    # rewrites /probe through the VFS, so the mode must let it. The mode is
    # the only owner information the on-disk record carries today.
    probe = append(KIND_CREATE, 0,
                   create_payload(ROOT_NODE, MODE_FILE | 0o666, b"probe"))
    append(KIND_DATA, probe, data_payload(0, pattern))
    committed = seq

    if clean:
        # A clean, consistent volume: heads match, flag is set.
        superblock = encode_superblock(sectors, committed, flags=FLAG_CLEAN)
        put(SUPERBLOCK_A, superblock)
        put(SUPERBLOCK_B, superblock)
        with open(path, "wb") as handle:
            handle.write(bytes(image))
        return committed

    # The power-loss tail: a well-formed record for the next sequence, then one
    # flipped byte so its checksum can never pass. The superblock below points
    # at it anyway.
    torn_seq = committed + 1
    torn = bytearray(encode_record(KIND_DATA, torn_seq, probe,
                                    data_payload(0, b"lost")))
    torn[200] ^= 0xFF
    put(LOG_START + ((torn_seq - 1) % (sectors - LOG_START)), bytes(torn))

    # Not clean: the volume was never unmounted, and the head over-claims the
    # torn record.
    superblock = encode_superblock(sectors, torn_seq, flags=0)
    put(SUPERBLOCK_A, superblock)
    put(SUPERBLOCK_B, superblock)
    with open(path, "wb") as handle:
        handle.write(bytes(image))
    return torn_seq


def fsck_image(data, check=False):
    """Replay `data` and repair it in a returned copy.

    Mirrors the guest's `Volume::repair`: the durable log decides the head, so
    an over-claiming or dirty superblock is rewritten with the clamped head and
    `FLAG_CLEAN`. Returns `(report, repaired, image)`; when `check` is true the
    report is computed but `image` is unchanged.
    """
    superblock = select_superblock(data[0:SECTOR], data[SECTOR:2 * SECTOR])
    sb, nodes = replay(data)
    claimed = superblock["head_seq"]
    clamped = sb["head_seq"]
    repaired = not (superblock["flags"] & FLAG_CLEAN) or claimed != clamped
    result = bytes(data)
    if repaired and not check:
        newsb = encode_superblock(sb["partition_sectors"], clamped,
                                  tail_seq=sb["tail_seq"], flags=FLAG_CLEAN,
                                  generation=sb["generation"])
        result = bytearray(data)
        result[SUPERBLOCK_A * SECTOR:SUPERBLOCK_A * SECTOR + SECTOR] = newsb
        result[SUPERBLOCK_B * SECTOR:SUPERBLOCK_B * SECTOR + SECTOR] = newsb
        result = bytes(result)
    if repaired:
        report = "fsck: repaired %d -> %d" % (claimed, clamped)
    else:
        report = "fsck: clean"
    return report, repaired, result, nodes


def main(argv):
    if argv[1] == "format" and "--clean" in argv:
        # format --clean <path> <sectors>
        path, sectors = argv[3], int(argv[4])
        seq = format_volume(path, sectors, clean=True)
        print("zcfs: formatted %s (%d sectors, head_seq %d, clean)"
              % (path, sectors, seq))
        return 0
    if len(argv) >= 4 and argv[1] == "format":
        sectors = int(argv[3])
        seq = format_volume(argv[2], sectors)
        print("zcfs: formatted %s (%d sectors, head_seq %d, torn tail)"
              % (argv[2], sectors, seq))
        return 0
    if len(argv) >= 3 and argv[1] == "fsck":
        path = argv[2]
        check = len(argv) >= 4 and argv[3] == "--check"
        with open(path, "rb") as handle:
            data = handle.read()
        report, repaired, image, _ = fsck_image(data, check)
        if repaired and not check:
            with open(path, "wb") as handle:
                handle.write(image)
        print(report)
        return 0
    if len(argv) == 3 and argv[1] == "dump":
        with open(argv[2], "rb") as handle:
            image = handle.read()
        sb, nodes = replay(image)
        print("zcfs: head=%d tail=%d clean=%d log=%d..%d"
              % (sb["head_seq"], sb["tail_seq"],
                 bool(sb["flags"] & FLAG_CLEAN), sb["log_start"],
                 sb["log_start"] + sb["log_sectors"]))
        for node_id in sorted(nodes):
            node = nodes[node_id]
            print("zcfs: node %d %s %r size=%d content=%s"
                  % (node_id, node["kind"], node["name"].decode("utf-8", "replace"),
                     len(node["content"]), bytes(node["content"]).hex()))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
