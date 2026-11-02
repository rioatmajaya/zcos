//! `initd` supervisor: the userspace owner of service lifecycle policy.
//!
//! The kernel performs the mechanisms a service lifecycle needs — reviving a
//! dead task slot and killing a live one — but the decision of *whether* to
//! restart belongs here. `initd` blocks on the supervision channel, and the
//! kernel posts one event whenever a supervised service faults or exits.
//!
//! The service list, its boot order, and the user-visible names come from
//! [`zc_supervise`] (the ABI catalog); this loop only applies policy. A fault
//! consumes one restart from a per-service budget, so a crash loop cannot keep
//! the boot alive forever. A clean exit is final: the service is recorded as
//! stopped and every service still running is stopped in reverse boot order,
//! dependents before providers, so no live driver outlives the boot.
//!
//! The block driver faults deliberately after its bring-up, proving that a
//! supervisor — not the kernel — decides to revive a service. The keyboard
//! driver is persistent from F8d-3a, so shutting it down last is what lets the
//! boot finish once every task has exited.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_supervise::manifest::Manifest;
use zc_supervise::{RestartBudget, ServiceSpec, shutdown_order, spec};
use zc_user::{
    IPC_SUPERVISE, SERVICE_KIND_EXIT, SERVICE_KIND_FAULT, close, getid, identity_uid, log,
    log_parts, open, read, recv_from, service_is_up, service_start, service_status, service_stop,
    supervise_kind, supervise_service, task_exit,
};

/// Boot manifest the supervisor checks against the compiled catalog.
const MANIFEST_PATH: &str = "zc.manifest";

/// Largest manifest the supervisor reads. The shipped file is well under it;
/// a longer one is reported instead of parsed from a truncated prefix.
const MANIFEST_MAX: usize = 256;

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("initd starting\n");
    // The supervisor is the only root task (see `zc_kernel::service::OWNERS`).
    // If spawn ever changed that, the identity the VFS treats as privileged
    // would silently move, so the boot says which one it actually got.
    if identity_uid(getid()) == 0 {
        log("initd: root identity\n");
    } else {
        log("initd: not root\n");
    }
    check_manifest();
    let mut budget = RestartBudget::new();
    loop {
        let event = recv_from(IPC_SUPERVISE as u64);
        let id = supervise_service(event);
        let kind = supervise_kind(event);

        // A malformed event naming no catalog service must not end the boot.
        let Some(service) = spec(id) else {
            log("initd: unknown service\n");
            continue;
        };

        match kind {
            SERVICE_KIND_FAULT => restart(service, &mut budget),
            SERVICE_KIND_EXIT => {
                log_parts(&["initd: ", service.name, " stopped\n"]);
                stop_live_services();
                task_exit()
            }
            _ => {
                log("initd: unknown event\n");
            }
        }
    }
}

/// Reads the boot manifest and checks it against the compiled catalog.
///
/// The kernel still spawns the catalog at build time, so this is a consistency
/// check: a stale or malformed manifest is visible in the boot transcript
/// instead of being silently ignored. A missing manifest is not fatal — the
/// supervisor can still use the compiled list — and an unreadable or malformed
/// one falls back the same way.
fn check_manifest() {
    let fd = open(MANIFEST_PATH);
    if fd == u64::MAX {
        log("initd: manifest absent\n");
        return;
    }
    let mut buffer = [0u8; MANIFEST_MAX];
    let count = read(fd, &mut buffer);
    close(fd);
    if count == u64::MAX || count as usize >= MANIFEST_MAX {
        log("initd: manifest unreadable\n");
        return;
    }
    let Ok(text) = core::str::from_utf8(&buffer[..count as usize]) else {
        log("initd: manifest not utf8\n");
        return;
    };
    match Manifest::parse(text) {
        Ok(manifest) if manifest.matches_boot() => log("initd: manifest ok\n"),
        Ok(_) => log("initd: manifest mismatch\n"),
        Err(_) => log("initd: manifest malformed\n"),
    };
}

/// Revives `service` after a fault, or gives up when its budget is spent.
///
/// The status check only feeds the "down" diagnostic; `service_start` is
/// idempotent, so an already-live service is never reset by a duplicate event.
fn restart(service: ServiceSpec, budget: &mut RestartBudget) {
    if !budget.record(service.id) {
        log_parts(&["initd: ", service.name, " restart budget spent\n"]);
        return;
    }
    if !service_is_up(service_status(u64::from(service.id))) {
        log_parts(&["initd: ", service.name, " down\n"]);
    }
    if service_start(u64::from(service.id)) == 0 {
        log_parts(&["initd: restarted ", service.name, "\n"]);
    } else {
        log("initd: restart failed\n");
    }
}

/// Stops every service still running, dependents before their providers.
///
/// The service whose exit started the shutdown is already gone, so the status
/// check skips it instead of stopping it twice.
fn stop_live_services() {
    for service in shutdown_order() {
        if !service_is_up(service_status(u64::from(service.id))) {
            continue;
        }
        if service_stop(u64::from(service.id)) == 0 {
            log_parts(&["initd: ", service.name, " stopped\n"]);
        }
    }
}
