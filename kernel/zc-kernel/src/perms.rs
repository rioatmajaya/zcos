//! POSIX-style file permission checks.
//!
//! Capabilities remain the kernel's security boundary: a task can only reach
//! a filesystem at all because it holds the authority to make the syscall.
//! Permission bits are a *policy* layer on top of that boundary, exactly as
//! the security reference separates the two. The VFS calls [`check`] with the
//! caller's [`Identity`] and the node's owner and mode; the filesystem itself
//! never sees the caller.
//!
//! The rule is the classic three-class one: a caller matching the file's user
//! is judged by the owner bits, otherwise a caller matching the file's group
//! is judged by the group bits, otherwise by the other bits. The classes are
//! **not** unioned — a matching owner with no owner-read bit is denied even
//! when the other bits would grant access. Root (uid 0) bypasses the check.
//!
//! Everything here is pure and allocation-free, so it runs on the host under
//! test and in the kernel unchanged.

use crate::vfs::VfsError;

/// A task's user and group identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    /// User id; `0` is root.
    pub uid: u32,
    /// Primary group id.
    pub gid: u32,
}

impl Identity {
    /// The root identity, which bypasses permission checks.
    pub const ROOT: Self = Self { uid: 0, gid: 0 };

    /// Builds an identity from a user and group id.
    #[must_use]
    pub const fn new(uid: u32, gid: u32) -> Self {
        Self { uid, gid }
    }

    /// Returns whether this is the root identity.
    #[must_use]
    pub const fn is_root(self) -> bool {
        self.uid == 0
    }
}

/// One kind of access a filesystem operation needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    /// Read a file's bytes.
    Read,
    /// Write a file's bytes or create a name in a directory.
    Write,
    /// Search a directory (walk through it to reach a name).
    Search,
}

/// Read bit in a permission class.
pub const PERM_READ: u32 = 0o4;
/// Write bit in a permission class.
pub const PERM_WRITE: u32 = 0o2;
/// Execute/search bit in a permission class.
pub const PERM_EXEC: u32 = 0o1;

/// The permission bits of a mode: everything below the type bits.
pub const PERM_BITS: u32 = 0o777;

/// Returns the permission bits of `mode`, with any type bits masked off.
#[must_use]
pub const fn perm_bits(mode: u32) -> u32 {
    mode & PERM_BITS
}

/// Returns the bit shift for the class `owner` falls into for a file.
const fn class_shift(file_uid: u32, file_gid: u32, owner: Identity) -> u32 {
    if owner.uid == file_uid {
        6
    } else if owner.gid == file_gid {
        3
    } else {
        0
    }
}

/// Returns the permission bit `access` needs.
const fn access_bit(access: Access) -> u32 {
    match access {
        Access::Read => PERM_READ,
        Access::Write => PERM_WRITE,
        Access::Search => PERM_EXEC,
    }
}

/// Checks whether `owner` may perform `access` on a node owned by
/// `file_uid`/`file_gid` with permission bits in `mode`.
///
/// Returns [`VfsError::PermissionDenied`] when the matching class lacks the
/// bit. Root always passes.
pub fn check(
    file_uid: u32,
    file_gid: u32,
    mode: u32,
    owner: Identity,
    access: Access,
) -> Result<(), VfsError> {
    if owner.is_root() {
        return Ok(());
    }
    let shift = class_shift(file_uid, file_gid, owner);
    if (perm_bits(mode) >> shift) & access_bit(access) != 0 {
        Ok(())
    } else {
        Err(VfsError::PermissionDenied)
    }
}

/// Returns whether `owner` may change a node's mode.
///
/// Only root and the node's own user may; a group match is not enough.
#[must_use]
pub const fn may_set_mode(file_uid: u32, owner: Identity) -> bool {
    owner.is_root() || owner.uid == file_uid
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file owned by uid 1, gid 2, with the given permission bits.
    fn file(mode: u32) -> (u32, u32, u32) {
        (1, 2, mode)
    }

    #[test]
    fn root_bypasses_every_check() {
        // Even a mode with no bits set does not stop root.
        let (uid, gid, mode) = file(0o000);
        for access in [Access::Read, Access::Write, Access::Search] {
            assert_eq!(check(uid, gid, mode, Identity::ROOT, access), Ok(()));
        }
    }

    #[test]
    fn the_owner_class_grants_and_denies() {
        let owner = Identity::new(1, 9);
        assert_eq!(
            check(1, 2, 0o400, owner, Access::Read),
            Ok(())
        );
        assert_eq!(
            check(1, 2, 0o200, owner, Access::Write),
            Ok(())
        );
        assert_eq!(
            check(1, 2, 0o100, owner, Access::Search),
            Ok(())
        );
        // A mode without the owner bit denies the owner.
        assert_eq!(
            check(1, 2, 0o000, owner, Access::Read),
            Err(VfsError::PermissionDenied)
        );
        // A file with only group/other read still denies the owner: the
        // classes are not unioned.
        assert_eq!(
            check(1, 2, 0o044, owner, Access::Read),
            Err(VfsError::PermissionDenied)
        );
    }

    #[test]
    fn a_non_owner_group_member_uses_the_group_bits() {
        let member = Identity::new(7, 2);
        assert_eq!(check(1, 2, 0o040, member, Access::Read), Ok(()));
        assert_eq!(check(1, 2, 0o020, member, Access::Write), Ok(()));
        assert_eq!(
            check(1, 2, 0o400, member, Access::Read),
            Err(VfsError::PermissionDenied)
        );
        // The owner's own bits do not leak to a group member.
        assert_eq!(
            check(1, 2, 0o600, member, Access::Read),
            Err(VfsError::PermissionDenied)
        );
    }

    #[test]
    fn everyone_else_uses_the_other_bits() {
        let stranger = Identity::new(7, 9);
        assert_eq!(check(1, 2, 0o004, stranger, Access::Read), Ok(()));
        assert_eq!(check(1, 2, 0o002, stranger, Access::Write), Ok(()));
        assert_eq!(check(1, 2, 0o001, stranger, Access::Search), Ok(()));
        assert_eq!(
            check(1, 2, 0o600, stranger, Access::Read),
            Err(VfsError::PermissionDenied)
        );
    }

    #[test]
    fn a_matching_uid_wins_over_a_matching_gid() {
        // The caller is both the owner and in the file's group; the owner
        // class decides, so a group-write bit does not grant the owner.
        let caller = Identity::new(1, 2);
        assert_eq!(check(1, 2, 0o020, caller, Access::Write), Err(VfsError::PermissionDenied));
        assert_eq!(check(1, 2, 0o200, caller, Access::Write), Ok(()));
    }

    #[test]
    fn type_bits_do_not_affect_permission_checks() {
        // A directory mode carries type bits above 0o777; they are masked off.
        let owner = Identity::new(1, 2);
        assert_eq!(check(1, 2, 0o040_755, owner, Access::Search), Ok(()));
        assert_eq!(check(1, 2, 0o040_755, owner, Access::Read), Ok(()));
    }

    #[test]
    fn only_root_or_the_owner_may_change_a_mode() {
        assert!(may_set_mode(1, Identity::ROOT));
        assert!(may_set_mode(1, Identity::new(1, 9)));
        assert!(!may_set_mode(1, Identity::new(7, 2)));
        assert!(!may_set_mode(1, Identity::new(7, 9)));
    }

    #[test]
    fn perm_bits_masks_the_type_bits() {
        assert_eq!(perm_bits(0o100_644), 0o644);
        assert_eq!(perm_bits(0o040_755), 0o755);
        assert_eq!(perm_bits(0o000), 0);
    }
}
