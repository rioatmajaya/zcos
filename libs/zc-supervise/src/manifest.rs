//! Parser for the boot manifest, `zc.manifest`.
//!
//! The manifest is the one file that names the boot: a small `key=value` text
//! file in the initramfs. The kernel still spawns the compile-time
//! [`SERVICE_CATALOG`], because it must build address spaces and provision
//! capabilities before any task runs. This parser lets `initd` read the
//! manifest at boot and check that the services it names match the catalog in
//! boot order, so a stale or malformed manifest is visible in the transcript
//! instead of being silently ignored.
//!
//! The grammar is deliberately tiny so it can be parsed without allocation:
//!
//! - one `key=value` pair per line;
//! - blank lines and lines whose first non-space byte is `#` are ignored;
//! - keys and values are trimmed;
//! - `services` holds a comma-separated list in boot order.
//!
//! Known keys are `name`, `version`, `init`, and `services`. An unknown or
//! duplicate key is an error: a typo in a boot file should be reported, not
//! skipped.

use crate::{SERVICE_CATALOG, ServiceSpec};

/// The init path a manifest must name for [`Manifest::matches_boot`] to pass.
pub const DEFAULT_INIT: &str = "/sbin/initd";

/// Largest number of service names one manifest may list.
///
/// The catalog is far smaller; the bound keeps the parsed value in a fixed
/// array and rejects a runaway boot file before it can consume stack.
pub const MAX_SERVICES: usize = 8;

/// Why a boot manifest could not be parsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestError {
    /// A non-comment, non-blank line has no `=`.
    MissingEquals,
    /// The key is not one the boot manifest defines.
    UnknownKey,
    /// The same key appears more than once.
    DuplicateKey,
    /// A key has an empty value.
    EmptyValue,
    /// `services` lists more than [`MAX_SERVICES`] names.
    TooManyServices,
    /// `services` contains an empty name, such as `blk,,kbd`.
    EmptyServiceName,
    /// `services` names the same service twice.
    DuplicateService,
}

/// A parsed boot manifest.
///
/// The value borrows from the text passed to [`Manifest::parse`], so the
/// caller keeps the buffer alive for as long as it uses the manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Manifest<'a> {
    name: Option<&'a str>,
    version: Option<&'a str>,
    init: Option<&'a str>,
    services: [&'a str; MAX_SERVICES],
    service_count: usize,
}

impl<'a> Manifest<'a> {
    /// Parses one manifest.
    ///
    /// # Errors
    ///
    /// Returns the first structural problem in [`ManifestError`]; the caller
    /// can log it but cannot recover a partial manifest.
    pub fn parse(text: &'a str) -> Result<Self, ManifestError> {
        let mut manifest = Self {
            name: None,
            version: None,
            init: None,
            services: [""; MAX_SERVICES],
            service_count: 0,
        };

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(ManifestError::MissingEquals);
            };
            let key = key.trim();
            let value = value.trim();
            if value.is_empty() {
                return Err(ManifestError::EmptyValue);
            }
            match key {
                "name" => set_once(&mut manifest.name, value)?,
                "version" => set_once(&mut manifest.version, value)?,
                "init" => set_once(&mut manifest.init, value)?,
                "services" => manifest.push_services(value)?,
                _ => return Err(ManifestError::UnknownKey),
            }
        }

        Ok(manifest)
    }

    /// Returns the manifest's display name, if it names one.
    #[must_use]
    pub const fn name(&self) -> Option<&'a str> {
        self.name
    }

    /// Returns the manifest's version, if it names one.
    #[must_use]
    pub const fn version(&self) -> Option<&'a str> {
        self.version
    }

    /// Returns the init path, if the manifest names one.
    #[must_use]
    pub const fn init(&self) -> Option<&'a str> {
        self.init
    }

    /// Returns the service names in boot order.
    #[must_use]
    pub fn services(&self) -> &[&'a str] {
        &self.services[..self.service_count]
    }

    /// Returns whether the named services are a subsequence of the catalog.
    ///
    /// A subset is allowed — the manifest may supervise fewer services than
    /// the catalog lists — but the relative order must still match the
    /// compile-time boot order, because the kernel provisions the address
    /// spaces and capabilities in catalog order. Reordering here would make
    /// the file lie about the boot.
    ///
    /// Names, not ids, are what the file carries: an id is an ABI detail and a
    /// person editing the manifest should never have to look one up.
    #[must_use]
    pub fn names_catalog_in_order(&self) -> bool {
        names_are_subsequence(self, &SERVICE_CATALOG)
    }

    /// Returns whether the manifest names the default init and a valid subset.
    ///
    /// This is the check `initd` makes at boot; it deliberately does not
    /// require informational keys such as `name` or `version`. A manifest that
    /// passes is authoritative for the supervised set and order; one that does
    /// not falls back to the full catalog rather than leaving services
    /// unsupervised. A manifest with no `services` line is not authoritative.
    #[must_use]
    pub fn matches_boot(&self) -> bool {
        self.init == Some(DEFAULT_INIT)
            && !self.services().is_empty()
            && self.names_catalog_in_order()
    }

    /// Appends the comma-separated names in `value` to the service list.
    ///
    /// The parser stays structural: it does not resolve names against the
    /// catalog, so it can be host-tested and reused if the catalog changes.
    /// [`matches_boot`](Self::matches_boot) is where a name is bound to the
    /// catalog, and an unknown name simply fails the subsequence check.
    fn push_services(&mut self, value: &'a str) -> Result<(), ManifestError> {
        for name in value.split(',') {
            let name = name.trim();
            if name.is_empty() {
                return Err(ManifestError::EmptyServiceName);
            }
            if self.services().contains(&name) {
                return Err(ManifestError::DuplicateService);
            }
            if self.service_count == MAX_SERVICES {
                return Err(ManifestError::TooManyServices);
            }
            self.services[self.service_count] = name;
            self.service_count += 1;
        }
        Ok(())
    }
}

