//! Device filesystem: the VFS face of hardware and pseudo-devices.
//!
//! A device is not a file that holds bytes — it is a name you open to reach
//! hardware or a kernel-synthesized character stream. [`DevFs`] gives each
//! supervised service a node in the filesystem so a task can discover what exists
//! through the same path walk it already uses, instead of a bespoke enumeration
//! channel.
//!
//! Standard character devices are also provided:
//! - `/dev/null`: reads yield EOF (0 bytes); writes discard all data.
//! - `/dev/zero`: reads yield infinite zero bytes; writes discard all data.
//! - `/dev/urandom`: reads yield pseudo-random bytes; writes discard all data.
//! All three pseudo-devices report `0o666` permissions so any user can read
//! or write them without privilege.
//!
//! Service nodes (`/dev/blk`, `/dev/kbd`) remain descriptive: they report
//! `KIND_CHR` and `0o600` permissions. Reads return EOF and writes return
//! `NotSupported` because device I/O is capability-routed rather than mounted.
//!
//! Entries come from [`crate::service::SERVICES`], the one table that already
//! names every domain, followed by the standard character devices.

use crate::service::{SERVICES, Service};
use crate::vfs::{FileSystem, NodeId, VfsError};
use zc_abi::{KIND_CHR, Stat};

/// Node id of the root directory.
pub const ROOT: NodeId = 0;

/// Mode reported for a service device node: owner read and write, no group or other.
const CHR_MODE: u32 = 0o600;

/// Mode reported for public pseudo-devices: read and write for all users.
const PUBLIC_CHR_MODE: u32 = 0o666;

/// Mode reported for the root directory.
const DIR_MODE: u32 = 0o755;

/// Node id of `/dev/null`.
pub const NODE_NULL: NodeId = 100;

/// Node id of `/dev/zero`.
pub const NODE_ZERO: NodeId = 101;

/// Node id of `/dev/urandom`.
pub const NODE_URANDOM: NodeId = 102;

/// Special character device descriptors: (name, node).
pub const SPECIAL_DEVICES: [(&[u8], NodeId); 3] = [
    (b"null", NODE_NULL),
    (b"zero", NODE_ZERO),
    (b"urandom", NODE_URANDOM),
];

/// The device filesystem: supervised services plus standard pseudo-devices.
///
/// The struct holds no state of its own, so it is `Copy` and a single `static`
/// instance can serve every task.
#[derive(Clone, Copy)]
pub struct DevFs;

impl DevFs {
    /// Creates the filesystem.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Returns the service published as node `node`.
    ///
    /// Service node ids are the index into [`SERVICES`] plus one, so `0` stays the
    /// root, `1..=SERVICES.len()` are services, and special nodes sit past them.
    #[must_use]
    pub const fn service(node: NodeId) -> Option<Service> {
        if node == 0 {
            return None;
        }
        let index = (node - 1) as usize;
        if index < SERVICES.len() {
            Some(SERVICES[index])
        } else {
            None
        }
    }

    /// Returns whether `node` is one of the standard pseudo-devices.
    #[must_use]
    pub const fn is_special(node: NodeId) -> bool {
        node == NODE_NULL || node == NODE_ZERO || node == NODE_URANDOM
    }

    /// Returns how many device nodes the filesystem publishes.
    #[must_use]
    pub const fn entry_count(&self) -> usize {
        SERVICES.len() + SPECIAL_DEVICES.len()
    }
}

impl Default for DevFs {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystem for DevFs {
    fn name(&self) -> &str {
        "devfs"
    }

    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(&self, dir: NodeId, name: &[u8]) -> Result<NodeId, VfsError> {
        if dir != ROOT {
            return Err(VfsError::NotADirectory);
        }
        if name.is_empty() {
            return Err(VfsError::BadPath);
        }
        // Service names take precedence and match in catalog order.
        if let Some(index) = SERVICES
            .iter()
            .position(|service| service.name.as_bytes() == name)
        {
            return Ok(index as NodeId + 1);
        }
        for &(special_name, node) in &SPECIAL_DEVICES {
            if special_name == name {
                return Ok(node);
            }
        }
        Err(VfsError::NotFound)
    }

    fn stat(&self, node: NodeId) -> Result<Stat, VfsError> {
        if node == ROOT {
            return Ok(Stat {
                kind: zc_abi::KIND_DIR,
                mode: DIR_MODE,
                size: 0,
                node,
                // Device nodes are kernel-owned; the mount has no writer.
                uid: 0,
                gid: 0,
            });
        }
        if Self::service(node).is_some() {
            return Ok(Stat {
                kind: KIND_CHR,
                mode: CHR_MODE,
                size: 0,
                node,
                uid: 0,
                gid: 0,
            });
        }
        if Self::is_special(node) {
            return Ok(Stat {
                kind: KIND_CHR,
                mode: PUBLIC_CHR_MODE,
                size: 0,
                node,
                uid: 0,
                gid: 0,
            });
        }
        Err(VfsError::NotFound)
    }

