# Architecture Decision Records

Short records of decisions that shape ZC OS: what we decided, why, and what it
costs. They exist so the same debate does not happen twice (skill rule **P6**).

## Index

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-microkernel-with-capabilities.md) | Capability-based microkernel | Accepted |
| [0002](0002-uefi-x86_64-first-platform.md) | x86_64 + UEFI as the first platform | Accepted |
| [0003](0003-rust-implementation-language.md) | Rust as the implementation language | Accepted |
| [0004](0004-framebuffer-first-no-3d.md) | Framebuffer-first rendering, no 3D in v1 | Accepted |
| [0005](0005-single-tss-port-bitmap.md) | One TSS, per-task port bitmap projected on switch | Accepted |
| [0006](0006-userspace-drivers-and-device-manager.md) | Userspace drivers with a device manager | Accepted |
| [0007](0007-vfs-core.md) | Read-only VFS with a trait-based filesystem | Accepted |
| [0008](0008-zc-native-log-structured-fs.md) | ZC-native log-structured filesystem | Accepted |
| [0009](0009-filesystem-rpc-replay.md) | Filesystem RPC with block-and-retry replay | Accepted |
| [0010](0010-userspace-service-supervision.md) | Userspace service supervision | Accepted |
| [0011](0011-crash-recovery-clamps-log-head.md) | Crash recovery clamps the log head | Accepted |
| [0012](0012-fsck-repairs-the-durable-prefix.md) | fsck repairs the durable prefix | Accepted |
| [0013](0013-permissions-and-ownership.md) | File permissions and ownership in the VFS | Accepted |
| [0014](0014-linux-driver-domain.md) | Reuse Linux drivers in a userspace driver domain | Accepted |
| [0015](0015-compositor-surface-model.md) | Compositor surface model and damage tracking | Accepted |
| [0016](0016-window-client-delegation.md) | Window clients and delegated surfaces | Accepted |
| [0017](0017-input-stream-with-mouse.md) | Input stream with a PS/2 mouse | Accepted |
| [0018](0018-window-placement-proof.md) | Window placement proof for interactive pixels | Accepted |
| [0019](0019-port-broker-capability.md) | Port broker capability for ring-3 PCI discovery | Accepted |
| [0020](0020-device-memory-broker.md) | Device memory broker capability for ring-3 drivers | Accepted |
| [0021](0021-coherent-dma-window.md) | Coherent DMA window for driver domains | Accepted |
| [0022](0022-terminal-commands-over-the-vfs.md) | Terminal commands execute over the VFS | Accepted |
| [0023](0023-pointer-and-mouse-read.md) | Pointer state and the mouse-read syscall | Accepted |
| [0024](0024-window-dragging-and-hit-testing.md) | Shared window placement for hit-testing and dragging | Accepted |
| [0025](0025-hidden-window-is-an-empty-rectangle.md) | A hidden window is an empty rectangle | Accepted |
| [0026](0026-closing-a-window-ends-a-session.md) | Closing a window ends a session, not just a rectangle | Accepted |
| [0027](0027-one-input-layer.md) | One shared input layer, and the start of F8e | Accepted |
| [0028](0028-boot-watchdog-only-in-test-builds.md) | The boot watchdog is armed only in test builds | Accepted |
| [0029](0029-two-scripted-sessions.md) | Two scripted sessions, one per final-frame proof | Accepted |
| [0030](0030-the-pointer-has-one-owner.md) | The pointer has exactly one owner | Accepted |
| [0031](0031-font-loads-from-the-vfs.md) | The font loads from the VFS, on every side | Accepted |
| [0032](0032-clean-shutdown-is-a-syscall.md) | Clean shutdown is a syscall, not a yank | Accepted |
| [0033](0033-device-interrupts-get-their-own-stack.md) | Device interrupts get their own stack | Accepted |
| [0034](0034-a-window-has-its-own-pointer-stream.md) | A window has its own pointer stream | Accepted |
| [0035](0035-a-scripted-click-is-a-proof.md) | A scripted click is a proof, not a demo | Accepted |
| [0036](0036-a-directory-can-be-listed.md) | A directory can be listed, and `ls` proves it twice | Accepted |
| [0037](0037-a-served-filesystem-can-list.md) | A filesystem served over IPC can list | Accepted |
| [0038](0038-the-window-manager-holds-a-table.md) | The window manager holds a table, not a window | Accepted |
| [0039](0039-a-second-window-client-and-per-window-state.md) | A second window client, and per-window kernel state | Accepted |
| [0040](0040-a-second-window-is-visible.md) | A second window is visible | Accepted, partly superseded by 0045 |
| [0041](0041-the-second-window-is-a-file-manager.md) | The second window is a file manager | Accepted, partly superseded by 0045 |
| [0042](0042-the-proof-scripts-belong-to-the-test-build.md) | The proof scripts belong to the test build | Accepted |
| [0043](0043-the-file-manager-navigates-by-proof.md) | The file manager navigates by proof | Accepted, partly superseded by 0045 |
| [0044](0044-the-file-manager-opens-a-file-by-proof.md) | The file manager opens a file by proof | Accepted, partly superseded by 0045 |
| [0045](0045-the-default-image-ships-the-apps-a-person-can-test.md) | The default image ships the apps a person can test | Accepted, partly superseded by 0047 |
| [0046](0046-the-taskbar-clock-is-live.md) | The taskbar clock is live | Accepted |
| [0047](0047-the-default-desktop-opens-the-file-manager.md) | The default desktop opens the file manager | Accepted |
| [0048](0048-live-clicks-survive-the-kernel.md) | A live click survives the kernel | Accepted |
| [0049](0049-the-loader-stays-inside-its-identity-map.md) | The loader stays inside its identity map | Accepted |
| [0050](0050-the-microkernel-revokes-runtime-resources.md) | The microkernel revokes runtime resources at end of life | Accepted |
| [0051](0051-one-service-catalog-and-ordered-shutdown.md) | One service catalog, a bounded restart budget, and ordered shutdown | Accepted |
| [0052](0052-service-state-is-observable-without-control.md) | Service state is observable without control | Accepted |
| [0053](0053-identity-is-readable-but-not-settable.md) | Identity is readable but not settable | Accepted |
| [0054](0054-liveness-and-readiness-are-separate.md) | Liveness and readiness are separate service states | Accepted |
| [0055](0055-the-boot-manifest-is-checked-not-authoritative.md) | The boot manifest is parsed and checked, not yet authoritative | Superseded by [0058](0058-the-manifest-selects-the-supervised-set.md) |
| [0056](0056-kernel-logs-live-in-a-bounded-readable-ring.md) | Kernel diagnostics live in a bounded ring that a task can read | Accepted |
| [0057](0057-service-health-exposes-mechanism-counters.md) | Service health exposes kernel mechanism counters, not policy | Accepted |
| [0058](0058-the-manifest-selects-the-supervised-set.md) | The boot manifest selects the supervised service set | Accepted |
| [0059](0059-a-task-slot-carries-a-generation.md) | A task slot carries a generation so incarnations are distinguishable | Accepted |
| [0060](0060-a-clean-exit-records-a-status.md) | A clean exit records a status in the task registry | Accepted |
| [0061](0061-sessions-group-tasks-as-spawn-time-data.md) | Sessions group tasks as spawn-time data | Accepted |
| [0062](0062-file-removal-is-directory-authority.md) | File removal is authority over the parent directory | Accepted |
| [0063](0063-the-scratch-filesystem-is-hierarchical.md) | The scratch filesystem is hierarchical | Accepted |
| [0064](0064-a-directory-is-removed-only-when-empty.md) | A directory is removed only when empty | Accepted |
| [0065](0065-a-rename-moves-a-node-within-one-filesystem.md) | A rename moves a node within one filesystem | Accepted |
| [0066](0066-a-persistent-removal-is-a-log-record.md) | A persistent removal is a log record | Accepted |
| [0067](0067-a-file-offset-is-explicit-and-movable.md) | A file offset is explicit and movable | Accepted |
| [0068](0068-a-persistent-volume-holds-directories.md) | A persistent volume holds directories | Accepted |
| [0069](0069-a-persistent-directory-removal-is-its-own-record.md) | A persistent directory removal is its own record | Accepted |
| [0070](0070-a-task-is-spawned-by-name-and-waited-by-slot.md) | A task is spawned by name and waited by slot | Accepted |
| [0071](0071-a-reap-returns-the-frames-a-spawn-took.md) | A reap returns the frames a spawn took | Accepted |
| [0072](0072-a-runtime-spawn-is-killed-by-its-spawner.md) | A runtime spawn is killed by its spawner | Accepted |
| [0073](0073-a-task-notifies-a-peer-through-an-event-word.md) | A task notifies a peer through an event word | Accepted |
| [0074](0074-a-persistent-rename-is-a-log-record.md) | A persistent rename is a log record | Accepted |
| [0075](0075-a-persistent-owner-is-a-log-record.md) | A persistent owner is a log record | Accepted |
| [0076](0076-the-kernel-reads-the-boot-manifest.md) | The kernel reads the boot manifest's service order | Accepted |
| [0077](0077-a-login-creates-a-session.md) | A login creates a session | Accepted |
| [0078](0078-a-stable-userspace-c-abi.md) | A stable userspace C ABI | Accepted |
| [0079](0079-service-discovery-at-boot.md) | Service discovery at boot | Accepted |
| [0080](0080-smp-bring-up-is-parked.md) | SMP bring-up is parked | Accepted |
| [0081](0081-signal-registration-and-raise.md) | Signal registration and raise | Accepted |
| [0082](0082-a-peers-signal-runs-at-the-targets-next-kernel-entry.md) | A peer's signal runs at the target's next kernel entry | Accepted |
| [0083](0083-the-host-checker-replays-every-record-kind.md) | The host checker replays every record kind the guest writes | Accepted |
| [0084](0084-a-service-publishes-its-endpoint-at-runtime.md) | A service publishes its endpoint at runtime | Accepted |
| [0085](0085-the-firmwares-tables-stay-installed-for-exitbootservices.md) | The firmware's tables stay installed for ExitBootServices | Accepted |
| [0086](0086-a-block-with-no-runnable-peer-waits-for-one.md) | A block with no runnable peer waits for one | Accepted |
| [0087](0087-a-windows-widgets-are-data-one-layout-pass-places.md) | A window's widgets are data one layout pass places | Accepted |
| [0088](0088-a-second-widget-resets-the-first.md) | A second widget resets the first | Accepted |
| [0089](0089-a-click-gives-a-widget-the-focus.md) | A click gives a widget the focus | Accepted |
| [0090](0090-the-verifier-replays-what-the-kernel-served.md) | The verifier replays what the kernel served | Accepted |
| [0091](0091-space-activates-the-focused-widget.md) | Space activates the focused widget at a bare prompt | Accepted |
| [0092](0092-static-text-label-widget.md) | Static text label widget | Accepted |
| [0093](0093-the-shell-inspects-the-system-clock.md) | The shell inspects the system clock | Accepted |
| [0094](0094-system-and-device-inspection-in-the-shell.md) | System and device inspection in the shell | Accepted |
| [0095](0095-physical-memory-introspection-and-the-free-command.md) | Physical memory introspection and the free command | Accepted |
| [0096](0096-monotonic-time-and-the-uptime-command.md) | Monotonic time and the uptime command | Accepted |
| [0097](0097-explicit-service-dependencies-and-failure-health-states.md) | Explicit service dependencies and failure health states | Accepted |
| [0098](0098-file-utilities-in-the-shell.md) | File utilities in the shell: cp, touch, head, and wc | Accepted |
| [0099](0099-synchronize-boot-feeder-with-shell-ready-prompt.md) | Synchronize boot feeder with shell ready prompt | Accepted |
| [0100](0100-shell-working-directory-and-relative-paths.md) | Shell working directory tracking and relative path resolution | Accepted |
| [0101](0101-shell-stdout-redirection-and-exit-status.md) | Shell standard output redirection and exit status tracking | Accepted |
| [0102](0102-shell-pattern-search-and-conditional-testing.md) | Shell pattern search (grep) and conditional testing (test, true, false) | Accepted |
| [0103](0103-shell-command-chaining-and-conditional-operators.md) | Shell command chaining (;, &&, \|\|) | Accepted |
| [0104](0104-shell-environment-variables-and-parameter-expansion.md) | Shell environment variables and parameter expansion | Accepted |
| [0105](0105-shell-pipeline-execution.md) | Shell pipeline execution (`\|` operator) | Accepted |
| [0106](0106-shell-script-execution-and-comments.md) | Shell script execution (sh, source, .) and comments (#) | Accepted |

## Adding a decision

1. Copy [`TEMPLATE.md`](TEMPLATE.md) to `NNNN-short-title.md` with the next
   number.
2. Keep it 3–10 lines of decision. Link the code and the roadmap phase.
3. Add a row to the index above.
4. Once accepted, do not rewrite it. Supersede it with a new ADR and mark the
   old one `Superseded by NNNN`.
