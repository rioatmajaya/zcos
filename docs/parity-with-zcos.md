# Parity map: ZC OS and the ZCOS prototype

Status: working reference, revised 2026-10-24 Owner: ZC OS maintainers Scope: current public tree (`/home/rhiiyu/Desktop/Dataku/OS_Build/zcos`) against the mature reference prototype (`/home/rhiiyu/Desktop/Dataku/OS_Build/zcos-original`)

This document is a map, not a promise that the two trees should become the same codebase. Its job is to answer three questions:

1. What does the reference actually do?
2. What does the current tree do today?
3. Which gaps are worth closing first, and how do we know a change did not weaken the current proof model?

The reference tree is **read-only**. Nothing in this document authorises a change, move, or deletion under `OS_Build/zcos-original`.

The work order in section 6 is **core-first**. A simple desktop is allowed to exist and stay useful, but it does not lead the project; applications and the host runner come later.

## 1. Constraints

### 1.1 Separate implementations, separate licences

The current tree is public and dual-licensed MIT / Apache-2.0. The reference tree has no top-level licence file and is treated as closed-source. Copying its source, or a mechanically translated version of it, into this repository would publish code that is not licensed for that use.

Therefore:

- The reference is a **behavioural and architectural reference**, not a source of copied code.
- Reimplement a capability against the current ABI, or write an adapter. Do not copy modules, constants, assets, or comments.
- If direct code reuse is ever wanted, the repository must first become private or the reference's licensing must be clarified. That is a project decision, not an implementation detail.
- The reference's names (Prism, ZDP, SOF, ZCFS, `libzgui`, and so on) may be used to describe behaviour, but their implementations stay out of this tree.

### 1.2 Preserve the current proof surface

The current tree has three locked proof images and checksums:

| Image | Purpose | Locked checksum |
| --- | --- | --- |
| `build/zcos-test.img` | single-window scripted proof | `0x18f9ce724a929efc` |
| `build/zcos-close.img` | close/erase proof | `0x6d66e82d0f2d6fb8` |
| `build/zcos-fm.img` | launcher, navigation, preview proof | `0xf89e1ca133f3e8dc` |

The test and fm checksums moved twice, deliberately: when the strip's second widget (the reset target) joined the frame (ADR 0088) and when the focused widget began wearing the ring (ADR 0089). The close checksum did not move at either step: that run erases the window before the final frame, so the frame is desktop and cursor only — the closed window's content is proven by the per-window content check (`wm: reset ok`, `wm: focus ok`), not by the final frame.

`build/zcos.img` is the human-testable image and has no golden checksum.

Parity work must either:

- keep those three checksums unchanged, or
- re-baseline one deliberately, with an ADR that explains why the frame is expected to change and what still verifies it.

The default image and the proof images must stay separate. A feature that makes the human image better must not silently change a proof image.

### 1.3 Definitions

- **Parity** means a user can perform the same task with comparable behaviour and reliability. It does not mean identical pixels, identical ABI, or a port of the reference implementation.
- **Proof path** means the scripted `--test`, `--test-close`, and `--fm` boots, where the kernel independently recomputes the expected frame.
- **Product path** means the default image a person boots: live input, live clock, no scripted session, no kernel frame checksum.
- **Reference** means `new-os/zcos`. **Current** means this repository.

## 2. Snapshot

| Item | Reference ZCOS | Current ZC OS |
| --- | --- | --- |
| Role | mature prototype, behavioural target | public rewrite, working project |
| Version / latest entry | crates mostly `0.2.2`; changelog through 2026-10-21 | `0.1.0`; changelog through 2026-10-22 |
| History | no Git metadata in the tree | 90 commits, initial commit 2026-10-01, HEAD `fde2509` |
| Size / files | about 11 GB, 38,953 files, including `target/` | about 525 MB, 4,774 files, including `target/` |
| Boot | Limine ISO (`zcos.iso`); QEMU with SMP; host emulator | custom UEFI loader; QEMU + OVMF; no host emulator |
| CPU | SMP enabled, QEMU tested at 4 CPUs | SMP code present but parked; single CPU in practice |
| Build | `./build.sh` → `zcos.iso` | `./tools/build-efi.sh` → `build/zcos.img` |
| Hardware | user reports a successful USB boot on a laptop (user report, 2026-10-22) | QEMU/OVMF only; no hardware validation yet |
| Licence | no top-level licence file; treated as closed | MIT / Apache-2.0 |
| Verification | 91 scripts under `scripts/`, host tests, QEMU tests, host emulator | 15 tools, CI host + boot jobs, 48 ADRs, 5 interface specs |

Method: static inspection of both trees on 2026-10-22. The reference was not rebuilt or booted for this map; its hardware claim is the user's report. Size and file counts include generated artifacts (`target/`), so they indicate scale, not source size.

The reference's source wins over its documentation when they disagree. The current roadmap wins over current prose when they disagree. The reference's desktop guide, for example, still lists ten dock entries including two Linux apps, while `userspace/services/winmgr/src/desktop.rs` defines nine entries and the 2026-10-20 changelog records the Linux entries being removed.

## 3. Status key

| Mark | Meaning |
| --- | --- |
| ✅ | current behaviour is equivalent enough for the task |
| 🟡 | present but materially narrower, or implemented on a different boundary |
| ❌ | absent |
| ⛔ | deliberately not to be copied; build an equivalent on the current boundary |

"Equivalent enough" is about the user-visible task, not implementation identity.

