//! `initd` supervisor: the userspace owner of service lifecycle policy.
//!
//! The kernel performs the mechanisms a service lifecycle needs — reviving a
//! dead task slot and killing a live one — but the decision of *whether* to
//! restart belongs here. `initd` blocks on the supervision channel, and the
//! kernel posts one event whenever a supervised service faults or exits.
//!
//! The supervised set and its order come from `zc.manifest`, resolved against
//! the ABI catalog by [`zc_supervise::plan_for`]; a service the manifest omits
//! is stopped at startup, and the survivors shut down in reverse manifest
//! order. When the manifest is absent or not usable the full catalog is used,
//! because leaving a service unsupervised is worse than supervising one the
//! file forgot. A fault consumes one restart from a per-service budget, so a
//! crash loop cannot keep the boot alive forever. A clean exit is final: the
//! service is recorded as stopped and every planned service still running is
//! stopped in reverse order, dependents before providers.
//!
//! The block driver faults deliberately after its bring-up, proving that a
//! supervisor — not the kernel — decides to revive a service. The keyboard
//! driver is persistent from F8d-3a, so shutting it down last is what lets the
//! boot finish once every task has exited.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_supervise::manifest::Manifest;
use zc_supervise::{RestartBudget, SERVICE_CATALOG, ServiceSpec, SupervisionPlan, plan_for, spec};
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
    let plan = load_plan();
    log_plan(&plan);
    retire_unplanned(&plan);
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
        // A service the manifest omitted was stopped at startup; an event
        // queued before that stop must not revive it behind the file's back.
        if !plan.supervises(id) {
            log_parts(&["initd: ", service.name, " not supervised\n"]);
            continue;
        }

        match kind {
            SERVICE_KIND_FAULT => restart(service, &mut budget),
            SERVICE_KIND_EXIT => {
                log_parts(&["initd: ", service.name, " stopped\n"]);
                stop_live_services(&plan);
                task_exit()
            }
            _ => {
                log("initd: unknown event\n");
            }
        }
    }
}

/// Builds the supervision plan from the boot manifest.
///
/// A manifest that passes [`Manifest::matches_boot`] is authoritative for the
/// supervised set and order. Anything else — absent, unreadable, not UTF-8,
/// malformed, wrong init, or naming no service — is logged and falls back to
/// the full catalog, so a broken file cannot leave a service unsupervised.
fn load_plan() -> SupervisionPlan {
    let fd = open(MANIFEST_PATH);
    if fd == u64::MAX {
        log("initd: manifest absent\n");
        return plan_for(None);
    }
    let mut buffer = [0u8; MANIFEST_MAX];
    let count = read(fd, &mut buffer);
    close(fd);
    if count == u64::MAX || count as usize >= MANIFEST_MAX {
        log("initd: manifest unreadable\n");
        return plan_for(None);
    }
    let Ok(text) = core::str::from_utf8(&buffer[..count as usize]) else {
        log("initd: manifest not utf8\n");
        return plan_for(None);
    };
    match Manifest::parse(text) {
        Ok(manifest) if manifest.matches_boot() => {
            log("initd: manifest ok\n");
            plan_for(Some(&manifest))
        }
        Ok(_) => {
            log("initd: manifest mismatch\n");
            plan_for(None)
        }
        Err(_) => {
            log("initd: manifest malformed\n");
            plan_for(None)
        }
    }
}

/// Logs the services the supervisor will watch, in order.
fn log_plan(plan: &SupervisionPlan) {
    let mut buffer = [0u8; 128];
    let prefix = b"initd: supervising ";
    buffer[..prefix.len()].copy_from_slice(prefix);
    let mut len = prefix.len();
    for (index, service) in plan.services().iter().enumerate() {
        if index > 0 && len < buffer.len() {
            buffer[len] = b',';
            len += 1;
        }
        for &byte in service.name.as_bytes() {
            if len >= buffer.len() {
                break;
            }
            buffer[len] = byte;
            len += 1;
        }
    }
    if len < buffer.len() {
        buffer[len] = b'\n';
        len += 1;
    }
    log(core::str::from_utf8(&buffer[..len]).unwrap_or("initd: supervising ?\n"));
}

/// Stops every catalog service the plan does not supervise.
///
/// The kernel spawns the whole catalog before any task runs; the manifest
/// decides which of those stay supervised. Stopping the rest keeps the running
/// set equal to the file instead of leaving stray services the supervisor
/// cannot account for.
fn retire_unplanned(plan: &SupervisionPlan) {
    for service in SERVICE_CATALOG {
        if plan.supervises(service.id) {
            continue;
        }
        if !service_is_up(service_status(u64::from(service.id))) {
            continue;
        }
        if service_stop(u64::from(service.id)) == 0 {
            log_parts(&["initd: ", service.name, " not in manifest, stopped\n"]);
        }
    }
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

/// Stops every planned service still running, dependents before providers.
///
/// The service whose exit started the shutdown is already gone, so the status
/// check skips it instead of stopping it twice.
fn stop_live_services(plan: &SupervisionPlan) {
    for service in plan.shutdown() {
        if !service_is_up(service_status(u64::from(service.id))) {
            continue;
        }
        if service_stop(u64::from(service.id)) == 0 {
            log_parts(&["initd: ", service.name, " stopped\n"]);
        }
    }
}
