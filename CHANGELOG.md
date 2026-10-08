# Changelog

All notable changes to ZC OS are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

ZC OS is pre-release: the `0.x` line tracks the roadmap phases in
[`docs/roadmap.md`](docs/roadmap.md), and `1.0.0` is reserved for the first
desktop daily-driver release. See
[CONTRIBUTING.md](CONTRIBUTING.md#changelog-policy) for how entries are
maintained.

## [Unreleased]

Roadmap phase **F6 — driver userspace** closed the runtime-authority story:
what a domain may touch is now granted by data, delegated at runtime, and
claimed explicitly. Phase **F7 — VFS & storage** has started: a write path
(F7a), a write-back cache (F7b), a read-only FAT32 mount (F7c), a read-only
ext2 mount (F7c-2), a read-only VFS core (F7d), a ZC-native log-structured
filesystem (F7e), the writable volume mounted into the kernel VFS (F7e-2),
a userspace supervisor that owns service lifecycle (F7f), crash recovery
that clamps the log head (F7g), an `fsck` repair that makes an unclean
mount clean again (F7h), `devfs`/`tmpfs` mounts (F7i), and file permissions
and ownership in the VFS (F7j). Phase **F8 — desktop** has started with the
userspace compositor (F8a-1): a capability-gated surface protocol, a
full-screen back buffer, and damage tracking the kernel verifies. F8a-2 adds
the window manager and the first client window, painted by a second task over a
delegated surface. F8b adds the PS/2 mouse to the input domain. F8c adds a
2D renderer: a bitmap font and a pure text overlay the compositor and the
kernel's frame verifier share, so the window title label renders as part of the
single-source-of-truth desktop layout and is proven by the same checksum. F8d
makes the client window a live graphical terminal: the kernel feeds it a
scripted keystroke session over a new `SYS_TERM_READ`, the client drives a
shared `zc-abi::terminal::Term` state machine and paints the result, and the
kernel replays the same script through the same state machine to verify the
window — so dynamic content stays a proof, not a trusted claim. F8f adds the
desktop chrome: a taskbar (launcher, focused task button, clock) and window
decorations (minimize and close), all deterministic and proven by the same
checksum. F8d-3a makes the keyboard domain persistent: a supervised service
`initd` revives after its boot fault, which then serves input for the rest of
the boot until the supervisor stops it at shutdown. F8d-3b wires that live
keyboard to the window: the kernel routes PS/2 bytes to a per-window input
queue instead of the serial ring, `SYS_TERM_READ` serves the window queue, and
the client and compositor become event loops that repaint on each keystroke
until the session closes. F8d-3c makes that dynamic window provable: the frame
checksum's window region becomes a placement proof against the client's own
surface (which the kernel owns and hashes before the compositor frees it),
while a separate boot check keeps proving the scripted content exactly. F8d-3d
connects that terminal to the VFS: commands run through one shared executor
with a caller-supplied file reader, so the scripted `cat` is proven against the
kernel's own mount table. F8b-2 draws and moves the pointer: a shared
`zc-abi::cursor` sprite and a non-blocking `SYS_MOUSE_READ`, composited topmost
with damage tracking and verified pixel by pixel. Track
**K — kernel minimalism** has started: M1 moves the storage parsers out of the
privileged crate. M2 moves PCI enumeration out of it too — the ring-3 device
manager scans the bus and brokers the BAR it finds, so the kernel never touches
it and `kernel/zc-kernel-image/src/pci.rs` is gone. It brokers device memory
too: the same manager discovers the AHCI controller's ABAR and the kernel maps
it into the domain, so a ring-3 driver can touch its registers while ring 0
still learns no address.

### Fixed

- **A CI build could leave the wrong kernel in the image a person boots** (F8b-7):
  `build-efi.sh --test-close` wrote to the same path as the default build, so the
  last build won. A CI close run therefore left `build/zcos.img` holding the
  close-proof kernel, and the next `tools/run-qemu.sh` booted a desktop that
  closes its own window — reported as "my fix had no effect", when the fix was
  fine and the image was simply the other variant. Each variant now has its own
  image (`build/zcos.img` and `build/zcos-close.img`), verified isolated: a full
  close-proof build and boot leaves the interactive image byte-identical.
- **`run-qemu.sh` refuses to boot an image older than the sources.** The script
  never built, so an image left over from an earlier edit booted yesterday's
  kernel and looked like a fix that did not work. A `find -newer` check turns that
  silent failure into a message naming the build command to run. It catches
  *unbuilt sources*, not a wrong feature variant — which is why the variants now
  write to different paths rather than relying on a check to tell them apart.
- **The interactive desktop no longer destroys itself at the end of its demo**
  (F8b-7): the scripted mouse session ended by clicking the close glyph, so
  `tools/run-qemu.sh` left a bare desktop, an exited compositor and an exited
  window client with nothing to interact with. The cause is structural — the
  compositor drains the whole scripted session before entering its event loop, so
  a close click always lands before the client has consumed its keystrokes, and
  "ends with a window on screen" and "ends with a close" are mutually exclusive in
  one run. `MOUSE_SCRIPT` is now one array served at one of two lengths:
  `script_len(close)` returns the whole script or the prefix before the close
  suffix, so the shared gestures are described exactly once and the two variants
  cannot drift. The default build serves the window-up prefix and leaves a live
  window; `build-efi.sh --test-close` sets a `close-proof` feature on the kernel
  so CI can still prove the close protocol end to end. CI now runs both scenarios,
  and each asserts the other's markers are absent, so placement (ADR 0018) is
  proven again while the erase (ADR 0026) keeps its proof — total coverage is
  higher than at any single point in this sequence. See
  [ADR 0029](docs/adr/0029-two-scripted-sessions.md).
- **An interactive boot no longer kills itself after five seconds** (F8b-6):
  booting with `tools/run-qemu.sh` and touching nothing froze the machine with
  `user: timed out after 5019 ticks` — the input driver stopped and the pointer
  went dead, which is exactly the symptom the task watchdog was introduced to
  prevent. The deadline exists so a wedged task fails the *headless boot test*
  rather than hanging CI, and that is the only configuration with a harness behind
  it; in every other build `halt()` is the only way this kernel stops, so there
  is no exit code or assertion that could distinguish a genuine wedge from a
  session that is simply still running. The deadline is now armed only under the
  `qemu-exit` feature that `build-efi.sh --test` sets, and a normal build keeps
  it at `u64::MAX` for good. The earlier revision disarmed it on the first byte of
  live input, which let anyone who typed or moved the mouse survive but left a
  five-second grace period for someone watching the desktop draw itself — exactly
  what the scripted session asks of them. Verified by booting a normal build,
  sending nothing for 45 seconds, and finding it still running with no timeout in
  the log; CI is unchanged and still fails fast. See
  [ADR 0028](docs/adr/0028-boot-watchdog-only-in-test-builds.md).

### Added

- **One shared input layer, and the start of F8e** (F8e-1): the PS/2 mouse
  reports the *current* button mask rather than changes, so "the button went down
  now" has to be derived by comparing against the previous report — which the
  window manager had solved for itself, the compositor had solved again slightly
  differently, and every future app would have solved once more. `zc_abi::ui`
  derives each edge once: `Input` emits exactly one `Event` per report (`Key`,
  `Move`, `Press`, `Release`), adopting the first report rather than treating it
  as an edge so a button already held when a session began cannot spuriously grab
  whatever was under the pointer, and carrying movement *on* the edge so a click
  acts at the pixel the report moved it to. `Wm::apply` now takes an `Event` and
  has lost its own `held` flag, and the kernel keeps its own `Input` beside its
  cursor — so the compositor and the verifier see one event stream instead of two
  readings of one input, which is what keeps the window-placement proof honest.
  `hit_topmost` and `hit_all` resolve a widget stack in the same last-painted-wins
  order `color_at` uses, allocating nothing. Keyboard events stay byte-only
  because the input domain has already folded releases into its ASCII output.
  The boot checksum is unchanged, which is the evidence this is a refactor rather
  than a feature. See [ADR 0027](docs/adr/0027-one-input-layer.md).
- **Closing a window** (F8b-5): the `x` glyph now ends the window's session for
  good. The client spends its session blocked in `SYS_TERM_READ` and the only
  end-of-session signal existed for the serial shell exiting, so the new
  `SYS_WINDOW_CLOSE` (33) posts exactly that one: the client is rewound, retries,
  paints a final frame and sends `WM_DONE` by the same path it already used, which
  is why the client needed no change at all. Only the holder of the surface
  factory may call it — the window manager, who created and delegated the window —
  so a client cannot end its own session and strand the compositor waiting for a
  frame that will not arrive. `zc_abi::wm::Wm` gains a terminal `closed` flag
  beside `shown`: the placement is remembered for the erase, the task button will
  not restore a closed window, and every later report is absorbed. The compositor
  erases a closed window across its remembered footprint — passing that footprint
  as clip bounds and an empty placement to the painter, so the blit rejects
  itself — and erases nothing in any other case, because a session that ended by
  the shell exiting leaves its pixels standing and those are what the placement
  proof compares. The scripted session now drives the whole lifecycle
  (drag → minimize → restore → close), so CI proves all four. The boot checksum
  changes. This trade is explicit: a session ending with a window up proves
  *placement* (ADR 0018), one ending with the window gone proves the *erase*, and
  the shipped script ends closed — see
  [ADR 0026](docs/adr/0026-closing-a-window-ends-a-session.md).
- **Minimize and restore** (F8b-4): clicking the `-` glyph now hides the window
  and the taskbar's `Terminal` button brings it back exactly where it was. The
  representation is the whole design: a hidden window's rectangle *is*
  `Rect::EMPTY`, so the shared desktop layout already recomputes every pixel it
  vacated as bare desktop and the compositor's clip bounds already reject the
  blit — neither path needs a new "is it visible" branch, and the area a
  minimize vacates is covered by the exact frame check rather than merely going
  unobserved. `zc_abi::wm::Wm` keeps the placement across a hide so a restore is
  exact, and `Wm::apply` returns an `Action` (`Grabbed`, `Moved`, `Released`,
  `Minimized`, `Restored`) so a caller can tell a grab from a move from a hide.
  Click regions are now derived from the renderers — `terminal::close_rect` and
  `minimize_rect` are public and `Term::render` calls them, as
  `desktop::launcher_button_rect` and `task_button_rect` are for `panel_color_at`
  — so a click can never land beside a glyph the user can see. The scripted
  session now drives the whole round trip, so CI proves minimize and restore
  (`compositor: window minimized`, `compositor: window restored`) rather than
  only a drag. Close stays drawn but inert: it ends the window session, which
  needs its own protocol. The boot checksum changes, because the scripted
  pointer path is different. See
  [ADR 0025](docs/adr/0025-hidden-window-is-an-empty-rectangle.md).
- **Clicking the title bar drags the window** (F8b-3): the mouse button bitmask
  crossed `SYS_MOUSE_READ` with nothing acting on it, so a click moved the
  pointer and nothing else. `zc_abi::wm` adds a shared placement machine — a
  left-button press inside the window's title bar arms a drag, the window follows
  the pointer with the grab offset preserved, and the release ends it. Presses
  on the window body, on the minimize/close glyphs, or off the window move
  nothing, and `clamp_window` keeps a dragged window on screen and clear of the
  taskbar. The kernel runs the *same* machine over the same reports it already
  applied to the pointer, so it derives the window's rectangle instead of being
  told it, and the frame verifier recomputes the desktop against that exact
  rectangle via `pixel_at_with_window` — so `fb: desktop checksum ok` stays an
  exact check after a drag instead of becoming a claim. Damage now covers the
  window's old *and* new rectangle, so the area a drag vacates is repainted
  rather than left stale. The scripted mouse session carries button bits and
  ends with a full press-drag-release, so CI proves the interaction
  (`compositor: window dragged`) rather than only the pointer motion. The boot
  checksum changes, because the window ends where the script dragged it. See
  [ADR 0024](docs/adr/0024-window-dragging-and-hit-testing.md).
- **Pointer and mouse-read syscall** (F8b-2): the desktop now draws and moves a
  mouse pointer. `zc-abi::cursor` holds the 8x12 arrow sprite, a screen-clamped
  `Cursor`, and the report codec, and the new `SYS_MOUSE_READ` (32) serves the
  next report — packed as `(buttons << 16) | (dx << 8) | dy`, with
  `MOUSE_NO_REPORT` when none is waiting — non-blocking. The kernel applies each
  report to its own cursor as it serves it, and the compositor draws the sprite
  topmost and repaints only the union of its old and new rectangles, so the
  pointer moves with damage tracking and `fb: cursor ok` proves every sprite
  pixel. `MOUSE_SCRIPT` entries became `MouseStep { buttons, dx, dy }` when
  F8b-3 added the click; the report packing itself is unchanged. See
  [ADR 0023](docs/adr/0023-pointer-and-mouse-read.md).
- **Terminal commands over the VFS** (F8d-3d): the graphical terminal's
  commands now act on the F7 filesystem. `zc_abi::terminal::Term` became a pure
  editor — Enter returns the submitted `Line` — and one shared `run_command`
  dispatches `help`, `echo`, and the new `cat` with a caller-supplied reader:
  the client injects the VFS syscalls and the kernel's frame verifier injects
  its own mount table. The boot script is `help\ncat hello.txt\n`, so the kernel
  derives the window screen from the initramfs and `wm: content ok` proves the
  client's read end to end; the kernel also logs `wm: vfs content ok` and the
  client `client: vfs ok`. See
  [ADR 0022](docs/adr/0022-terminal-commands-over-the-vfs.md).
- **Device memory broker** (K8-a): a ring-3 driver can now map a device's
  memory-mapped registers without the kernel knowing the BAR ahead of time. The
  device manager holds `MMIO_BROKER_OBJECT` (a new object namespace, bit 27)
  with `GRANT` only, discovers the AHCI controller's ABAR, and brokers the
  range to itself; the kernel validates it is device-shaped and not
  kernel-owned memory (`device::mmio_range_allowed` refuses usable RAM, the
  framebuffer, an unaligned, empty, oversized, or overflowing range), records
  it in `zc_kernel::mmio`, mints the capability, and maps the registers
  uncached (`PTE_PCD`) and non-executable with the new `SYS_MMIO_MAP` (31). A
  new `syscall5` passes the 64-bit base and length in `r10` and `r8`. See
  [ADR 0020](docs/adr/0020-device-memory-broker.md).
- **Coherent DMA window** (K8-a): a driver that programs a device to move data
  itself now gets memory the device can address. At spawn the kernel allocates
  64 KiB of physically contiguous frames for the roles the device table names
  (the device manager), maps them at `DMA_VIRT`, and publishes the
  device-visible base in a `DmaInfo` at `DMA_INFO_VIRT`, so the virtual alias
  and the physical base are the same frames and a buffer needs no copy. A new
  `FrameAllocator::allocate_contiguous` fails without consuming anything on a
  fragmented heap, so the boot stops loudly rather than program a device with a
  broken ring. The manager writes two proof words through the alias and the
  kernel reads them back through the base (`dma: window coherent ok`). See
  [ADR 0021](docs/adr/0021-coherent-dma-window.md).

- **Frozen interface specifications**: `docs/specs/` now documents the
  normative interfaces that cross the kernel/userspace boundary — the syscall
  ABI (numbers, arguments, return codes, capability rights and object
  namespaces, fixed per-task windows), the IPC ABI (message layout, the seven
  channels, the window and supervision word encodings), the boot-info ABI
  (loader hand-off and validation), and the server protocol (service table,
  discovery handshake, filesystem exchange page). The Rust code in `libs/zc-abi`
  stays the single source of truth; the specs describe it and must change with
  it, so a server can never silently drift from the kernel.
- **Window placement proof** (F8d-3c): the frame verifier no longer needs to
  know the window's exact pixels. It records the surface the compositor
  delegates to the window client, snapshots a hash of that surface's pixels on
  the destroy path (the compositor releases it before the boot ends), and
  checks the display's window region against the snapshot — proving the
  compositor placed the client's pixels faithfully for *any* content, scripted
  or typed. The desktop outside the window stays exact (`pixel_at`), and the
  same snapshot pass still compares the surface to `Term` replayed over the
  script, logging `wm: content ok` as the deterministic-boot content proof.
  `Surface::pixel_location` is host-tested. See
  [ADR 0018](docs/adr/0018-window-placement-proof.md).
- **Input-driven window session** (F8d-3b): the window terminal is now driven
  by real input routing rather than a kernel-only script feed. The kernel
  splits the input domain's byte stream: mouse frames are consumed for the
  cursor, and keyboard bytes are routed to a per-window queue
  (`zc_kernel::input::route`, host-tested) instead of the COM1 ring the shell
  reads. `SYS_TERM_READ` (30) now serves that window queue after the scripted
  session and blocks while the window is open, so the client reads one
  keystroke at a time. `zc-win` repaints and sends `WM_ACK` per keystroke;
  `zcompositor` runs an event loop that re-composites the window at its moved
  position and flushes only that rectangle on every `WM_ACK`, and stops on the
  new `WM_DONE`. When the shell exits, the kernel closes the window session, so
  the client's read returns `u64::MAX`, it paints its final frame and exits,
  and the compositor follows. The frame checksum still recomputes the expected
  window from the shared `Term` state machine, so the event-driven pixels stay
  a proof.
- **Persistent keyboard driver, supervised by `initd`** (F8d-3a): the kbd
  domain is no longer a one-shot proof that faults and dies. It still runs its
  boot proofs and faults deliberately once (the F6 fault-isolation proof), but
  it is now a supervised service: `initd` revives it after that fault, and the
  revived run re-claims the IRQ sources and controller ports the fault revoked
  and enters an `irq_wait → drain → push` loop (`kbd: serving`) that keeps
  draining the 8042 for the rest of the boot. Because a live driver would
  otherwise keep the boot from ever finishing, `initd` stops the kbd service
  when the block driver stops, so the boot still ends at `fb: desktop checksum
  ok`. The keyboard domain joins the block driver in `zc_kernel::service`
  (`KBD_SERVICE`), so it also publishes `/dev/kbd` through `devfs`. The
  now-unused kernel-side restart budget was removed; a faulted supervised
  service is revived by its supervisor, exactly like the block driver.
- **Taskbar and window decorations** (F8f): the top panel is now a taskbar —
  a launcher button (`ZC`), a task button for the focused window (`Terminal`),
  and a clock — drawn with the F8c bitmap font in `zc_abi::desktop::panel_color_at`.
  The window title bar gains minimize (`-`) and close (`x`) glyphs in
  `Term::render`. Both are fully deterministic and route through the same
  shared layout the kernel's frame verifier uses, so `fb: desktop checksum ok`
  proves them: the taskbar is verified as part of the static desktop, and the
  decorations as part of the window region. No compositor, client, or kernel
  change was needed — the chrome lives entirely in the shared `zc-abi` layout.
  Host tests assert the launcher, task button, labels, and background paint,
  and that both decoration glyphs render.
- **Live graphical terminal driven by input** (F8d-2): the client window is now
  driven by keystrokes rather than a fixed transcript. A new `SYS_TERM_READ`
  (30) serves the kernel's scripted session (`zc-abi::terminal::SCRIPT`), and
  `zc-abi::terminal::Term` is a shared, allocation-free state machine — prompt
  editing, backspace, `help`/`echo` commands, and scrolling — that both the
  client and the kernel run. The client applies the keys it reads and paints
  `Term::pixel`; the kernel replays the same script and verifies the window
  region against `Term::render`, so a client that drops a key or mis-renders
  fails `fb: desktop checksum ok` instead of shipping silently. This is the new
  proof model for non-deterministic client pixels: the kernel derives the
  expected content independently instead of trusting the client's surface.
  `window_color_at` is now the deterministic placeholder the compositor draws
  before it blits the client's live surface. Host tests cover prompt editing,
  backspace, `echo`, unknown commands, scrolling, and rendering. Routing real
  blocking keyboard input is the follow-up; this proves the input → command →
  render path deterministically.
- **Graphical terminal window** (F8d-1): `zc-abi::terminal` holds the
  deterministic terminal content the client window renders: a `Terminal` title
  bar and a body with a prompt, the shell's `help` output, and a steady cursor
  block, all drawn with the F8c bitmap font on a dark background. Because the
  client paints its surface with `window_pixel_at` and the kernel verifies the
  frame with `window_color_at`, both route through this one module, so the
  transcript is proven by `fb: desktop checksum ok` rather than assumed. Host
  tests assert the prompt, output, and background all paint, and that the
  padding stays dark.
- **2D renderer with a bitmap font and text overlay** (F8c): `zc-abi::font`
  embeds the standard VGA 8x16 glyph set (ASCII `0x20`..=`0x7F`, carried over
  from the earlier prototype's `font8x16.raw`) and exposes `glyph_row`,
  `glyph_bit`, `text_blend`, and `text_width` as pure, `const`-callable helpers
  with no allocation. `text_blend` layers a string over any base color with
  1-bit alpha, so it composites over the title bar or any surface. The window
  title bar now renders the label `ZC OS` through `window_color_at`, which both
  the compositor (`window_pixel_at`) and the kernel's frame verifier
  (`pixel_at` → `color_at` → `window_color_at`) call — one source of truth, so
  the label is proven by `fb: desktop checksum ok` rather than assumed. A host
  test asserts a glyph stroke pixel takes the foreground color while the bar
  color survives underneath, proving text actually renders. The proof holds for
  both desktop frames, because the moved-window verifier reuses the same
  function.
- **Input service with a PS/2 mouse** (F8b): the kbd domain (task 5) now owns
  both 8042 lines — the keyboard on ISA IRQ1 and the mouse on ISA IRQ12 with a
  new `IRQ_MOUSE` source and `MOUSE_VECTOR` (0x22). Mouse reports travel
  tag-framed in the same input ring (`0xFF 'M' buttons dx dy`), which the
  kernel drain strips before the shell, so the shell transcript is untouched
  and one stream serves every client. The 8042 aux enable runs best-effort
  (QEMU's CI config guarantees no device); the loopback and self-IPI IRQ tests
  prove the assembler and delivery without hardware
  (`input: mouse loopback ok`, `input: mouse irq self-test ok`,
  `kbd: irq 34 delivered`). `TASK_COUNT`-sized IRQ ownership also widens
  (`MAX_TASKS` 8 → 16), and the kernel stashes the latest mouse state for the
  future pointer consumer (see
  [ADR 0017](docs/adr/0017-input-stream-with-mouse.md)).
- **Window manager and the first delegated client window** (F8a-2): the
  compositor creates a window surface and hands it to a new client task,
  `user/zc-win` (slot 8), with `SYS_CAP_DELEGATE`; the client maps it, paints
  the deterministic window content, and acknowledges. Assignment and
  acknowledgement travel on two dedicated IPC channels, `IPC_WM` and
  `IPC_WM_REPLY`, mirroring the filesystem request/reply split. The compositor
  composites the client's pixels and moves the window, logging
  `wm: window mapped` and `wm: move ok`; the kernel's existing frame checksum is
  unchanged, so a client that fails to paint fails the boot. The task table
  widens from eight slots to nine (see
  [ADR 0016](docs/adr/0016-window-client-delegation.md)).
- **Userspace compositor with a surface protocol and damage tracking** (F8a-1):
  `user/zcompositor` takes over the display slot and creates a full-screen back
  buffer through three new capability-gated syscalls — `SYS_SURFACE_CREATE`,
  `SYS_SURFACE_MAP`, and `SYS_SURFACE_DESTROY` (27/28/29). A surface is a
  kernel-owned set of frames handed out by capability (bit 29, beside port and
  service caps), so a client can give the compositor a read-only view with the
  existing `SYS_CAP_DELEGATE` and no pixel ever crosses an IPC message. The
  compositor paints a deterministic desktop, moves its window, and repaints
  only the damaged rectangles; the kernel recomputes the expected final frame
  from the same pure `zc_abi::desktop` layout and fails the boot on a mismatch
  (`fb: desktop checksum ok`), which makes damage tracking a proof rather than
  an assumption. The boot logs `compositor: damage ok (235008/1024000 px)` —
  only 23% of the screen was touched. `zc-fb` is replaced by `zcompositor`
  (see [ADR 0015](docs/adr/0015-compositor-surface-model.md)).
- **File permissions and ownership in the VFS** (F7j): every task now carries
  an identity and every node an owner, and the VFS enforces POSIX-style mode
  bits on open, read, write, and create. A new pure `zc-kernel::perms` module
  holds the policy — owner/group/other classes, checked without unioning, with
  root bypassing — so it is host-tested like the rest of the crate. The check
  lives at the syscall boundary rather than in each filesystem, because a
  `FileSystem` method has no caller; the dispatcher passes the caller's
  `Identity` in, and a descriptor caches the `Stat` taken at open so a later
  read or write needs no second filesystem call. `Stat` grows `uid`/`gid`
  (24 → 32 bytes), `tmpfs` records the creator as owner, and `ramfs`/`devfs`/
  `zcfs` report root. `initd` is the only root task; the shell runs as an
  unprivileged user, so the checks are observable: a new `SYS_CHMOD` and shell
  `chmod` clear `/tmp/scratch`'s read bit, the kernel logs
  `audit: task 2 denied read /tmp/scratch`, and restoring the mode lets the
  read through. The shell's `stat` now prints the mode and owner the decision
  is made from. Ownership is runtime-only for now — the zcfs on-disk record
  carries the mode but not the owner, so a reboot resets owners to root (see
  [ADR 0013](docs/adr/0013-permissions-and-ownership.md)).
- **`tmpfs` and `devfs` mounts** (F7i): the VFS now holds four filesystems at
  once — the initramfs at `/`, the zcfs volume at `/data`, `devfs` at `/dev`,
  and `tmpfs` at `/tmp` — and `MAX_MOUNTS` rises from 4 to 8 so a session can
  mount and unmount without exhausting the table. `tmpfs` is a fixed-capacity
  writable filesystem with inline storage, so it needs no allocator and no
  other task: it proves the VFS write path without a disk. Since the kernel
  crate is `no_std` and denies `unsafe` while every `FileSystem` method takes
  `&self`, its interior mutability comes from `core::cell::RefCell` rather than
  an `UnsafeCell`, and the kernel image holds the volume in a `static mut` and
  lends `&'static` — the same trade the initramfs already makes. `create` is
  idempotent, because the shell creates then re-opens. `devfs` publishes one
  node per supervised service straight from `service::SERVICES`, so `/dev`
  cannot drift from the bring-up layout; a new `KIND_CHR` node kind makes a
  device distinguishable from a file, and its nodes are descriptive — `read`
  is end-of-file — because a device's bytes belong to its driver, not to the
  namespace. The shell grows a `tmp` command that writes and reads
  `/tmp/scratch`, and `stat` prints `char device` for a `KIND_CHR` node.
- **`fsck` repair in `zcfs`** (F7h): `Volume::repair` is the consumer of
  `FLAG_CLEAN` that F7g left without a caller. `mark_clean` is now wired into
  `FS_OP_UNMOUNT`, so a clean unmount sets the flag; on mount the block domain
  reads it (`blk: zcfs dirty`/`clean`) and, when it recovered an over-claiming
  superblock, runs `repair` to persist the clamped head and stamp `FLAG_CLEAN`
  (`blk: zcfs fsck repaired (4 -> 3)`). Repair truncates the over-claim, never
  the durable prefix: `/probe` survives, a fresh append reuses the gap, and a
  repaired image mounts with no recovery. `tools/zcfs.py fsck` mirrors the
  repair byte for byte — a clean volume reports `fsck: clean`, a power-loss
  volume `fsck: repaired 4 -> 3` and then `fsck: clean` on the next pass —
  and `tools/check-fsck.sh` drives that cycle. A new host test
  `a_power_loss_mid_write_is_fscked_to_clean` sweeps every mid-write boundary
  with a volatile write-back `BlockIo` and asserts each crash reaches clean
  (see [ADR 0012](docs/adr/0012-fsck-repairs-the-durable-prefix.md)).
- **Crash recovery in `zcfs` mount** (F7g): `Volume::mount_into` now clamps
  `head_seq` to the last record that actually replayed and reports it through
  `was_recovered()`. A superblock can claim a record a crash never made durable,
  and before the clamp `append` derived the next sequence from that claim — so
  the new record landed *past* the gap and no later mount could ever reach it.
  That was silent, permanent loss on the first write after a crash, and the
  existing torn-tail test could not express it (its on-disk head already matched
  the durable prefix, so the clamp was never exercised). Recovery is part of
  mount, so it needs no separate tool and no flag: the corrected head is
  persisted by the next append's superblock update, and re-clamping is
  idempotent. Two host tests back it — a `BlockIo` double with a volatile
  write-back layer sweeps every power-loss boundary in a mixed workload and
  asserts the tree is always a consistent, appendable prefix, and a crafted
  over-claiming superblock proves both the clamp and that the next append reuses
  the gap. `tools/zcfs.py replay()` mirrors the clamp so the host and guest
  cannot disagree about the head on exactly the images that need recovering, and
  the host formatter now plants a torn, over-claiming tail so every boot
  exercises recovery; the boot logs `blk: zcfs recovered`
  (see [ADR 0011](docs/adr/0011-crash-recovery-clamps-log-head.md)).
- **Userspace service supervision** (F7f): `initd` becomes the manifest's
  `init=/sbin/initd`, a ring-3 supervisor that decides whether a service
  domain is restarted or stopped. The kernel keeps only the mechanisms —
  reviving a dead slot, killing a live one — behind `SYS_SERVICE_START`,
  `SYS_SERVICE_STOP`, and `SYS_SERVICE_STATUS` (numbers 23–25), each gated by
  a capability in its own namespace (bit 30, `service_cap`). A pure
  `zc-kernel::service` table names which task slot is which service, so the
  image, the supervisor, and the tests share one source of truth. The kernel
  posts a tagged `FAULT`/`EXIT` event on a fifth IPC channel (`IPC_SUPERVISE`)
  before it removes a supervised slot, so the event is already queued when the
  supervisor wakes. The block domain faults on purpose after its probes
  (`port_inb` on an ungranted port), `initd` restarts it, and it resumes
  filesystem serving from `.bss` state — the BAR base, queue depth, and
  submission counter survive the restart, and it skips the one-shot discovery
  handshake that the now-exited device manager could never answer again. The
  boot shows `initd: restarted blk` and `blk: zcfs serving` after the fault
  (see [ADR 0010](docs/adr/0010-userspace-service-supervision.md)).
- **`SYS_SERVICE_START`, `SYS_SERVICE_STOP`, `SYS_SERVICE_STATUS`** (F7f):
  syscalls 23–25. `TaskTable` gains `is_alive`, `has_runnable_other_than`,
  `respawn`, and `kill`; a supervisor-driven restart reuses the slot's address
  space and image and clears its descriptor table, exactly as a fault restart
  does. The serial-idle condition now checks for a runnable peer rather than a
  live-task count, so a lone shell parks instead of deadlocking once `initd` is
  the only other live task.
- **Writable zcfs in the kernel VFS** (F7e-2): the volume the block domain
  serves is mounted at `/data`, so `SYS_WRITE`, `SYS_CREATE`, `SYS_MOUNT`, and
  `SYS_UMOUNT` reach the disk through the ordinary VFS path. The kernel holds
  only a proxy: it shares one mapped exchange page with the domain and turns
  each `FileSystem` call into a request on the new `IPC_FS` channel, taking the
  reply on `IPC_FS_REPLY` (the IPC table grows from two channels to four).
  Because a trait method cannot block, the proxy returns `WouldBlock` and the
  syscall handler blocks on its behalf, replaying the syscall when the reply
  arrives. A per-task replay log records the calls a syscall has already made,
  so a replay answers them from the log instead of re-sending and consuming the
  reply that is still in flight for the call that blocked. The domain's server
  always publishes its reply length, so a reply without a payload reports zero
  instead of echoing the request's. The shell gains `write`, `persist`,
  `mount`, and `umount`; `persist` writes `/data/probe`, unmounts, remounts (a
  cold-cache replay from the device), reads it back, and logs
  `vfs: persistence ok`. `exit` tells the domain to flush and stop, so the boot
  ends with the volume clean.
- **`SYS_WRITE`, `SYS_MOUNT`, `SYS_UMOUNT`, `SYS_CREATE`** (F7e-2): syscalls 18
  and 20–22. `MountTable::create` splits the path at its last separator,
  resolves the parent, and asks the filesystem to create the entry;
  `FileSystem::create` defaults to `NotSupported`, so the read-only mounts keep
  refusing writes. `VfsError::WouldBlock` carries the block-and-retry contract
  through the trait, and `Stat::read_from` inverts the existing encoder.

- **ZC-native log-structured filesystem** (F7e): `zc-kernel::zcfs` defines the
  format the writable volume uses. Two CRC-32 superblock copies sit at relative
  sectors 0 and 1 and the log starts at sector 2, holding fixed 512-byte
  `CREATE` (parent, mode, name) and `DATA` (offset, bytes) records, each
  CRC-32-checked. A node's id is the sequence number of its `CREATE`, so replay
  needs no allocator state. The write order is the crash rule: append the record
  and flush, then advance `head_seq` in both superblock copies (B, flush, A,
  flush). Replay reads the durable prefix and stops at the first torn or
  mismatched record, so a crash leaks space instead of corrupting. Fifteen host
  tests cover the checksums, torn tails, shadowing, and remounts.
- **zcfs durable write proof** (F7e): the block domain finds the third MBR
  partition (`0x7F`), mounts the volume through the same write-back cache the
  read-only filesystems use, reads the host-planted `/probe`, creates and writes
  `/written`, then discards every cached sector and remounts from the disk to
  read its own file back. `tools/zcfs.py` is an independent host implementation
  of the format, and `tools/check-disk-zcfs.sh` replays the guest's log to
  confirm `/written` (and, once F7e-2's shell has rewritten it, `/probe`), so
  neither side validates its own work
  (see [ADR 0008](docs/adr/0008-zc-native-log-structured-fs.md)).
- **Read-only VFS core** (F7d): `zc-kernel::vfs` defines an object-safe
  `FileSystem` trait whose methods all take `&self`, so one mount is shared by
  every task and the read offset lives in the descriptor. A `MountTable`
  resolves paths against the longest matching mount prefix, compared at
  component granularity, so `/data` never captures `/database`; a
  `DescriptorTable` tracks each task's open files above a reserved descriptor
  base. `zc-kernel::ramfs` implements the trait over the cpio initramfs and the
  kernel mounts it at `/`, replacing the flat `zc-kernel::fs`. The shell gains
  a `stat` command, and the boot log records `vfs: mounted ramfs at /`.
- **`Stat` and `SYS_STAT`** (F7d): file metadata crosses the syscall boundary
  as a fixed 24-byte, little-endian record with a hand-written encoder, so the
  layout is an ABI and needs no pointer casts. `SYS_STAT` is number 19; 18
  stays reserved for the `SYS_WRITE` that arrives with F7e-2.
- **Read-only ext2** (F7c-2): `zc-kernel::ext2` mounts a real ext2 volume —
  superblock validation, the group descriptor, inode locations, directory
  entries with `rec_len` walking, and direct/single/double/triple indirect
  block maps with sparse holes, all host-tested. The block domain mounts the
  second MBR partition through the same write-back cache and reads
  `EXT2.TXT`, so two filesystems share one cache seam.
  `tools/check-disk-ext2.sh` reads the same file with `debugfs`, independently
  of the driver. Blocks are 1024 bytes only.
- **Read-only FAT32** (F7c): `zc-kernel::mbr` parses the partition table and
  `zc-kernel::fat32` mounts the volume — BPB validation, cluster math, the FAT
  chain, 8.3 root-directory lookup, and file reads, all host-tested. The block
  domain parses the MBR, mounts the partition, and reads `HELLO.TXT` through
  the write-back cache, so a real filesystem consumes the cache seam.
  `tools/check-disk-fs.sh` reads the same file with mtools, independently of
  the driver.
- **Write-back block cache** (F7b): a pure, host-tested `Cache<N>` in
  `zc-kernel::block_cache` owns the sector buffers and tracks dirty slots. The
  block domain holds a four-slot cache in `.bss` and reads/writes through it;
  dirty data is written back before a slot is reused, and flush writes back
  what remains before the device flush. The boot log proves a miss, a hit, a
  dirty hit, an eviction write-back, a flush write-back, and a durable
  read-back that bypasses the cache.
- virtio-blk **write path** (F7a): the block domain negotiates
  `VIRTIO_BLK_F_FLUSH`, writes a known pattern to a data sector, flushes the
  device cache, then reads the sector back and compares every byte. Reads,
  writes, and flushes share one descriptor-chain helper.
- `tools/check-disk-write.sh`: a host-side check that the raw write and both
  cache write-backs reached `build/disk.img`, proving durability and not just
  the DMA buffers.
- `SYS_PORT_CLAIM` (F6i): a driver claims exactly the `(start, len)` port
  range its capability table holds. Ports are packed with a high bit so they
  never collide with IRQ indices, and the kernel records and projects the
  grant immediately — a domain's next instruction is usually a port access.
- `zc-kernel::device` (F6j): a host-tested device grant table. Every grant
  (keyboard IRQ plus two 8042 ranges; PCI config plus the discovered BAR
  window) is a pure constructor per role, with the exact values and the
  IRQ/port namespace split pinned by tests.
- `user/zc-devmgr` (F6k): a seventh ring-3 domain that exclusively owns PCI
  config, scans bus zero, enables bus mastering, and publishes the winning BAR
  base to the block driver.
- `IPC_DISCOVERY` plus `SYS_SEND_TO`/`SYS_RECV_FROM` (F6l): an explicit-channel
  IPC fabric. The data word stream and manager-to-driver discovery use
  independent queues that provably never share traffic.
- `SYS_CAP_DELEGATE` (F6m): runtime capability delegation. A grant-holding
  domain delegates a non-amplifying rights subset into another task's table,
  refusing unknown bits, empty rights, self-delegation, and out-of-range
  targets. The block driver's BAR window arrives this way.
- This changelog, following Keep a Changelog, with the project history
  backfilled.
- `CONTRIBUTING.md`: build and test commands, Conventional Commits, the
  changelog policy, and the branch/PR flow.
- `docs/adr/`: Architecture Decision Records, with an index, a template, and
  the first six decisions.
- `docs/blocked/`: a blocker registry, with the OVMF SMP entry.
- A CI `changelog` job that requires an `[Unreleased]` section.
- **Port-broker capability** (Track K, M2): `PORT_BROKER_OBJECT`
  (`0x1000_0001`, its own namespace) carries `GRANT` over the PCI I/O window
  `0x1000..=0xFFFF` and nothing else, so a device manager can hand a driver the
  BAR it discovered but cannot claim or touch a port itself. A `syscall4` stub
  passes a fourth argument in `r10`, letting `SYS_CAP_DELEGATE` name the
  discovered `(start, len)` range as a raw word — a packed port capability
  cannot be decoded back into a range — which the kernel validates against the
  window before minting the driver's capability.

### Changed

- **PCI enumeration moved to ring 3** (Track K, M2): the kernel no longer scans
  the bus. `user/zc-devmgr` owns the config window, discovers the transitional
  virtio-blk BAR, and brokers it to the block driver through the new broker
  capability, so the kernel's boot path carries no device knowledge, the
  `pci:` diagnostic lines disappear, and `kernel/zc-kernel-image/src/pci.rs`
  plus its `serial::outl`/`inl` helpers are deleted. `zc-kernel::device` gains
  `pci_io_broker_grant` and `pci_io_window_contains` in place of `bar_range`.
  The frame hash is unchanged at `0x5b8ba1ab75967a51`.
- **Storage parsers moved out of the kernel crate** (Track K, M1): `zcfs`,
  ext2, FAT32, the MBR table, the write-back block cache, and the virtio
  register layout now live in the new shared `zc-storage` crate. Only the block
  domain links them, so ring 0 no longer carries any filesystem code; `zc-kernel`
  keeps the mechanisms. `zc-kernel`'s test count drops from 268 to 200 and
  `zc-storage` runs the 68 moved tests. No behaviour change: the boot log and
  the frame hash are byte-identical.
- The frame allocator is now global and reserves the virtual windows user
  address spaces remap (`FrameAllocator::reserve`). The surface syscalls
  allocate frames while a task is running, and the kernel reaches a fresh
  frame through the identity map — but a task's page tables point those
  addresses at its images, stack, display, or a surface, so a frame in one of
  those windows would be written into user memory. `PhysFrame::from_address`
  lets a caller holding only an address (a surface's recorded frames) return a
  frame to the allocator.
- The loader reads `PixelsPerScanLine` from the firmware graphics mode instead
  of assuming the pitch equals the visible width, and the framebuffer mapping
  covers `stride * height` pixels, so a stride-padded mode maps correctly on
  real hardware.
- File syscalls now go through the VFS instead of the flat initramfs module:
  `zc-kernel::fs` is replaced by `zc-kernel::ramfs` (a `FileSystem`) plus
  `zc-kernel::vfs` (mount table and descriptors). `open` resolves against the
  mount table, `stat` is available, and the shell's `help` line lists it.
  Relative paths still resolve from the root, so existing scripts are
  unaffected.
- `build/disk.img` is now a 128 MiB MBR disk carrying two real filesystems:
  a FAT32 volume in the first partition (LBA 2048) and an ext2 volume in the
  second (LBA 83968), replacing the 1 MiB marker disk. The sector-zero
  `ZCDISK01` magic and the write/cache test sectors are unchanged, so the
  earlier proofs still run.
- `docs/roadmap.md` was restructured around the skill's phases **F0–F9**, each
  with a goal, tasks, and machine-runnable pass criteria, plus detailed plans
  for F7 (VFS & storage), F8 (desktop), and F9 (distribution & daily driver).
  The old `Milestone N` labels are kept only in a mapping table.

- Port and IRQ authority no longer flows from the kernel at boot: `main.rs`
  only reports the PCI hardware, and all rights originate from spawn-time
  capability tables through explicit claims. The block driver holds no port
  until the device manager delegates its window at runtime.
- The keyboard domain's boot log now proves the gates: an unprovided IRQ
  source and an unprovided port range are both refused before any state
  changes.
- The block driver now reads the device feature word and acknowledges only
  `VIRTIO_BLK_F_FLUSH` instead of writing zero, so the flush request it sends
  is one the device actually offered. Devices without the feature log
  `blk: flush unsupported` and continue.
- The headless boot harness (`tools/run-qemu.sh --test`) now drives the shell
  through `tools/boot-feed.py`, which launches QEMU with its serial on stdio,
  copies the transcript to stdout, and paces the scripted commands. The old
  whole-copy FIFO drip tore lines once the F7j script outgrew the input ring:
  QEMU only forwards stdin while the chardev is actively written, and a burst
  larger than the guest's receive buffers is dropped mid-line. The feeder keeps
  one command body in flight, watches the transcript for a full pass, and only
  then appends `exit`, so the shell always reads a complete, in-order script.
  The kernel's shared input ring (`INPUT_CAP`) rises from 256 to 1024 bytes so
  a body plus the terminator cannot overflow it.
- The project now commits to reusing Linux drivers in a userspace **driver
  domain** instead of hand-writing one per device
  ([ADR 0014](docs/adr/0014-linux-driver-domain.md)): DDE first, then a full
  Linux kernel as a driver container under virtualization for drivers that need
  ring 0. A new [`docs/carry-over.md`](docs/carry-over.md) records what from the
  earlier prototype is worth reusing (host emulator, compositor, GUI toolkit,
  `.sof` packaging) and what to leave behind (hand-written and ring-0 drivers).

### Fixed

- **Kernel interrupt stacks were too small for the syscall frame** (Track K,
  M2): `user_syscall`'s widest arms materialise two 8 KiB `Surface` values
  (`frames: [u64; 1024]`, `Copy`) for a frame just over 16 KiB, but every IDT
  gate ran on a 16 KiB `IST1_STACK`. The overflow was latent until M2's `.bss`
  reshuffle put `NEXT_CR3`/`FRAMES` in its shadow, at which point every syscall
  corrupted the next task's page table and the boot wedged. Both interrupt
  stacks are now 64 KiB.

- **Mouse never moved because the input domain only waited on the keyboard
  interrupt** (F8b): the driver domain's `serve()` loop called
  `irq_wait(IRQ_KEYBOARD)`, so a mouse-only interrupt flowed through the real
  path (vector → handler → EOI → `irq_post`) and recorded a count, but the
  scheduler's `any_pending()` wake simply re-entered the keyboard wait, which
  re-blocked without draining the 8042. The mouse bytes sat in the controller
  forever and never reached the ring, the kernel drain, or the compositor — the
  pointer froze on real hardware even though the cursor plumbing was correct. The
  fix adds `IRQ_ANY` (`u64::MAX`) to `SYS_IRQ_WAIT`: passing it makes the kernel
  wake on whichever owned source fired and clear every owned count at once, so
  the input domain now waits on *any* of its claimed lines and drains the 8042
  wholesale on every wake. The per-line `irq_wait(source)` semantics the
  bring-up proofs rely on are unchanged. See
  [ADR 0023](docs/adr/0023-pointer-and-mouse-read.md).

- **Live pointer flew across the screen and overshot on tiny moves** (F8b):
  `SYS_MOUSE_READ` delivered the accumulated movement but never cleared
  `MOUSE_PENDING`, so every later report *added* to the last delivered delta
  instead of replacing it. The value pinned at the clamp (±127) and a stale
  direction kept replaying — even a purely vertical move re-fired the old
  horizontal jump. The read now resets the accumulator to zero after consuming
  it, and button state tracks the latest packet instead of OR-ing across frames,
  so one physical move maps to one on-screen move. See
  [ADR 0023](docs/adr/0023-pointer-and-mouse-read.md).

- **Terminal hung and stopped accepting keystrokes while the pointer was over
  the window** (F8b): the kernel posted a `WM_MOUSE` nudge on the same 4-slot
  `IPC_WM_REPLY` channel the client uses for `WM_ACK`, and `SYS_SEND_TO` blocks
  when the channel is full. A fast pointer kept the channel full of `WM_MOUSE`
  messages, so the terminal's `WM_ACK` blocked forever waiting for a free slot —
  deadlocking keystroke input. `WM_MOUSE` is now coalesced: it is only posted
  when the channel is empty, because the compositor already drains every pending
  mouse report on each wake, so one queued nudge schedules a full drain and a
  second one only competes with the client's acks. See
  [ADR 0023](docs/adr/0023-pointer-and-mouse-read.md).

- **Pointer vanished the moment it was aimed at the terminal window** (F8b-2):
  the compositor's event loop re-composited the window over the `moved` region
  (`blit_window` + `blit_rect`) on every wake — keystroke *or* pointer nudge —
  but never redrew the cursor on top of that region afterward. The pointer is
  foreground, so `blit_window` overwrote the sprite wherever it overlapped the
  window, and the next flush sent a cursor-less window to the display. The
  pointer therefore disappeared (and looked frozen) exactly when it sat over the
  terminal, which also made typed input look dead because the user's on-screen
  reference point was gone. The event loop now redraws the cursor over the window
  region before flushing, so the pointer stays visible over the window and moves
  normally. Keyboard input was never actually blocked: `SYS_TERM_READ` serves the
  routed keystrokes and the client runs them (verified end-to-end through the
  scripted session), so the fix is compositor-side only. See
  routed keystrokes and the client runs them (verified end-to-end through the
  scripted session), so the fix is compositor-side only. See
  [ADR 0023](docs/adr/0023-pointer-and-mouse-read.md).

- **The boot watchdog killed every interactive session about five seconds
  in** (F8b-2): the user-phase watchdog (`USER_TIMEOUT_TICKS = 5000`) exists so
  a wedged task fails the headless boot test instead of hanging it, but it
  compared the tick count against the deadline with no notion of an interactive
  session. Any human at the machine — moving the pointer, clicking, typing —
  keeps the tasks alive past 5000 ticks, and the kernel then reported
  `user task timed out` and halted, which looked like a random freeze (often
  right on a click, because by then the budget was spent). The first byte of
  live input from an input domain now disarms the watchdog by pushing the
  deadline out of reach. The headless boot test never delivers physical input —
  the scripted session is served kernel-side — so there the watchdog stays
  armed and a wedged task still fails the boot fast.

## [0.1.0] - 2026-10-01

First tracked state of the project: the full path from firmware to restartable
userspace drivers is in place (roadmap phases F0–F6h).

### Added

**Foundation (F0)**

- Cargo workspace with dual MIT / Apache-2.0 licensing, a versioned
  loader-to-kernel ABI crate (`libs/zc-abi`), a shared ELF parser
  (`libs/zc-elf`), and host-tool validation (`tools/verify-host.sh`).
- CI (`Rust` workflow) building and unit-testing the host workspace, plus a
  headless QEMU/OVMF boot test that greps the serial log for every milestone
  proof.
- Architecture document (`docs/architecture.md`) covering trust boundaries,
  the boot protocol, and the address-space / authority model.

**Bootloader (F1)**

- A UEFI application loader (`boot/uefi-loader`) that disables the firmware
  watchdog, discovers the GOP framebuffer, captures the memory map, locates
  the ACPI RSDP, and loads a kernel ELF from its own FAT volume.
- Exit-boot-services hand-off: the loader builds identity and higher-half page
  tables, installs a 64-bit GDT, and jumps to the kernel entry with a
  `BootInfo` pointer. The kernel rejects a wrong sentinel or version.
- An initramfs (newc cpio) delivered through `BootInfo`, parsed by the kernel
  with an allocation-free walker.

**Kernel mechanisms and memory (F2)**

- Boot-contract validation, a physical frame allocator with frame recycling,
  virtual-memory helpers, a bounded round-robin scheduler, bounded IPC
  endpoints, a userspace address-space tracker, syscall dispatch, and
  generation-safe capability tables — all covered by host unit tests.

**Interrupts, timer, and SMP (F3)**

- A 256-gate IDT with naked-assembly stubs running on IST1, a local APIC, HPET
  calibration of the APIC bus (~1 GHz), and 1 ms periodic ticks.
- ACPI RSDP/XSDT/MADT parsing with checksums, reporting CPUs and the I/O APIC.
- SMP bring-up (INIT-SIPI-SIPI, sub-megabyte trampoline, per-AP stacks) is
  implemented but dormant; see
  [`docs/blocked/smp-ovmf.md`](docs/blocked/smp-ovmf.md).

**Threads and IPC (F4)**

- A GDT/TSS with ring-3 segments, an `iretq` entry into ring 3, and preemptive
  round-robin scheduling of full register state across a bounded task table.
- Blocking IPC between live tasks with transparent full/empty blocking, a
  deadlock fail-stop, and a timeout watchdog.
- Capability tables provisioned at spawn; every IRQ claim consults the
  caller's rights before touching the IRQ table.

**Userspace (F5)**

- Freestanding ET_EXEC userspace binaries at distinct link bases
  (`user/zc-user` plus `zc-producer`, `zc-consumer`, `zc-shell`), loaded by the
  kernel with user permissions and packed into the initramfs.
- `SYS_LOG_WRITE` task logging, and `SYS_OPEN`/`SYS_READ`/`SYS_CLOSE` serving a
  read-only initramfs filesystem with path validation and descriptor tables.
- An interactive serial shell running `help`, `echo`, `cat`, and `exit` with
  line editing, scripted through a drip-fed FIFO in CI.

**Driver userspace (F6a–F6h)**

- PCI enumeration through type-1 configuration space and a userspace virtio-blk
  domain (`user/zc-blk`) driving the transitional PIO transport from ring 3.
- An 8 KiB deny-by-default TSS I/O permission bitmap replacing blanket `IOPL`,
  plus per-task port policy projected onto the single TSS bitmap on every
  context switch.
- Per-task address spaces: a private PML4/PDPT/PD per task with `CR3` reloaded
  on every switch, and a boot self-check proving no task maps another's pages.
- IRQ-to-IPC delivery: the kernel handler only counts and EOIs, while the
  domain that claimed the source blocks in `irq_wait` and drains the device.
- Fault isolation: a ring-3 CPU exception kills only its domain, revokes its
  IRQ claims, port grants, and shared ring page, and the scheduler continues
  into the next task. A ring-0 fault remains fatal by design.
- Automatic driver restart: a faulted slot with restart budget respawns in
  place with fresh registers and re-claims its device; the budget bounds an
  unconditional fault loop.
- A userspace framebuffer domain painting eight color bars verified by a
  full-frame checksum, and a keyboard domain draining the 8042 itself and
  publishing ASCII through a shared ring page.

### Changed

- The kernel-side virtio-blk driver was deleted in favor of the userspace
  domain.
- The kernel no longer grants any I/O port at boot.

[Unreleased]: https://github.com/zc-os/zcos/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/zc-os/zcos/releases/tag/v0.1.0