## 4. Layer-by-layer matrix

### 4.1 Boot, platform, and lifecycle

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| Boot path | Limine ISO, `boot/limine.conf`, three entries for full/SMP-failsafe/bootstrap debugging | custom UEFI loader, versioned `BootInfo`, direct kernel jump | 🟡 Different by design. Keep the current loader and its contract; do not add Limine just for parity. |
| Firmware target | BIOS/UEFI through Limine; hardware USB | UEFI only, QEMU/OVMF | ❌ Hardware boot is unproven in the current tree. Treat it as a separate hardware-validation work item, not a side effect. |
| SMP | AP bring-up, per-CPU areas, priority scheduler, QEMU tested at 4 CPUs | trampoline and per-AP code exist, but SMP is parked because OVMF fails during `ExitBootServices` with two CPUs | ❌ A real desktop needs SMP eventually. The current blocker is documented in `docs/blocked/smp-ovmf.md`; solve it on a target that can boot two CPUs. |
| ACPI and power | RSDP/MADT, AML, battery, thermal/power paths, S5 | RSDP/MADT and a minimal AML `_S5_` reader; clean `SYS_POWEROFF` | 🟡 Power-off is at parity for the tested path; battery, thermal, suspend, and idle states are absent. |
| Installer / updater | `zinstall`, `zupdate`, signed `.zup`, A/B slots | none | ❌ Belongs to the distribution phase, after storage and signing exist. |
| Host emulator | `tools/zcos-emu` runs the real compositor and GUI apps on Linux | none | ❌ Useful later as a developer accelerator, not a core prerequisite. Add an independent host runner when GUI iteration needs it; do not treat it as an isolation test. |
| Boot diagnostics | serial logs, screenshots, hardware probes, diagnostics overlay | serial logs, QEMU monitor input, proof checks, and `dmesg` over the kernel ring | 🟡 Current proofs are strong and the kernel log is readable from the shell; user-facing diagnostics are still thin. |

