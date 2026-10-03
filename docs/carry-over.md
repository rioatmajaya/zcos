# Carry-over from the earlier ZC OS

Before this open-source rewrite there was a closed prototype at
`~/Desktop/Project/new-os/zcos`. It reached bare metal (USB boot, working
keyboard) and built the hard parts of an OS — a capability kernel, IPC, a
compositor, a GUI toolkit, apps, a signed binary format, and a host emulator.
Its one failure was strategic: every driver was written by hand, and that did
not scale ([ADR 0014](adr/0014-linux-driver-domain.md)).

The prototype is postponed, not abandoned. This note records what is worth
carrying into the open-source tree and in what order, so months of work are not
repeated. Nothing here is a copy-paste: the two trees have different syscall
numbers, capability models, and IPC, so every item below needs an **ABI shim**
or a port, not a plain move.

## Worth carrying

| Asset | Old location | What it gives | Effort | When |
|---|---|---|---|---|
| Host emulator | `tools/zcos-emu/` | Runs the desktop in-process with `minifb` input, no QEMU — the fastest iteration loop for UI work | Medium | Developer tooling, early |
| Compositor engine | `libzcos/src/compositor/` (`engine`, `surface`, `theme`) | Software compositor with damage tracking; already shared between bare metal and the emulator | Medium | F8 |
| GUI toolkit | `libzgui/src/` (`window`, `layout`, `list`, `scroll`, `slider`, `modal`, `theme`, `app`) | Widget and layout library | Medium | F8 |
| Window manager | `userspace/services/winmgr/src/` (`desktop`, `input_router`, `greeter`) | Desktop policy, input routing, login greeter | Medium | F8 |
| Applications | `userspace/bin/` (`zterm`, `zfile`, `zedit`, `zviewer`, `zsettings`, `zcalc`, `zsysmon`) | Daily-driver apps | Medium | F8 |
| `.sof` format + tools | `libsof/`, `tools/sofc`, `sofinfo`, `sofsign`, `sofverify`, `kernel/src/sof.rs` | Signed, verifiable binary/package format — a head start on F9 distribution | Medium | F9 |
| Linux ELF personality | `kernel/src/linux_syscall.rs`, `linux_fs.rs` | Runs static Linux x86_64 ELF (busybox, tcc) — the seed of the rootfs subsystem | High | Linux subsystem (later) |
| Shared client library | `libzcos/src/` (`fs`, `net`, `sock`, `dir`, `shared_memory`, `pkg`) | Client-side wrappers for services | Medium | As needed |
| Service designs | `userspace/services/` (`vfs`, `netd`, `authd`, `logd`, `procmgr`, `clipboardd`) | Working service decompositions to compare against | Low | Reference |

The Linux ELF personality is the most important one to carry **differently**:
in the prototype it lives in `kernel/src`, which violates the microkernel
boundary. In this tree it belongs in a userspace personality domain, alongside
the later rootfs subsystem.

## Do not carry

- **Hand-written drivers** (`userspace/drivers/{battery,gpu,hda,nvme,touchpad,wifi,xhci}`).
  This is the part the rewrite replaces with a Linux driver domain.
- **Drivers living in the kernel** (`kernel/src/{nvme,xhci,e1000,r8169,ahci,ata,usb,virtio*,framebuffer,keyboard,mouse}.rs`).
  Ring-0 drivers are exactly what this tree moved out.

## Suggested order

1. **`zcos-emu` first.** UI and service work is far faster without a boot cycle;
   porting the emulator pays for itself before any desktop code moves.
2. **F8 desktop.** Compositor engine → `libzgui` → `winmgr` → one app (`zterm`)
   as the proof, then the rest.
3. **F9 distribution.** `.sof` signing and packaging.
4. **Linux subsystem (later).** Re-home the ELF personality out of the kernel,
   then the downloaded-rootfs goal on top of it.

Each item should land only when this tree can run it end to end and prove it in
the boot log or a host check — the same bar every other phase uses.
