//! `initd` supervisor: the userspace owner of service lifecycle policy.
//!
//! The kernel performs the mechanisms a service lifecycle needs — reviving a
//! dead task slot and killing a live one — but the decision of *whether* to
//! restart belongs here. `initd` blocks on the supervision channel, and the
//! kernel posts one event whenever a supervised service faults or exits. On a
//! fault it revives the service; on a clean exit it records the stop and ends,
//! which lets the boot finish once every task has exited.
//!
//! Two services are supervised: the block driver, which faults deliberately
//! after its bring-up and proves a supervisor can revive a service without the
//! kernel choosing for it; and the keyboard driver, which is persistent from
//! F8d-3a. The keyboard domain keeps serving after its own boot fault, so the
//! supervisor stops it when the block driver stops — otherwise a live driver
//! would keep the boot from ever finishing.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{
    IPC_SUPERVISE, SERVICE_KIND_FAULT, log, recv_from, service_start, service_status, service_stop,
    supervise_kind, supervise_service, task_exit,
};

/// Service id of the block driver, mirroring `zc-kernel::service::BLK_SERVICE`.
const BLK_SERVICE: u64 = 0;

/// Service id of the keyboard driver, mirroring `zc-kernel::service::KBD_SERVICE`.
const KBD_SERVICE: u64 = 1;

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    log("initd starting\n");
    loop {
        let event = recv_from(IPC_SUPERVISE as u64);
        let service = u64::from(supervise_service(event));
        let kind = supervise_kind(event);

        if kind == SERVICE_KIND_FAULT {
            if service == BLK_SERVICE {
                if service_status(BLK_SERVICE) == 0 {
                    log("initd: blk down\n");
                }
                if service_start(BLK_SERVICE) == 0 {
                    log("initd: restarted blk\n");
                } else {
                    log("initd: restart failed\n");
                }
            } else if service == KBD_SERVICE {
                if service_status(KBD_SERVICE) == 0 {
                    log("initd: kbd down\n");
                }
                if service_start(KBD_SERVICE) == 0 {
                    log("initd: restarted kbd\n");
                } else {
                    log("initd: restart failed\n");
                }
            }
            continue;
        }

        // A clean exit is final: the block driver stopped after the shell
        // asked it to. The keyboard driver is persistent, so stop it now —
        // otherwise a live driver would keep the boot from finishing — then
        // end supervision so the boot ends once no task is left.
        if service == BLK_SERVICE {
            log("initd: blk stopped\n");
            if service_stop(KBD_SERVICE) == 0 {
                log("initd: kbd stopped\n");
            }
            task_exit()
        }
    }
}