### 4.2 Kernel, process model, and authority

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| Memory and address spaces | 4-level paging, buddy allocator, per-process address spaces | 4-level paging, frame recycling, per-task `CR3`, user-window mapping | ✅ Core mechanisms are present. |
| Scheduling | 256 priority queues, affinity-aware, SMP load balancing | bounded round-robin over a fixed task table, single CPU | 🟡 No priorities, no SMP scheduler, no per-CPU run queues. |
| IPC | synchronous endpoints, shared memory, notifications, poll | channels, blocking send/receive, shared exchange page, `SYS_SEND_TO`/`SYS_RECV_FROM`, per-task event word with `SYS_NOTIFY`/`SYS_EVENT_WAIT` | 🟡 Enough for current services; no general shared-memory API or endpoint object, and notifications are a coalescing flag word rather than a queued endpoint object. |
| Capabilities | `libcap` rights over process/thread/endpoint/shm/IRQ/I/O/block; SOF manifests; signed policy | generation-safe tables, explicit rights, delegation, per-task I/O bitmap, port/IRQ claims | 🟡 Current delegation is strong, but there is no signed object or policy layer. |
| Process model | `fork`/`exec`/`spawn`/`wait`, signals, process registry, sessions | fixed 10 task slots; spawn by image name (`SYS_SPAWN`), kill a runtime spawn with a status (`SYS_KILL`), block-and-reap by slot (`SYS_WAIT`, returning the spawn's frames), per-slot generations and exit status (`SYS_TASK_INFO`), `SYS_TASK_EXIT`, service revive, per-task event notifications within a session (`SYS_NOTIFY`/`SYS_EVENT_WAIT`), signals with a handler (`SYS_SIGNAL`/`SYS_RAISE`/`SYS_SIGRETURN`); no fork/exec | 🟡 A ZC-native process model now creates, kills, waits, reaps, and reclaims tasks, and inherited identity and session make the registry meaningful; `fork`/`exec` is absent, signals are one handler per task with no masks, and the table stays bounded. |
| Syscall surface | about 70+ calls, including process, signal, network, library, Linux compatibility | 60 numbers, highest `SYS_RAISE` (59), one invalid-number probe | 🟡 The current ABI is coherent; it is simply much smaller. Add calls only with specs and tests. |
| Fault isolation | ring-3 faults kill only the domain; init supervises services | same principle; `initd` supervises `blk` and `kbd` | 🟡 The mechanism is at parity; the service catalog is much smaller. |
| Linux personality | kernel-level POSIX translator for static x86\_64 ELF (`busybox`, `tcc`), sockets `ENOSYS` | none | ⛔ Do not copy the kernel placement. If needed later, build a userspace personality domain. |
| Linux driver strategy | many hand-written drivers, some in ring 0 | ring-3 driver domains with brokered authority | ✅ Keep the current boundary. Do not regress to ring-0 drivers for feature speed. |

### 4.3 Storage and VFS

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| Native filesystem | ZCFS, journaled, persistent `/user` | zcfs, log-structured records (create, data, unlink, rmdir, rename, owner), dual superblock, crash recovery, fsck, on-demand `sync`, host checker replaying every kind | ✅ Different format, comparable durability goal. Current has strong crash tests. |
| In-memory / pseudo filesystems | RAMFS, ISO9660 | ramfs, tmpfs, devfs | 🟡 Current has a better writable RAM proof and device namespace; no ISO9660. |
| Foreign filesystems | ZCFS / RAMFS / ISO9660 are the documented set | FAT32 read-only, ext2 read-only, zcfs read-write | 🟡 Current has more read-only formats but no ISO9660; both are narrower than a daily-driver rootfs. |
| Block layer | AHCI, ATA, NVMe, virtio-blk; block service | virtio-blk only; four-slot write-back cache | 🟡 Only one transport is proven. AHCI/NVMe are required for hardware parity. |
| VFS placement | userspace `vfs` service; kernel keeps mechanisms | kernel VFS + `FileSystem` trait; storage parsers in `zc-storage` | 🟡 A deliberate difference. Keep it until a service split is justified by a measured need. |
| Permissions | authentication/session plus service policy | uid/gid, POSIX mode classes, root bypass, per-descriptor cached stat; `SYS_GETID` reads the spawn-time identity and `SYS_SESSION` the spawn-time session | 🟡 File permissions and the session grouping are present; owners and sessions are runtime-only and authentication is absent. |
| Installer / update slots | `zinstall`, signed updates, A/B slot promotion | none | ❌ Later phase. |

### 4.4 Drivers and hardware

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| Driver count / placement | 7 ring-3 `.dri` daemons (battery, GPU, HDA, NVMe, touchpad, Wi-Fi, xHCI) plus many ring-0 drivers | 3 ring-3 domains: virtio-blk, PS/2 keyboard/mouse, PCI device manager | 🟡 Current boundary is cleaner; device coverage is far smaller. |
| Storage hardware | AHCI/SATA, NVMe, ATA, virtio-blk | virtio-blk | ❌ No real laptop storage path yet. |
| Input hardware | PS/2 fallback, I2C-HID touchpad, xHCI USB HID | PS/2 keyboard and mouse only | ❌ No touchpad, USB HID, hotplug, or gestures. |
| Display / GPU | software compositor plus virtio-gpu / iGPU work | software framebuffer, software composition | 🟡 No acceleration; acceptable for 1.0 only if the UI remains responsive. |
| Audio | HDA driver, `audiod`, volume/mute paths | none | ❌ Planned as F8i. |
| Battery / thermal | ACPI battery driver, `batmgr`, power UI | none | ❌ Planned with the power phase. |
| Networking hardware | e1000, r8169, virtio-net, RTL8822CE Wi-Fi | none | ❌ See networking below. |
| Authority / DMA | capabilities, MMIO brokerage, shared rings; some ring-0 drivers | deny-by-default I/O bitmap, port/IRQ claims, BAR/MMIO brokerage, coherent DMA, restartable domains | ✅ Keep and extend the current model. This is a current strength. |

### 4.5 Networking, security, and distribution

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| IP stack | `netd`: TCP/IP, DHCP, DNS, sockets | none | ❌ No network syscalls or service. |
| NIC support | e1000, r8169, virtio-net, Wi-Fi | none | ❌ |
| TLS / PKI | TLS 1.2/1.3, X.509 chain validation, CA bundle, crypto library | none | ❌ |
| Browser | `zcbrow`: HTML, CSS layout, JavaScript subset, cookies/cache/HSTS, real HTTPS | none | ❌ Very large. Treat as later product work, not an F8 task. |
| SSH | client and test server | none | ❌ Optional after IP/TLS. |
| Authentication | `authd`, `zlogin`, greeter, session control | none; `initd` is the only root task. Sessions exist as spawn-time grouping (`SYS_SESSION`) but there is no credential check | ❌ No login boundary. |
| Signed objects / policy | SOF v2 signatures, manifests, signed policy, development key | unsigned ELF tasks; capability grants are the only authority | ❌ No package or binary signature chain. |
| Update / install | signed `.zup`, A/B slots, `zupdate`, `zinstall` | none | ❌ |
| Linux subsystem | kernel POSIX translator, static busybox/tcc | none | ⛔ Revisit only as a userspace personality, and only after the native app model works. |

### 4.6 System services

The reference defines 19 well-known endpoints and has 17 service crates:

`init`, `devmgr`, `vfs`, `netd`, `logd`, `shell`, `procmgr`, `authd`, `winmgr`, `clipboardd`, `audiod`, `batmgr`, `nvmed`, `xhcid`, `wifid`, `virtiogpud`, `touchpadd`, `loaderd`, `driver`.

The current tree has:

- kernel-owned VFS, device, input, and display mechanisms;
- ring-3 domains for virtual block, PS/2 input, PCI discovery, the compositor, the shell, and `initd`;
- exactly two supervised services (`blk`, `kbd`) in the fixed task table, named once in `zc_abi::SERVICE_CATALOG` and applied by the data-driven `initd`.

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| Service catalog | 19 endpoints, driver catalog, loader sandbox | 2 supervised services in one ABI catalog with boot order | ❌ Most service roles are absent, and the catalog is compile-time only. |
| Discovery / loading | service discovery by name, driver match rules, dynamic spawn | runtime task creation by initramfs name (`SYS_SPAWN`), fixed boot indices, `SYS_SERVICE_START`/`STOP`; `SYS_SERVICE_HEALTH` reports restart and fault counters; a service publishes the channel it serves on (`SYS_SERVICE_PUBLISH`) and any task resolves a name to it (`SYS_SERVICE_LOOKUP`) | 🟡 Runtime spawn and runtime name-to-endpoint discovery exist ([ADR 0084](adr/0084-a-service-publishes-its-endpoint-at-runtime.md)), but services are still a compile-time catalog with no manifest-driven loader, driver match, or packaging. |
| Logging | `logd` | kernel diagnostic ring readable through `SYS_LOG_READ` and the shell's `dmesg`; task logs remain serial-only | 🟡 The kernel half is user-visible and bounded; there is still no persistent or task-facing log service. |
| Clipboard | `clipboardd` | none | ❌ |
| Process/session manager | `procmgr`, sessions, job control | fixed slots | ❌ |
| Authentication | `authd`, greeter | none | ❌ |
| Audio/battery/network/USB/GPU/touchpad services | present as services + drivers | none | ❌ |

### 4.7 Desktop, compositor, and UI toolkit

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| Compositor | Prism (`winmgr`) with dynamic surfaces, damage tracking, ZDP events, themes | `zcompositor` with full-screen back buffer, damage tracking, delegated surfaces | 🟡 Current is smaller but has a strong damage proof. |
| Window manager | dynamic list; drag, edge resize, maximize/restore, minimize, close, layering; one global focus | fixed table of 4; 2 client tasks; drag, minimize/restore, close; no resize or maximize | 🟡 Table exists, but capacity and operations are below the reference. |
| Top bar / dock / launcher | floating top bar with app pills, status cluster, control center, power menu; 9-entry bottom dock; hover magnification; running indicators | 32 px taskbar with `ZC` launcher, up to 4 task buttons, clock | ❌ The launcher is a cycle/launch button, not an app catalog. |
| App catalog | 9 dock entries: Editor, Terminal, Files, Monitor, Viewer, Calculator, Settings, Audio, Browser | 2 binaries: terminal and file manager | ❌ Adding apps is a kernel/task-table change today, not a catalog operation. |
| App framework | `AppHandler` lifecycle (`on_init`, `on_update`, `on_render`, `on_event`, `on_close`), window surface, event loop | each client hand-writes its own loop; no shared app runtime | ❌ |
| Widget toolkit / layout | buttons, labels, text input, progress, switch, panel, tabs, list, scroll, slider, modal, flex layout, theme | exactly one widget: `Clicks: N`; no layout pass | ❌ This is the main reason the current UI feels like a demo. |
| Theming | four themes, wallpaper, animations, toasts, OSD, control center | fixed palette, no theme switching | ❌ |
| Login / lock | greeter, `zlogin` | none | ❌ |
| Keyboard shortcuts | Win, Alt+shortcuts, F-keys, Ctrl+Alt+Del, diagnostics overlay | none beyond typing into the focused window | ❌ |
| Proof vs product | live desktop; QEMU tests use screenshots and serial assertions | kernel frame verifier is woven into the product path; default image is live but still fixed-slot | 🟡 The proof model is a strength, but it is now a bottleneck. Split proof-only checks from the live desktop before adding dynamic UI. |
| Host iteration | `zcos-emu` with minifb | none | ❌ UI iteration requires a full QEMU boot. |

### 4.8 Applications

| App | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| Terminal | `zterm`: tabs/panes, scrollback, ANSI, selection/copy, search, shell backend | single window terminal; serial shell with `help echo cat ls stat write tmp persist chmod mount umount poweroff exit` | 🟡 Basic terminal works; no tabs, panes, search, selection, or rich shell integration. |
| Files | `zfile`: dual-pane, tabs, address bar, search, sort, icon/details views, mkdir/rename/delete/copy/move/trash | single list, navigate, select, preview text | 🟡 Navigation and preview are proven; file operations and dual-pane workflow are absent. |
| Editor | `zedit`: cursor, selection, line numbers, syntax colouring, save/open, scrolling | none | ❌ |
| Viewer | `zviewer`: raster image viewer with loaders | none | ❌ |
| Calculator | `zcalc`: expression evaluator, keyboard/mouse UI | none | ❌ |
| Settings | `zsettings`: themes, display, system preferences | none | ❌ |
| Monitor | `zsysmon`: CPU/RAM gauges, process table, hardware profile | none | ❌ |
| Browser | `zcbrow`: DOM/CSS/JS subset, real HTTPS, store | none | ❌ |
| Audio player | `zlisten` | none | ❌ |
| Shell | rich shell: pipelines, redirection, jobs, history, completion, scripts, plugins, many builtins | serial + window terminal with a small command set | 🟡 The serial shell is useful; parity is far away. |
| Login/install/update | `zlogin`, `zinstall`, `zupdate` | none | ❌ |

### 4.9 Input

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| PS/2 keyboard/mouse | kernel fallback plus ring-3 input service | ring-3 keyboard/mouse domain, shared ring, APIC IRQ | ✅ Core path works, including live clicks after F8h-3. |
| Modifiers and shortcuts | full modifier routing and desktop shortcuts | Shift state is tracked for ASCII; no desktop shortcut layer | 🟡 |
| Window-local pointer | ZDP events with window coordinates | `SYS_WINDOW_READ` derives window-local reports; compositor keeps the global pointer | ✅ Current model is coherent and proven. |
| Touchpad | I2C-HID driver, relative motion, tap/drag/two-finger gestures | none | ❌ |
| USB HID | xHCI driver and service | none | ❌ |
| Input injection / diagnostics | `SYS_INPUT_INJECT`, diagnostics overlay | QEMU monitor + scripted/live checks | 🟡 Test harness is good; no user-facing diagnostics. |
| Keyboard navigation of desktop | Win/arrow/Enter dock navigation, Alt shortcuts | none | ❌ |

### 4.10 Build, test, and tooling

| Capability | Reference | Current | Status and gap |
| --- | --- | --- | --- |
| Build | `build.sh`, Makefile, 67+ scripts, ISO | `build-efi.sh`, 15 tools, QEMU/OVMF image | 🟡 Current build is simpler and reproducible; it lacks packaging and hardware smoke. |
| Host tests | hundreds across libs, shell, browser, layout, TLS, X.509 | 176 `zc-abi`, 234 kernel, 253 libs test markers; workspace tests in CI | ✅ Strong for the implemented surface. |
| Boot proofs | QEMU scripts, framebuffer screenshots, serial assertions | scripted frame checksums, close proof, FM proof, live input, disk/fsck/poweroff checks | ✅ Current proof discipline is a differentiator; keep it. |
| Host emulator | yes | no | ❌ |
| CI | GitHub workflow, subset of suites | host build/test, changelog guard, boot job with many log assertions | ✅ Good foundation; add new parity checks to the boot job only when they are stable. |
| Docs | extensive but drifted | roadmap, architecture, 5 specs, 48 ADRs | 🟡 Current docs are tighter; parity work must add ADRs, not prose-only claims. |

## 5. Cross-cutting findings

### 5.1 The fixed task table is a microkernel limit, not an app shortcut

The current kernel has 10 fixed task slots. Two are window clients. The window manager can describe four windows, but the image cannot run more than two client tasks. The launcher therefore has only three real outcomes: open the second window, switch to the next open window, or do nothing.

The reference has a process registry and dynamic spawn. That is why its dock can launch nine apps. The current limit belongs to the microkernel and core-system steps, not to an app catalog. Adding a third client binary without changing the task model would be another one-off. Before the GUI grows, the task and service lifecycle should stop requiring an edit to `kernel/zc-kernel/src/service.rs` for every new service.

### 5.2 The proof model must not remain the product model

The kernel frame verifier is excellent for the scripted images. It is a poor foundation for a live desktop because every new visual feature becomes a new proof obligation. The reference does not have this constraint.

Recommended split:

- **Proof path**: `--test`, `--test-close`, `--fm`; keep the scripts, the verifier, and the locked checksums.
- **Product path**: default image; live events, dynamic windows, no per-frame kernel checksum; host tests and live QEMU checks guard behaviour instead.

Shared layout and widget code can be used by both paths. The verifier should not be a dependency of the app runtime. This split belongs to the simple-GUI step; it is not a reason to start GUI or app work before the core is ready.

### 5.3 The reference is not a clean target to copy

The reference has real breadth, but it also has known limits and some architectural debt:

- several drivers still live in ring 0;
- the Linux personality lives in the kernel;
- ZCFS writes are slow;
- the Linux ABI is partial and does not translate sockets;
- IP fragmentation is incomplete;
- browser hardening is still a work in progress;
- the signing key is a development key and default credentials exist for development;
- the documentation has drifted from the source in places.

Parity should copy the **user-visible outcomes**, not the debt.

### 5.4 Keep the current authority model

The current tree's ring-3 driver model, deny-by-default I/O bitmap, BAR/MMIO brokerage, coherent DMA window, and restartable services are better than the reference's mixed driver placement. Do not trade them away to close a device gap faster. Add device classes behind the same authority model.

### 5.5 Core parity first, desktop polish last

The reference's Prism desktop has a distinctive look: floating top bar, pill tabs, dock magnification, four themes, control centre, toasts. Reproducing the pixels is not the target of early work. The early target is a boot path, kernel, core services, shell, support libraries, storage, drivers, and network that can be trusted. A simple desktop that exposes those services comes after them; the visual polish and the application catalog come later. Apps and the host runner do not drive the order.

## 6. Core-first work order

This is a priority order, not a second milestone track. It follows the boot and dependency chain and deliberately puts the base system before the desktop. "Done" means the exit condition holds in the current tree and has a test or a boot check where one can exist. Section 4 still records the full gap; this section says what to close first.

Applications and the host runner are not part of this order. They come later.

### 6.1 Bootloader

**Goal.** A boot path that is understood, reproducible, and verified on the target.

- Audit the UEFI loader against the versioned `BootInfo` contract and `docs/specs/boot-info.md`.
- Make the hand-off deterministic: memory map, framebuffer, ACPI/RSDP, module placement, and failure paths.
- Record and resolve the SMP/OVMF `ExitBootServices` blocker; do not call the kernel multi-CPU ready while that path is unproven.
- Prove the same image boots reproducibly on QEMU/OVMF. A hardware claim needs a serial log and a repeatable procedure, not only a screenshot.

**Exit condition.** The image boots reproducibly; the boot contract is versioned and tested; every firmware-specific workaround is explained.

### 6.2 Microkernel

**Goal.** A small, predictable mechanism layer with no policy leakage.

- Audit address spaces, scheduling, IPC, capabilities, traps, timers, and power against `docs/specs/`.
- Document the fixed 10-slot task table and its revocation contract ([ADR 0050](adr/0050-the-microkernel-revokes-runtime-resources.md)). The bounded capacity stays for this step; a real create/wait/reap lifecycle is core-system work, because each slot's address space and grants are provisioned at boot and the slot index is a stable identity.
- Give services a discovery and delegation path so a new service does not require a new kernel constant or an edit to `service.rs`.
- Keep the syscall surface minimal; each new call gets a spec, host tests, and a boot proof where it is used.
- Decide the SMP scheduler story before the desktop depends on more tasks.

**Exit condition.** A ring-3 task can be created, run, fault, restart, and be reclaimed without a kernel special case; the ABI spec and the implementation agree.

### 6.3 Core system

**Goal.** The smallest complete userspace that can own the machine.

- `initd`: one ABI service catalog, per-service restart budget, and ordered shutdown are done ([ADR 0051](adr/0051-one-service-catalog-and-ordered-shutdown.md)). Liveness and readiness are now separate ([ADR 0054](adr/0054-liveness-and-readiness-are-separate.md)); explicit dependencies and failure health states are done ([ADR 0097](adr/0097-explicit-service-dependencies-and-failure-health-states.md)).
- Device manager, VFS boundary, logging, time, and a minimal identity/session boundary. Identity is now readable through `SYS_GETID` ([ADR 0053](adr/0053-identity-is-readable-but-not-settable.md)); a logging service, authentication, and sessions are still absent.
- One service registry: name, endpoint, capabilities, health, and restart policy. The catalog now supplies name, id, boot order, and restart policy, and the shell can observe state through a read-only grant ([ADR 0052](adr/0052-service-state-is-observable-without-control.md)) and reports liveness and self-declared readiness ([ADR 0054](adr/0054-liveness-and-readiness-are-separate.md)). It does not discover or load a service, and it has no endpoint or failure-rate data.
- One boot manifest that names services and order; no magic indices spread across compositor, shell, and tests. `zc.manifest` now carries `services=blk,kbd`, and it is authoritative for the supervised set and shutdown order: `initd` resolves it through `zc_supervise::plan_for`, stops any catalog service the file omits, and falls back to the full catalog when the file is unusable ([ADR 0058](adr/0058-the-manifest-selects-the-supervised-set.md)). Spawn order is still compile-time — the kernel provisions address spaces and capabilities before any task runs — and there is no reload path, so a new service still needs a fixed slot and an image.
- Keep the kernel VFS decision explicit; do not move it to a service without a measured reason.

**Exit condition.** Boot reaches a defined multi-service state; one supervised service can fault without taking down the rest; shutdown is ordered and repeatable.

### 6.4 Shell

**Goal.** A shell that can bring up, inspect, and shut down the system.

- Complete the command set for files, mounts, processes, services, devices, logs, time, memory, and power. `svc` inspects the service catalog with a read-only grant; processes (`ps`, `spawn`, `kill`, `wait`), devices (`dev`), system info (`uname`, [ADR 0094](adr/0094-system-and-device-inspection-in-the-shell.md)), memory (`free`/`mem`, [ADR 0095](adr/0095-physical-memory-introspection-and-the-free-command.md)), logs (`dmesg`), time (`date`/`time`, [ADR 0093](adr/0093-the-shell-inspects-the-system-clock.md); `uptime`, [ADR 0096](adr/0096-monotonic-time-and-the-uptime-command.md)), file operations (`cp`, `touch`, `head`, `wc`, [ADR 0098](adr/0098-file-utilities-in-the-shell.md); working directory `pwd`, `cd`, and relative paths, [ADR 0100](adr/0100-shell-working-directory-and-relative-paths.md); pattern search `grep` and conditional testing `test`, `[`, `true`, `false`, [ADR 0102](adr/0102-shell-pattern-search-and-conditional-testing.md)), and power (`poweroff`) are done.
- Add pipelines, redirection, background jobs, history, completion, and scripts only as far as the core services can support them. Output redirection (`>`, `>>`) and exit status tracking (`$?`) are done ([ADR 0101](adr/0101-shell-stdout-redirection-and-exit-status.md)); command chaining with sequential (`;`) and conditional (`&&`, `||`) operators is done ([ADR 0103](adr/0103-shell-command-chaining-and-conditional-operators.md)).
- Share one command layer between the serial shell and the window terminal.
- Report failures through exit status and stderr, not only a printed line. Command exit status tracking with `$?` is done ([ADR 0101](adr/0101-shell-stdout-redirection-and-exit-status.md)).

**Exit condition.** Shell commands exercise every core service; serial and window terminals agree; failure cases are tested.

### 6.5 Supporting libraries

**Goal.** Shared, tested libraries that the core and the later GUI can depend on.

- ABI types, errors, formatting, collections, synchronization, and math.
- ELF/image loading, storage parsers, configuration, and logging clients.
- Input codec, layout, drawing primitives, and a minimal widget set only when the simple-GUI step needs them.
- Each library has host tests and a documented interface; no service forks a private copy of a core library.

**Exit condition.** The core builds on the libraries; public APIs have host tests; a library change cannot silently alter a proof frame.

### 6.6 Storage, drivers, network, and security

**Goal.** The machine can use its real hardware and reach the network without weakening the authority model.

- Block: AHCI/SATA, NVMe, and virtio-blk behind the block service.
- Input: xHCI USB HID, I2C-HID touchpad, PS/2 fallback, hotplug, and restart.
- Display: virtio-GPU or a safe framebuffer path; audio and battery reporting later in this step.
- Network: virtio-net, e1000, and r8169; then IP, TCP, UDP, DHCP, DNS, and a socket API.
- Security: signed objects and policy, authentication/session, and TLS/X.509 once networking is stable.
- Every driver stays a ring-3 domain with brokered MMIO, DMA, IRQ, and port authority; a fault must be recoverable.

**Exit condition.** The target machine boots from internal storage with working keyboard, pointer, display, and network; a tampered signed object is rejected; a driver fault does not take down the desktop.

### 6.7 Simple GUI desktop

**Goal.** A small, honest desktop that exposes the core, not a visual clone.

- Keep one compositor, one window table, one input path, and one taskbar.
- The taskbar needs a launcher, running-window buttons, focus, and a clock.
- Window operations: focus, drag, minimize/restore, close. Add resize/maximize only when clients can reallocate surfaces safely.
- Keep the launcher minimal: open or focus a running client. Do not build an app catalog yet.
- Keep `--test`, `--test-close`, and `--fm` on their scripts and checksums. Split product rendering from proof verification only if the verifier starts to constrain normal UI work.

**Exit condition.** A person can boot the default image, use live keyboard and pointer input, and open, focus, move, and close at least two real clients; the three proof images still pass.

### 6.8 Later

Not part of the current order:

- additional applications (editor, viewer, calculator, monitor, settings, browser, audio player);
- app catalog, dock magnification, themes, control centre, notifications, and other Prism polish;
- the host runner/emulator;
- installer/updater, A/B slots, distribution packaging, and a Linux personality.

The host runner is useful for UI iteration, but it is not a prerequisite for a trustworthy core.

## 7. What not to carry over

- **Ring-0 drivers.** The reference's AHCI/NVMe/xHCI/e1000/r8169/ATA/USB/ framebuffer/keyboard/mouse drivers are not the target. Implement the device behind the current ring-3 authority model.
- **Kernel Linux personality.** Keep POSIX compatibility out of ring 0.
- **Reference source or assets.** No verbatim modules, constants, themes, fonts, or comments. Rebuild the behaviour independently.
- **Reference documentation as truth.** Its desktop guide already disagrees with its source about the dock catalog; use source and changelog for evidence.
- **Development signing keys and default credentials.** These are development aids, not product security.
- **Browser-first scope.** The browser is a large subsystem. Do not let it block the desktop, storage, or driver work.

## 8. Open decisions

1. **Licence / direct reuse.** Is the current repository staying public? If yes, parity must remain independent. If direct reuse is wanted, make the repository private or clarify the reference licence first.
2. **Process model.** The fixed table is documented as a bounded capacity in [ADR 0050](adr/0050-the-microkernel-revokes-runtime-resources.md). Decide in the core-system step whether to grow it or build a real spawn/exit/reap model before the GUI depends on more tasks.
3. **VFS boundary.** Keep the kernel VFS with `zc-storage` parsers, or move it to a service? Do not change this without a measured reason; the current model is working and proved.
4. **Boot target.** Stay UEFI-only, or add a BIOS/Limine path for the target laptop? Hardware parity may force this decision.
5. **Visual parity.** Is the goal "same tasks" or "same Prism look"? The work order assumes a simple desktop first; visual parity is later.
6. **Browser and Linux subsystem.** Are they 1.0 requirements or post-1.0? Treat them as post-1.0 unless the product scope says otherwise.

## 9. Starting points

1. Review and accept this map; keep the reference tree read-only.
2. Step 1 (bootloader) is complete: [ADR 0049](adr/0049-the-loader-stays-inside-its-identity-map.md) records the identity-map rule, and [`specs/boot-info.md`](specs/boot-info.md) now states the final-map and address-range guarantees. The three proof checksums still hold.
3. Step 2 (microkernel) is audited: [ADR 0050](adr/0050-the-microkernel-revokes-runtime-resources.md) records the bounded table and the end-of-life revocation path, and the kernel now enforces both. The three proof checksums still hold.
4. Step 3 (core system) has started: [ADR 0051](adr/0051-one-service-catalog-and-ordered-shutdown.md) makes the ABI catalog the one service list, surfaces a bounded per-service restart budget, and shuts live services down in reverse boot order. The shell can inspect that catalog through a read-only grant ([ADR 0052](adr/0052-service-state-is-observable-without-control.md)), and a task can read its own spawn-time identity ([ADR 0053](adr/0053-identity-is-readable-but-not-settable.md)). Service status now separates liveness from self-declared readiness ([ADR 0054](adr/0054-liveness-and-readiness-are-separate.md)). The compile-time catalog is still the registry; discovery, endpoints, and failure-rate routing remain. The boot manifest is now authoritative for the supervised service set and shutdown order ([ADR 0058](adr/0058-the-manifest-selects-the-supervised-set.md)); the kernel now reads and logs its service order at boot ([ADR 0076](adr/0076-the-kernel-reads-the-boot-manifest.md)), but the bring-up slot assignment stays fixed by the catalog. Kernel diagnostics are now a bounded ring that a task can read through `SYS_LOG_READ`, and the shell exposes it as `dmesg` ([ADR 0056](adr/0056-kernel-logs-live-in-a-bounded-readable-ring.md)); task output still stays on the serial line. The kernel now also reports its own restart and fault counters through `SYS_SERVICE_HEALTH`, so a restarted service's history is visible from `svc` ([ADR 0057](adr/0057-service-health-exposes-mechanism-counters.md)); the restart budget remains userspace policy. The task table now numbers each incarnation with a generation exposed through `SYS_TASK_INFO` and the shell's `ps`, so a revived slot is distinguishable from its predecessor ([ADR 0059](adr/0059-a-task-slot-carries-a-generation.md)). A clean exit records an exit status that the registry reports, so a slot's record says how its incarnation ended ([ADR 0060](adr/0060-a-clean-exit-records-a-status.md)); `SYS_SPAWN` now builds a task from a named initramfs image, `SYS_KILL` ends a runtime spawn with a chosen status, and `SYS_WAIT` blocks on a slot, returns that status, reaps the record, and returns the frames the spawn allocated, which is the ZC-native process model ([ADR 0070](adr/0070-a-task-is-spawned-by-name-and-waited-by-slot.md), [ADR 0071](adr/0071-a-reap-returns-the-frames-a-spawn-took.md), [ADR 0072](adr/0072-a-runtime-spawn-is-killed-by-its-spawner.md)). A task can now raise a coalescing event bit on a live peer in its own session with `SYS_NOTIFY`, and `SYS_EVENT_WAIT` blocks until a requested bit is pending, returns the matched bits, and clears them; this is a flag word, not a signal with handlers ([ADR 0073](adr/0073-a-task-notifies-a-peer-through-an-event-word.md)). Tasks now also belong to a session, fixed at spawn and visible through `SYS_SESSION` and `ps`. A task can mint a new session with `SYS_SESSION_CREATE` and spawn into it through `SYS_SPAWN`'s session argument, so the grouping is no longer build-time data only ([ADR 0061](adr/0061-sessions-group-tasks-as-spawn-time-data.md), [ADR 0077](adr/0077-a-login-creates-a-session.md)); a credential check and user database remain. The VFS can now remove a file through `SYS_UNLINK`, gated by the same parent-directory authority as `create`, and the shell exposes it as `rm` ([ADR 0062](adr/0062-file-removal-is-directory-authority.md)). The scratch filesystem now holds directories too, so a file can live below one and each directory lists only its own children ([ADR 0063](adr/0063-the-scratch-filesystem-is-hierarchical.md)); `zcfs` catches up below, since its `CREATE` records already carried the parent and the mode's type bit. A directory can then be removed once it is empty (`SYS_RMDIR`, [ADR 0064](adr/0064-a-directory-is-removed-only-when-empty.md)), so the scratch tree can be built up and torn down. An entry can also be renamed or moved within one filesystem (`SYS_RENAME`, [ADR 0065](adr/0065-a-rename-moves-a-node-within-one-filesystem.md)), completing the ordinary namespace operations for the scratch filesystem. The persistent volume can now remove a file too: `zcfs` appends an `UNLINK` record, so `rm /data/probe` survives an explicit remount, and `SYS_SYNC` flushes and stamps the volume clean on demand ([ADR 0066](adr/0066-a-persistent-removal-is-a-log-record.md)), and `mkdir` works on `/data`: the proxy adds the directory type bit to the same `CREATE` record, so the tree shape is durable ([ADR 0068](adr/0068-a-persistent-volume-holds-directories.md)). An empty directory can be removed for real too: `zcfs` appends an `RMDIR` record, and the empty check runs before the write ([ADR 0069](adr/0069-a-persistent-directory-removal-is-its-own-record.md)). Ownership is persisted on zcfs. A descriptor's offset is now explicit and movable through `SYS_SEEK`, so a file can be updated in place or appended ([ADR 0067](adr/0067-a-file-offset-is-explicit-and-movable.md)). The three proof checksums still hold.
5. Do not start the app catalog, widget toolkit, or host runner yet.
6. Keep `build/zcos.img` human-testable and keep the three proof images locked.

## 10. Evidence index

Reference paths:

- `README.md`, `changelog.md`, `docs/STRUCTURE.md`, `docs/ARCHITECTURE.md`, `docs/API.md`, `docs/desktop-guide.md`, `docs/DESKTOP_ENVIRONMENT_API.md`
- `boot/limine.conf`, `build.sh`, `run.sh`, `scripts/`
- `userspace/services/winmgr/src/desktop.rs` (9-entry catalog, top bar, dock, control centre)
- `userspace/services/winmgr/src/input_router.rs` (drag, resize, focus, close)
- `libzgui/src/app.rs` (`AppHandler` lifecycle)
- `libzgui/src/widget.rs`, `layout.rs`, `list.rs`, `scroll.rs`, `slider.rs`, `modal.rs`, `theme.rs` (widget set)
- `libzgui/src/apps/` (term, file, edit, calc, viewer, settings, sysmon, browser)
- `userspace/bin/shell/src/main.rs` (pipelines, jobs, history, completion, plugins)
- `docs/API.md` (19 endpoints), `userspace/services/`, `userspace/drivers/`
- `scripts/test_*.py` (91 scripts) and `tools/zcos-emu`

Current paths:

- `README.md`, `CHANGELOG.md`, `docs/roadmap.md`, `docs/architecture.md`, `docs/carry-over.md`
- `kernel/zc-kernel/src/service.rs` (10 slots, 2 window clients, 2 supervised services)
- `libs/zc-abi/src/desktop.rs` (taskbar, launcher, 4 task buttons)
- `libs/zc-abi/src/wm.rs` (4-window table, drag/minimize/close, no resize)
- `libs/zc-abi/src/syscall.rs` (39 syscalls)
- `user/zcompositor/src/main.rs`, `user/zc-win/src/main.rs`, `user/zc-fm/src/main.rs`
- `docs/adr/0038`–`0048` (window table, second client, FM, clock, live clicks)
- `docs/specs/` (boot, IPC, syscall, server protocol)
- `tools/check-live-input.sh`, `.github/workflows/rust.yml`
- `build/zcos*.img` and the checksums in `docs/roadmap.md`
