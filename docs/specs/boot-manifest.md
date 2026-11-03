# Boot manifest

The userspace boot manifest, `zc.manifest`, lives at the root of the
initramfs. It names the boot in one small text file so a reader can see what the
image starts without opening a binary. Normative parser source:
`libs/zc-supervise/src/manifest.rs`.

The kernel still spawns `zc_abi::SERVICE_CATALOG` at build time, because it must
build each address space and provision capabilities before any task runs. The
manifest cannot change that spawn, but it **is authoritative for which catalog
services stay supervised and in what order they shut down**: `initd` resolves
the file through `zc_supervise::plan_for`, stops any catalog service the file
omits, and uses the file's order for shutdown. An absent or unusable file falls
back to the full catalog, so a broken config cannot silently leave a service
unwatched. See
[ADR 0058](../adr/0058-the-manifest-selects-the-supervised-set.md).

## Format

One `key=value` pair per line.

- Blank lines are ignored.
- A line whose first non-space byte is `#` is a comment.
- Keys and values are trimmed of ASCII whitespace.
- An unknown or duplicate key is an error.
- An empty value is an error.

| Key | Required | Meaning |
|---|---|---|
| `name` | no | Display name, for example `ZC OS`. |
| `version` | no | Manifest or image version. |
| `init` | for a match | Path of the first userspace program, `/sbin/initd`. |
| `services` | for authority | Comma-separated supervised service names in boot order; a subset of the catalog. |

The service names are the names in `zc_abi::SERVICE_CATALOG`, not numeric ids:
an id is an ABI detail, and an operator editing a boot file should not have to
look one up. The names must form a **subsequence** of the catalog: a subset is
allowed, but the relative order must match the compile-time boot order, because
the kernel provisions the services in catalog order. `services` may name at
most eight services (`manifest::MAX_SERVICES`); the parser rejects a longer
list before it can consume unbounded stack.

The shipped file is:

```text
name=ZC OS
version=0.1.0
init=/sbin/initd
services=blk,kbd
```

## Boot check

`initd` opens `zc.manifest`, reads at most 256 bytes, and parses it. It logs one
of:

| Line | Meaning |
|---|---|
| `initd: manifest ok` | the `init` path is right and the services are a catalog subsequence; the file is authoritative |
| `initd: manifest mismatch` | parsed, but the `init` path differs, a service is unknown, the order is wrong, or no service is named |
| `initd: manifest malformed` | a structural error such as a missing `=`, unknown key, or duplicate service |
| `initd: manifest absent` | the file is missing |
| `initd: manifest unreadable` | the read failed or exceeded the fixed buffer |
| `initd: manifest not utf8` | the bytes are not UTF-8 |

Only `manifest ok` makes the file authoritative. Every other line falls back to
the full catalog: `initd` logs `initd: supervising blk,kbd` for the effective
plan, then stops any catalog service the plan omits with
`initd: <name> not in manifest, stopped`. A failure therefore never stops the
boot, but it also never disables a service by accident.

The manifest is **not reloaded** at runtime, and it does not yet choose the
spawn order. Those remain compile-time data until the kernel can read the
manifest before it builds address spaces.
