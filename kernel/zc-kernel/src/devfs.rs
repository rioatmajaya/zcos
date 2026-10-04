//! Device filesystem: the VFS face of a hardware device.
//!
//! A device is not a file that holds bytes — it is a name you open to reach
//! hardware. [`DevFs`] gives each supervised service a node in the filesystem
//! so a task can discover what exists through the same path walk it already
//! uses, instead of a bespoke enumeration channel.
//!
//! The nodes are descriptive in this phase. [`FileSystem::stat`] reports
//! [`zc_abi::KIND_CHR`] and [`FileSystem::read`] returns end-of-file, because
//! the bytes a device produces are the driver's business, not the
//! filesystem's: the block domain already owns the virtio window and the
//! cache, and routing data through a mount would give the filesystem a second
//! path to hardware it has no authority over. Wiring an actual read or write to
//! a device node means handing the mount a capability-backed driver handle,
//! which belongs with the I/O path rather than with the namespace.
//!
//! Entries come from [`crate::service::SERVICES`], the one table that already
//! names every domain, so a new service appears here automatically and the
//! filesystem cannot drift from the bring-up layout.

use crate::service::{SERVICES, Service};
use crate::vfs::{FileSystem, NodeId, VfsError};
use zc_abi::{KIND_CHR, Stat};

/// Node id of the root directory.
pub const ROOT: NodeId = 0;

/// Mode reported for a device node: owner read and write, no group or other.
const CHR_MODE: u32 = 0o600;

/// Mode reported for the root directory.
const DIR_MODE: u32 = 0o755;

/// The device filesystem: one node per supervised service.
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
    /// Node ids are the index into [`SERVICES`] plus one, so `0` stays the
    /// root and the mapping needs no table of its own.
    #[must_use]
    const fn service(node: NodeId) -> Option<Service> {
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

    /// Returns how many device nodes the filesystem publishes.
    #[must_use]
    pub const fn entry_count(&self) -> usize {
        SERVICES.len()
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
        // Names are the service's own, so the node and the log name can never
        // disagree; the first match wins, and the table has no duplicates.
        SERVICES
            .iter()
            .position(|service| service.name.as_bytes() == name)
            .map(|index| index as NodeId + 1)
            .ok_or(VfsError::NotFound)
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
        Self::service(node).ok_or(VfsError::NotFound)?;
        Ok(Stat {
            kind: KIND_CHR,
            mode: CHR_MODE,
            // A device has no length of its own: how many bytes a read yields
            // is the driver's answer, not a property of the node.
            size: 0,
            node,
            uid: 0,
            gid: 0,
        })
    }

    fn read(&self, node: NodeId, _offset: u64, _out: &mut [u8]) -> Result<usize, VfsError> {
        // End-of-file until a driver is attached: the node exists and can be
        // opened, but the filesystem has no bytes to hand over.
        Self::service(node).ok_or(VfsError::NotFound)?;
        Ok(0)
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
        assert_eq!(fs.entry_count(), SERVICES.len());

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
    fn device_nodes_are_read_only() {
        let fs = DevFs::new();
        let node = fs.lookup(ROOT, b"blk").expect("blk node");
        // Writing a device or inventing one is refused: the table of services
        // is the only source of nodes.
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
        let beyond = fs.entry_count() as NodeId + 1;
        assert_eq!(fs.stat(beyond), Err(VfsError::NotFound));
        assert_eq!(fs.stat(999), Err(VfsError::NotFound));
        let mut buffer = [0u8; 4];
        assert_eq!(fs.read(beyond, 0, &mut buffer), Err(VfsError::NotFound));
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
        assert_eq!(fs.entry_count(), 2);
        assert_eq!(SERVICES[0].id, BLK_SERVICE);
        // The keyboard domain publishes a node too, so `cat /dev/kbd` opens.
        assert_eq!(fs.lookup(ROOT, b"kbd"), Ok(2));
    }
}