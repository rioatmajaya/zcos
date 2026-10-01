//! x86 I/O permission bitmaps for the TSS.
//!
//! A bitmap with one bit per port (0 = allowed) lets a ring-3 task use
//! exactly the ports its device needs instead of blanket IOPL. The bitmap
//! lives past the 104-byte TSS followed by a terminating `0xFF` byte; the
//! CPU reads it on every port access once the TSS I/O-map base points at
//! it. All arithmetic here is pure so host tests pin the layout.

/// Bytes in a full 65536-port bitmap.
pub const BITMAP_BYTES: usize = 8192;

/// Offset of the bitmap inside the extended TSS.
pub const BITMAP_OFFSET: usize = 104;

/// Bytes of the extended TSS: base structure, bitmap, terminator.
pub const TSS_BITMAP_SIZE: usize = BITMAP_OFFSET + BITMAP_BYTES + 1;

/// Total ports covered.
pub const PORTS: u32 = 65536;

/// Port ranges one task may hold.
///
/// Small on purpose: a driver domain needs a device window, not the port
/// space. A grant beyond this bound is dropped, so the mapping from policy to
/// bitmap stays total and fails closed.
pub const MAX_TASK_PORTS: usize = 4;

/// Per-task port authority, projected onto one shared bitmap.
///
/// The CPU reads the I/O bitmap inside the single TSS, and a TSS descriptor
/// can only be loaded once, so per-task rights cannot live in per-task TSS
/// descriptors. They live here instead: this table is the policy, and
/// [`TaskPorts::apply_to`] rebuilds the TSS bitmap from one task's entry on
/// every switch. Rebuilding rather than tracking deltas costs one 8 KiB fill
/// per switch and cannot forget a revoke.
#[derive(Clone, Copy)]
pub struct TaskPorts<const SLOTS: usize> {
    /// First port of each grant a task holds.
    starts: [[u16; MAX_TASK_PORTS]; SLOTS],
    /// Length of each grant, parallel to `starts`.
    lengths: [[u16; MAX_TASK_PORTS]; SLOTS],
    /// Grants actually recorded per task.
    counts: [u8; SLOTS],
}

impl<const SLOTS: usize> TaskPorts<SLOTS> {
    /// Creates a table where no task may touch any port.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            starts: [[0; MAX_TASK_PORTS]; SLOTS],
            lengths: [[0; MAX_TASK_PORTS]; SLOTS],
            counts: [0; SLOTS],
        }
    }

    /// Grants `len` ports from `start` to a task, reporting whether it stuck.
    ///
    /// A zero-length grant is refused: it would consume a slot while
    /// authorising nothing. A task past [`MAX_TASK_PORTS`] grants is refused
    /// too, so the table stays bounded and fails closed rather than growing.
    pub fn grant(&mut self, task: usize, start: u16, len: u16) -> bool {
        if task >= SLOTS || len == 0 {
            return false;
        }
        let count = self.counts[task] as usize;
        if count >= MAX_TASK_PORTS {
            return false;
        }
        self.starts[task][count] = start;
        self.lengths[task][count] = len;
        self.counts[task] = count as u8 + 1;
        true
    }

    /// Revokes everything a task held, returning the ports it gave up.
    pub fn revoke(&mut self, task: usize) -> usize {
        if task >= SLOTS {
            return 0;
        }
        let ports = self.allowed_count(task);
        self.counts[task] = 0;
        ports
    }

    /// Counts the ports a task currently holds.
    #[must_use]
    pub fn allowed_count(&self, task: usize) -> usize {
        if task >= SLOTS {
            return 0;
        }
        let count = self.counts[task] as usize;
        let mut ports = 0usize;
        let mut index = 0;
        while index < count {
            ports += usize::from(self.lengths[task][index]);
            index += 1;
        }
        ports
    }

    /// Rebuilds `bitmap` from one task's grants, denying everything else.
    ///
    /// An out-of-range task clears the bitmap, so a bad index can never
    /// inherit the previous task's ports.
    pub fn apply_to(&self, task: usize, bitmap: &mut [u8]) {
        deny_all(bitmap);
        if task >= SLOTS {
            return;
        }
        let count = self.counts[task] as usize;
        let mut index = 0;
        while index < count {
            allow_range(bitmap, self.starts[task][index], self.lengths[task][index]);
            index += 1;
        }
    }
}

impl<const SLOTS: usize> Default for TaskPorts<SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

/// Marks a whole bitmap denied: the state every task starts in.
///
/// # Panics
///
/// Panics in debug builds when `bitmap` is shorter than [`BITMAP_BYTES`].
pub fn deny_all(bitmap: &mut [u8]) {
    debug_assert!(bitmap.len() >= BITMAP_BYTES);
    bitmap[..BITMAP_BYTES].fill(0xFF);
}

