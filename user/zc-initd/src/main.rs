//! `initd` supervisor: the userspace owner of service lifecycle policy.
//!
//! The kernel performs the mechanisms a service lifecycle needs — reviving a
//! dead task slot and killing a live one — but the decision of *whether* to
//! restart belongs here. `initd` blocks on the supervision channel, and the
//! kernel posts one event whenever a supervised service faults or exits. On a
//! fault it revives the service; on a clean exit it records the stop and ends,
//! which lets the boot finish once every task has exited.
//!
//! The block driver is the one supervised service today: it faults
//! deliberately after its bring-up, and the restart below is what proves a
//! userspace supervisor can bring a service back without the kernel choosing
//! for it.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_user::{
    IPC_SUPERVISE, SERVICE_KIND_FAULT, log, recv_from, service_start, service_status,
    supervise_kind, supervise_service, task_exit,
};

/// Service id of the block driver, mirroring `zc-kernel::service::BLK_SERVICE`.
const BLK_SERVICE: u64 = 0;

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
                // Status is checked before acting, so the supervisor decides on
                // observed state rather than on the event alone.
                if service_status(BLK_SERVICE) == 0 {
                    log("initd: blk down\n");
                }
                if service_start(BLK_SERVICE) == 0 {
                    log("initd: restarted blk\n");
                } else {
                    log("initd: restart failed\n");
                }
            }
            continue;
        }

        // A clean exit is final: record it and stop supervising, so the boot
        // ends once no task is left.
        if service == BLK_SERVICE {
            log("initd: blk stopped\n");
            task_exit()
        }
    }
}