    fn read(&self, node: NodeId, offset: u64, out: &mut [u8]) -> Result<usize, VfsError> {
        if Self::service(node).is_some() {
            // End-of-file until a driver is attached.
            return Ok(0);
        }
        match node {
            NODE_NULL => Ok(0),
            NODE_ZERO => {
                out.fill(0);
                Ok(out.len())
            }
            NODE_URANDOM => {
                let mut state = offset.wrapping_add(0x9E37_79B9_7F4A_7C15);
                for byte in out.iter_mut() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    *byte = (state & 0xFF) as u8;
                }
                Ok(out.len())
            }
            _ => Err(VfsError::NotFound),
        }
    }

    fn write(&self, node: NodeId, _offset: u64, data: &[u8]) -> Result<usize, VfsError> {
        if Self::service(node).is_some() {
            return Err(VfsError::NotSupported);
        }
        if Self::is_special(node) {
            // Writing to /dev/null, /dev/zero, or /dev/urandom discards all data.
            return Ok(data.len());
        }
        Err(VfsError::NotFound)
    }

    fn read_dir(
        &self,
        dir: NodeId,
        index: usize,
        out: &mut [u8],
    ) -> Result<Option<()>, VfsError> {
        if dir != ROOT {
            return Err(VfsError::NotADirectory);
        }
        if let Some(service) = SERVICES.get(index) {
            let mut entry = zc_abi::DirEntry::new(KIND_CHR);
            entry
                .set_name(service.name.as_bytes())
                .ok_or(VfsError::BadBuffer)?;
            entry.write_into(out).ok_or(VfsError::BadBuffer)?;
            return Ok(Some(()));
        }
        let special_index = index.saturating_sub(SERVICES.len());
        if let Some(&(name, _)) = SPECIAL_DEVICES.get(special_index) {
            let mut entry = zc_abi::DirEntry::new(KIND_CHR);
            entry
                .set_name(name)
                .ok_or(VfsError::BadBuffer)?;
            entry.write_into(out).ok_or(VfsError::BadBuffer)?;
            return Ok(Some(()));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::BLK_SERVICE;

    #[test]
    fn the_block_domain_has_a_node() {
        let fs = DevFs::new();
        assert_eq!(fs.name(), "devfs");
        assert_eq!(fs.root(), ROOT);
        assert_eq!(fs.entry_count(), SERVICES.len() + SPECIAL_DEVICES.len());

        // The node the roadmap names for the block domain resolves by name.
        let node = fs.lookup(ROOT, b"blk").expect("blk node");
        assert_ne!(node, ROOT);
        // Looking it up again returns the same node, so a path walk converges.
        assert_eq!(fs.lookup(ROOT, b"blk"), Ok(node));
        assert_eq!(fs.lookup(ROOT, b"missing"), Err(VfsError::NotFound));
        assert_eq!(fs.lookup(ROOT, b""), Err(VfsError::BadPath));
        // A device node is not a directory.
        assert_eq!(fs.lookup(node, b"blk"), Err(VfsError::NotADirectory));
    }

    #[test]
    fn a_device_node_reports_kind_chr_and_no_length() {
        let fs = DevFs::new();
        let node = fs.lookup(ROOT, b"blk").expect("blk node");
        let stat = fs.stat(node).expect("stat");
        assert_eq!(stat.kind, KIND_CHR);
        assert_eq!(stat.size, 0);
        assert_eq!(stat.mode, CHR_MODE);
        assert_eq!(stat.node, node);

        let root = fs.stat(ROOT).expect("root");
        assert_eq!(root.kind, zc_abi::KIND_DIR);
        assert_eq!(root.size, 0);
    }

    #[test]
    fn reading_a_device_is_end_of_file() {
        let fs = DevFs::new();
        let node = fs.lookup(ROOT, b"blk").expect("blk node");
        let mut buffer = [0u8; 16];
        // A descriptive node opens and reads as empty rather than failing, so
        // `cat /dev/blk` terminates cleanly instead of hanging the shell.
        assert_eq!(fs.read(node, 0, &mut buffer), Ok(0));
        assert_eq!(fs.read(node, 5, &mut buffer), Ok(0));
        assert_eq!(&buffer, &[0u8; 16]);
    }

    #[test]
    fn service_nodes_are_read_only() {
        let fs = DevFs::new();
        let node = fs.lookup(ROOT, b"blk").expect("blk node");
        // Writing a service device directly without capability is refused.
        assert_eq!(fs.write(node, 0, b"x"), Err(VfsError::NotSupported));
        assert_eq!(
            fs.create(ROOT, b"newdev", 0o644, crate::perms::Identity::ROOT),
            Err(VfsError::NotSupported)
        );
        assert_eq!(fs.lookup(ROOT, b"newdev"), Err(VfsError::NotFound));
    }

    #[test]
    fn unknown_nodes_are_not_found() {
        let fs = DevFs::new();
        let beyond = 999;
        assert_eq!(fs.stat(beyond), Err(VfsError::NotFound));
        let mut buffer = [0u8; 4];
        assert_eq!(fs.read(beyond, 0, &mut buffer), Err(VfsError::NotFound));
        assert_eq!(fs.write(beyond, 0, b"test"), Err(VfsError::NotFound));
    }

    #[test]
    fn every_service_publishes_exactly_one_node() {
        let fs = DevFs::new();
        // Each service in the bring-up table is reachable, and its node points
        // back at the same service, so /dev cannot disagree with the kernel.
        for (index, service) in SERVICES.iter().enumerate() {
            let node = fs.lookup(ROOT, service.name.as_bytes()).expect("node");
            assert_eq!(node, index as NodeId + 1);
            assert_eq!(DevFs::service(node), Some(*service));
        }
        assert_eq!(SERVICES[0].id, BLK_SERVICE);
        // The keyboard domain publishes a node too, so `cat /dev/kbd` opens.
        assert_eq!(fs.lookup(ROOT, b"kbd"), Ok(2));
    }

    #[test]
    fn read_dir_lists_the_service_table_and_special_devices() {
        let fs = DevFs::new();
        let mut out = [0u8; zc_abi::DIRENT_LEN];
        let total = SERVICES.len() + SPECIAL_DEVICES.len();
        for index in 0..total {
            assert_eq!(fs.read_dir(ROOT, index, &mut out), Ok(Some(())));
            let entry = zc_abi::DirEntry::read_from(&out).expect("entry");
            assert_eq!(entry.kind, KIND_CHR);
            if index < SERVICES.len() {
                assert_eq!(entry.name(), SERVICES[index].name.as_bytes());
            } else {
                assert_eq!(entry.name(), SPECIAL_DEVICES[index - SERVICES.len()].0);
            }
        }
        assert_eq!(fs.read_dir(ROOT, total, &mut out), Ok(None));
        assert_eq!(
            fs.read_dir(1, 0, &mut out),
            Err(VfsError::NotADirectory)
        );
    }

    #[test]
    fn null_device_reads_eof_and_discards_writes() {
        let fs = DevFs::new();
        let node = fs.lookup(ROOT, b"null").expect("null node");
        assert_eq!(node, NODE_NULL);

        let stat = fs.stat(node).expect("stat");
        assert_eq!(stat.kind, KIND_CHR);
        assert_eq!(stat.mode, PUBLIC_CHR_MODE);

        let mut buf = [0xAAu8; 16];
        assert_eq!(fs.read(node, 0, &mut buf), Ok(0));
        assert_eq!(buf, [0xAAu8; 16]); // Unchanged, EOF

        assert_eq!(fs.write(node, 0, b"hello null"), Ok(10));
    }

    #[test]
    fn zero_device_reads_zeros_and_discards_writes() {
        let fs = DevFs::new();
        let node = fs.lookup(ROOT, b"zero").expect("zero node");
        assert_eq!(node, NODE_ZERO);

        let stat = fs.stat(node).expect("stat");
        assert_eq!(stat.kind, KIND_CHR);
        assert_eq!(stat.mode, PUBLIC_CHR_MODE);

        let mut buf = [0xFFu8; 8];
        assert_eq!(fs.read(node, 0, &mut buf), Ok(8));
        assert_eq!(buf, [0u8; 8]);

        assert_eq!(fs.write(node, 0, b"discard me"), Ok(10));
    }

    #[test]
    fn urandom_device_reads_prng_and_discards_writes() {
        let fs = DevFs::new();
        let node = fs.lookup(ROOT, b"urandom").expect("urandom node");
        assert_eq!(node, NODE_URANDOM);

        let stat = fs.stat(node).expect("stat");
        assert_eq!(stat.kind, KIND_CHR);
        assert_eq!(stat.mode, PUBLIC_CHR_MODE);

        let mut buf1 = [0u8; 16];
        let mut buf2 = [0u8; 16];
        assert_eq!(fs.read(node, 0, &mut buf1), Ok(16));
        assert_eq!(fs.read(node, 16, &mut buf2), Ok(16));
        assert_ne!(buf1, [0u8; 16]);
        assert_ne!(buf1, buf2); // Different offsets produce different sequences

        assert_eq!(fs.write(node, 0, b"entropy"), Ok(7));
    }
}