/// Stores `value` in `slot`, refusing a second assignment.
fn set_once<'a>(slot: &mut Option<&'a str>, value: &'a str) -> Result<(), ManifestError> {
    if slot.is_some() {
        return Err(ManifestError::DuplicateKey);
    }
    *slot = Some(value);
    Ok(())
}

/// Convenience for callers that already hold a catalog slice.
///
/// [`Manifest::names_catalog_in_order`] uses the ABI catalog; this helper lets
/// a test name an arbitrary catalog without rebuilding a manifest.
#[must_use]
pub fn names_are_subsequence(manifest: &Manifest<'_>, catalog: &[ServiceSpec]) -> bool {
    let mut catalog = catalog.iter();
    for name in manifest.services() {
        loop {
            match catalog.next() {
                Some(service) if service.name == *name => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = include_str!("../../../initramfs/zc.manifest");

    #[test]
    fn the_shipped_manifest_parses_and_matches_the_catalog() {
        let manifest = Manifest::parse(SHIPPED).expect("shipped manifest");
        assert_eq!(manifest.name(), Some("ZC OS"));
        assert_eq!(manifest.version(), Some("0.1.0"));
        assert_eq!(manifest.init(), Some(DEFAULT_INIT));
        assert_eq!(manifest.services(), &["blk", "kbd"]);
        assert!(manifest.names_catalog_in_order());
        assert!(manifest.matches_boot());
    }

    #[test]
    fn comments_blank_lines_and_spacing_are_ignored() {
        let text = "\n  # boot file\n name = ZC OS \ninit=/sbin/initd\nservices= blk , kbd \n";
        let manifest = Manifest::parse(text).expect("manifest");
        assert_eq!(manifest.name(), Some("ZC OS"));
        assert_eq!(manifest.services(), &["blk", "kbd"]);
        assert!(manifest.matches_boot());
    }

    #[test]
    fn a_wrong_order_is_not_a_subsequence() {
        let text = "init=/sbin/initd\nservices=kbd,blk\n";
        let manifest = Manifest::parse(text).expect("manifest");
        assert!(!manifest.names_catalog_in_order());
        assert!(!manifest.matches_boot());
        // The names are all present; only the order differs, and the kernel
        // still provisions them in catalog order.
        assert_eq!(manifest.services().len(), SERVICE_CATALOG.len());
    }

    #[test]
    fn a_subset_is_a_valid_boot_but_an_unknown_name_is_not() {
        // Fewer services than the catalog is allowed: the manifest is
        // authoritative for the supervised set.
        let subset = Manifest::parse("init=/sbin/initd\nservices=blk\n").unwrap();
        assert!(subset.names_catalog_in_order());
        assert!(subset.matches_boot());
        assert_eq!(subset.services(), &["blk"]);

        // A name the catalog does not list breaks the subsequence.
        let unknown = Manifest::parse("init=/sbin/initd\nservices=blk,net\n").unwrap();
        assert!(!unknown.names_catalog_in_order());
        assert!(!unknown.matches_boot());

        // No services line at all is not authoritative, even with the right
        // init path: an empty list would otherwise pass vacuously.
        let none = Manifest::parse("init=/sbin/initd\n").unwrap();
        assert!(none.services().is_empty());
        assert!(!none.matches_boot());
    }

    #[test]
    fn the_wrong_init_is_a_mismatch() {
        let manifest = Manifest::parse("init=/bin/sh\nservices=blk,kbd\n").unwrap();
        assert!(manifest.names_catalog_in_order());
        assert!(!manifest.matches_boot());
    }

    #[test]
    fn structural_errors_are_reported() {
        assert_eq!(Manifest::parse("name\n"), Err(ManifestError::MissingEquals));
        assert_eq!(
            Manifest::parse("serial=on\n"),
            Err(ManifestError::UnknownKey)
        );
        assert_eq!(
            Manifest::parse("name=a\nname=b\n"),
            Err(ManifestError::DuplicateKey)
        );
        assert_eq!(Manifest::parse("name=\n"), Err(ManifestError::EmptyValue));
        assert_eq!(
            Manifest::parse("services=blk,,kbd\n"),
            Err(ManifestError::EmptyServiceName)
        );
        assert_eq!(
            Manifest::parse("services=blk,kbd,blk\n"),
            Err(ManifestError::DuplicateService)
        );
    }

    #[test]
    fn the_service_list_is_bounded() {
        // One more name than the fixed array can hold.
        let text = "services=a,b,c,d,e,f,g,h,i\n";
        assert_eq!(
            Manifest::parse(text),
            Err(ManifestError::TooManyServices)
        );
    }

    #[test]
    fn names_are_subsequence_uses_the_supplied_catalog() {
        let manifest = Manifest::parse("services=blk,kbd\n").unwrap();
        assert!(names_are_subsequence(&manifest, &SERVICE_CATALOG));
        let reversed = [SERVICE_CATALOG[1], SERVICE_CATALOG[0]];
        assert!(!names_are_subsequence(&manifest, &reversed));
        assert!(!names_are_subsequence(&manifest, &[]));

        // A subset keeps the catalog's relative order.
        let subset = Manifest::parse("services=blk\n").unwrap();
        assert!(names_are_subsequence(&subset, &SERVICE_CATALOG));
    }
}
