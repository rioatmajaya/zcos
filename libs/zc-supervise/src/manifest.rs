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

    /// Returns whether the manifest names the catalog's services in boot order.
    ///
    /// Names, not ids, are what the file carries: an id is an ABI detail and a
    /// person editing the manifest should never have to look one up.
    #[must_use]
    pub fn names_catalog_in_order(&self) -> bool {
        names_match_in_order(self, &SERVICE_CATALOG)
    }

    /// Returns whether the manifest names the default init and the catalog.
    ///
    /// This is the check `initd` makes at boot; it deliberately does not
    /// require informational keys such as `name` or `version`.
    #[must_use]
    pub fn matches_boot(&self) -> bool {
        self.init == Some(DEFAULT_INIT) && self.names_catalog_in_order()
    }

    /// Appends the comma-separated names in `value` to the service list.
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
/// a test name an arbitrary order without rebuilding a manifest.
#[must_use]
pub fn names_match_in_order(manifest: &Manifest<'_>, catalog: &[ServiceSpec]) -> bool {
    manifest.services().len() == catalog.len()
        && manifest
            .services()
            .iter()
            .zip(catalog.iter())
            .all(|(name, service)| *name == service.name)
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
    fn a_wrong_order_names_the_wrong_catalog_order() {
        let text = "init=/sbin/initd\nservices=kbd,blk\n";
        let manifest = Manifest::parse(text).expect("manifest");
        assert!(!manifest.names_catalog_in_order());
        assert!(!manifest.matches_boot());
        // The names are all present; only the order differs.
        assert_eq!(manifest.services().len(), SERVICE_CATALOG.len());
    }

    #[test]
    fn a_missing_or_extra_service_is_rejected() {
        let missing = Manifest::parse("init=/sbin/initd\nservices=blk\n").unwrap();
        assert!(!missing.names_catalog_in_order());
        let extra = Manifest::parse("init=/sbin/initd\nservices=blk,kbd,net\n").unwrap();
        assert!(!extra.names_catalog_in_order());
        // No services line at all is also a mismatch, not a parse error.
        let none = Manifest::parse("init=/sbin/initd\n").unwrap();
        assert!(none.services().is_empty());
        assert!(!none.names_catalog_in_order());
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
    fn names_match_in_order_uses_the_supplied_catalog() {
        let manifest = Manifest::parse("services=blk,kbd\n").unwrap();
        assert!(names_match_in_order(&manifest, &SERVICE_CATALOG));
        let reversed = [SERVICE_CATALOG[1], SERVICE_CATALOG[0]];
        assert!(!names_match_in_order(&manifest, &reversed));
        assert!(!names_match_in_order(&manifest, &[]));
    }
}