/// Returns the byte index and mask for `port`.
#[must_use]
pub const fn bit(port: u16) -> (usize, u8) {
    ((port as usize) / 8, 1 << ((port % 8) as u8))
}

/// Marks `port` denied (sets its bit), the reverse of [`allow`].
pub fn deny(bitmap: &mut [u8], port: u16) {
    debug_assert!(bitmap.len() >= BITMAP_BYTES);
    let (index, mask) = bit(port);
    bitmap[index] |= mask;
}

/// Marks every port in `start..start+len` denied, saturating at 65536.
pub fn deny_range(bitmap: &mut [u8], start: u16, len: u16) {
    let mut port = start as u32;
    let end = (port + u32::from(len)).min(PORTS);
    while port < end {
        deny(bitmap, port as u16);
        port += 1;
    }
}

/// Marks `port` allowed (clears its deny bit).
///
/// # Panics
///
/// Panics in debug builds when `bitmap` is shorter than [`BITMAP_BYTES`].
pub fn allow(bitmap: &mut [u8], port: u16) {
    debug_assert!(bitmap.len() >= BITMAP_BYTES);
    let (index, mask) = bit(port);
    bitmap[index] &= !mask;
}

/// Marks every port in `start..start+len` allowed, saturating at 65536.
pub fn allow_range(bitmap: &mut [u8], start: u16, len: u16) {
    let mut port = start as u32;
    let end = (port + u32::from(len)).min(PORTS);
    while port < end {
        allow(bitmap, port as u16);
        port += 1;
    }
}

/// Returns whether `port` is allowed by `bitmap`.
#[must_use]
pub fn is_allowed(bitmap: &[u8], port: u16) -> bool {
    debug_assert!(bitmap.len() >= BITMAP_BYTES);
    let (index, mask) = bit(port);
    bitmap[index] & mask == 0
}

