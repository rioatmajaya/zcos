# Boot manifest

The userspace boot manifest, `zc.manifest`, lives at the root of the
initramfs. It names the boot in one small text file so a reader can see what the
image starts without opening a binary. Normative parser source:
`libs/zc-supervise/src/manifest.rs`.

The kernel still spawns `zc_abi::SERVICE_CATALOG` at build time, because it must
build each address space and provision capabilities before any task runs. The
manifest is therefore checked, not yet consumed for spawn order: `initd` reads
it at boot and reports whether the `init` path and the service names match the
compiled catalog. A mismatch is visible in the boot transcript instead of being
silently ignored.

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
| `services` | for a match | Comma-separated supervised service names in boot order. |

The service names are the names in `zc_abi::SERVICE_CATALOG`, not numeric ids:
an id is an ABI detail, and an operator editing a boot file should not have to
look one up. `services` may name at most eight services
(`manifest::MAX_SERVICES`); the parser rejects a longer list before it can
consume unbounded stack.

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
| `initd: manifest ok` | the `init` path and every service name match the catalog in order |
| `initd: manifest mismatch` | parsed, but a name, the order, or the `init` path differs |
| `initd: manifest malformed` | a structural error such as a missing `=`, unknown key, or duplicate service |
| `initd: manifest absent` | the file is missing |
| `initd: manifest unreadable` | the read failed or exceeded the fixed buffer |
| `initd: manifest not utf8` | the bytes are not UTF-8 |

Only the first line means the manifest agrees with the image. A failure does
not stop the boot: the kernel has already spawned the catalog, and `initd`
continues to supervise it. This keeps a stale config file from bricking a
working image while still making the disagreement loud.

The manifest is **not reloaded** at runtime, and it does not yet choose the
spawn order or the shutdown order. Those remain compile-time data until the
kernel can read the manifest before it builds address spaces.