/// Counts allowed ports, for diagnostics.
#[must_use]
pub fn allowed_count(bitmap: &[u8]) -> usize {
    let mut count = 0;
    for byte in bitmap.iter() {
        count += (!byte).count_ones() as usize;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn denied() -> [u8; BITMAP_BYTES] {
        [0xFF; BITMAP_BYTES]
    }

    #[test]
    fn layout_covers_all_ports() {
        assert_eq!(BITMAP_BYTES * 8, PORTS as usize);
        assert_eq!(TSS_BITMAP_SIZE, 104 + 8192 + 1);
        assert_eq!(BITMAP_OFFSET, 104);
    }

    #[test]
    fn bit_positions_match_intel_layout() {
        assert_eq!(bit(0x0000), (0, 0x01));
        assert_eq!(bit(0x0007), (0, 0x80));
        assert_eq!(bit(0x0008), (1, 0x01));
        assert_eq!(bit(0x03F8), (0x7F, 0x01));
        assert_eq!(bit(0xFFFF), (8191, 0x80));
    }

    #[test]
    fn allow_clears_only_its_bit() {
        let mut map = denied();
        allow(&mut map, 0xCF8);
        assert!(is_allowed(&map, 0xCF8));
        assert!(!is_allowed(&map, 0xCF9));
        assert!(!is_allowed(&map, 0x0000));
        assert!(!is_allowed(&map, 0xFFFF));
        assert_eq!(allowed_count(&map), 1);
    }

    #[test]
    fn ranges_saturate_at_top_of_space() {
        let mut map = denied();
        allow_range(&mut map, 0xCF8, 8);
        for port in 0xCF8..0xD00 {
            assert!(is_allowed(&map, port));
        }
        assert!(!is_allowed(&map, 0xCF7));
        assert!(!is_allowed(&map, 0xD00));
        assert_eq!(allowed_count(&map), 8);

        allow_range(&mut map, 0xFFFE, 10);
        assert!(is_allowed(&map, 0xFFFE));
        assert!(is_allowed(&map, 0xFFFF));
        assert_eq!(allowed_count(&map), 10);
    }

    #[test]
    fn deny_all_by_default() {
        let map = denied();
        assert_eq!(allowed_count(&map), 0);
        assert!(!is_allowed(&map, 0x6080));
    }

    #[test]
    fn deny_all_resets_a_granted_map() {
        let mut map = denied();
        allow_range(&mut map, 0x60, 2);
        allow_range(&mut map, 0x64, 1);
        assert_eq!(allowed_count(&map), 3);

        deny_all(&mut map);
        assert_eq!(allowed_count(&map), 0);
        for port in [0x60, 0x61, 0x64] {
            assert!(!is_allowed(&map, port));
        }
    }

    #[test]
    fn deny_range_removes_only_its_own_bits() {
        let mut map = denied();
        allow_range(&mut map, 0x60, 8);
        deny_range(&mut map, 0x61, 2);
        assert!(is_allowed(&map, 0x60));
        assert!(!is_allowed(&map, 0x61));
        assert!(!is_allowed(&map, 0x62));
        assert!(is_allowed(&map, 0x63));
        assert!(is_allowed(&map, 0x67));
        assert_eq!(allowed_count(&map), 6);

        // Re-denying is idempotent, which matters when a domain exits twice.
        deny_range(&mut map, 0x61, 2);
        assert_eq!(allowed_count(&map), 6);
        deny_range(&mut map, 0xFFFE, 10);
        assert_eq!(allowed_count(&map), 6);
    }

    #[test]
    fn two_maps_stay_independent() {
        let mut driver = denied();
        let mut shell = denied();
        allow_range(&mut driver, 0xCF8, 8);
        deny_all(&mut shell);
        assert!(is_allowed(&driver, 0xCF8));
        assert!(!is_allowed(&shell, 0xCF8));
        assert_eq!(allowed_count(&shell), 0);
    }

    type Ports = TaskPorts<8>;

    #[test]
    fn a_fresh_table_authorises_nothing() {
        let ports = Ports::new();
        assert_eq!(ports.allowed_count(0), 0);
        let mut map = denied();
        ports.apply_to(0, &mut map);
        assert_eq!(allowed_count(&map), 0);
    }

    #[test]
    fn grants_project_only_onto_their_own_task() {
        let mut ports = Ports::new();
        assert!(ports.grant(4, 0xCF8, 8));
        assert!(ports.grant(4, 0xC000, 0x100));
        assert_eq!(ports.allowed_count(4), 264);

        let mut map = denied();
        ports.apply_to(4, &mut map);
        assert_eq!(allowed_count(&map), 264);
        assert!(is_allowed(&map, 0xCF8));
        assert!(is_allowed(&map, 0xC0FF));

        // A different task sees none of it: this is the whole point.
        ports.apply_to(2, &mut map);
        assert_eq!(allowed_count(&map), 0);
        assert!(!is_allowed(&map, 0xCF8));

        // ...and coming back restores exactly what it had.
        ports.apply_to(4, &mut map);
        assert_eq!(allowed_count(&map), 264);
    }

    #[test]
    fn a_revoked_task_loses_every_port() {
        let mut ports = Ports::new();
        ports.grant(5, 0x60, 2);
        ports.grant(5, 0x64, 1);
        assert_eq!(ports.allowed_count(5), 3);

        assert_eq!(ports.revoke(5), 3);
        assert_eq!(ports.allowed_count(5), 0);
        let mut map = denied();
        ports.apply_to(5, &mut map);
        assert_eq!(allowed_count(&map), 0);

        // The slot is reusable after a release.
        assert!(ports.grant(5, 0x60, 2));
        ports.apply_to(5, &mut map);
        assert_eq!(allowed_count(&map), 2);
        assert!(!is_allowed(&map, 0x64));
    }

    #[test]
    fn grants_are_bounded_and_fail_closed() {
        let mut ports = Ports::new();
        for index in 0..MAX_TASK_PORTS {
            assert!(ports.grant(0, 0x10 * (index as u16 + 1), 1));
        }
        // One grant too many is refused rather than silently widening rights.
        assert!(!ports.grant(0, 0xFF, 1));
        assert_eq!(ports.allowed_count(0), MAX_TASK_PORTS);
        let mut map = denied();
        ports.apply_to(0, &mut map);
        assert!(!is_allowed(&map, 0xFF));
        assert_eq!(allowed_count(&map), MAX_TASK_PORTS);
    }

    #[test]
    fn empty_and_out_of_range_grants_are_refused() {
        let mut ports = Ports::new();
        assert!(!ports.grant(0, 0x60, 0));
        assert!(!ports.grant(99, 0x60, 1));
        assert_eq!(ports.allowed_count(0), 0);
        assert_eq!(ports.allowed_count(99), 0);
        assert_eq!(ports.revoke(99), 0);
        // An out-of-range task must not inherit the last task's ports.
        let mut map = denied();
        ports.grant(1, 0x60, 2);
        ports.apply_to(1, &mut map);
        assert!(is_allowed(&map, 0x60));
        ports.apply_to(99, &mut map);
        assert_eq!(allowed_count(&map), 0);
    }
}
