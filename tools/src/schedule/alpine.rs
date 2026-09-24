//! Alpine Linux package index sync, see docs/alpine.md.
//!
//! Alpine publishes `<mirror>/<branch>/<repo>/<arch>/APKINDEX.tar.gz`, for
//! example `https://dl-cdn.alpinelinux.org/alpine/v3.24/main/x86_64/`. The
//! mapping onto rebuilderd's model follows the Debian one:
//!
//! | rebuilderd     | Alpine                                  | Debian                                  |
//! |----------------|-----------------------------------------|-----------------------------------------|
//! | release        | branch: `v3.24`, `edge`                 | suite: `trixie`, `sid`                  |
//! | component      | repo: `main`, `community`, `testing`    | `main`, `contrib`, `non-free`           |
//! | architecture   | the arch directory: `x86_64`, `aarch64` | `amd64`; there is no `all` (see `A:`)   |
//! | source package | origin (`o:`), the aport directory      | source package                          |
//! | version        | `<pkgver>-r<pkgrel>` (`V:`)             | version; but no epoch and no binNMU     |
//! | binary package | one index record (`P:`)                 | binary package                          |
//! | build input    | `apkbuildinfo.json` from the archive    | `.buildinfo` from buildinfos.debian.net |
//!
//! Two things differ from Debian and shape the URLs below:
//!
//! - The mirror keeps only the newest version of each package, so `source` is
//!   an Alpine archive (see [`Layout`]): its `latest/` tree has the mirror's
//!   layout and its `archive/` keeps every `.apk` that was ever published.
//! - The same name and version is published on several branches with different
//!   bytes (another builder, toolchain and abuild; on stable branches usually a
//!   cherry-picked commit too). Of 18,010 name-version pairs present on both
//!   edge and v3.24 x86_64 on 2026-09-24, 2,037 had the same control checksum.
//!   So the release is part of every URL the sync emits, and the daemon never
//!   treats an edge build input as the friend of a v3.24 one.

use crate::args::PkgsSync;
use crate::rules;
use crate::schedule::{Pkg, fetch_url_or_path};
use flate2::read::MultiGzDecoder;
use rebuilderd_common::api::v1::{BinaryPackageReport, PackageReport, SourcePackageReport};
use rebuilderd_common::errors::*;
use rebuilderd_common::http;
use std::collections::BTreeMap;
use std::io::prelude::*;
use tar::Archive;

pub const DISTRO: &str = "alpine";

/// One `APKINDEX` record: a binary package as published in one
/// `<branch>/<repo>/<arch>` directory.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApkPkg {
    /// `C:`, `Q1` + base64(sha1(control segment)). This is the identity of the
    /// published bytes. The same filename can be re-published with a new
    /// checksum (a `testing` to `community` move on edge, a rebuild after a
    /// purge), and the same name and version exists on several branches.
    pub checksum: String,
    /// `P:`
    pub name: String,
    /// `V:`, `<pkgver>-r<pkgrel>`. No epoch, no binNMU suffix, and every
    /// subpackage of an origin carries the origin's version.
    pub version: String,
    /// `A:`. Always the builder's architecture: `buildrepo` runs
    /// `apk index --rewrite-arch <arch>`, so `noarch` never appears in a
    /// published index and each arch directory's noarch packages are that
    /// builder's own build (6,012 of 6,012 edge/main/x86_64 records say
    /// `x86_64`).
    pub architecture: String,
    /// `o:`, the aport (source package) this record was built from.
    pub origin: String,
    /// `m:`
    pub maintainer: Option<String>,
    /// `c:`, the aports commit that last touched the aport directory.
    pub commit: Option<String>,
}

impl ApkPkg {
    pub fn filename(&self) -> String {
        format!("{}-{}.apk", self.name, self.version)
    }
}

impl Pkg for ApkPkg {
    fn binary_pkg_name(&self) -> &str {
        &self.name
    }

    fn source_pkg_name(&self) -> Option<&str> {
        Some(&self.origin)
    }

    fn maintainers(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.maintainer.iter().map(String::as_str))
    }
}

/// The parsed `APKINDEX.tar.gz`: `APKINDEX`, `DESCRIPTION` and a signature
/// member we do not verify here (apk does, on the worker).
#[derive(Debug, Default)]
pub struct ApkIndex {
    /// `DESCRIPTION`: `git describe` of aports at index time, for example
    /// `v3.24.2-23-gf65d7bf05a4`. Not used yet, but it is the closest thing
    /// Alpine has to a Release file's date.
    pub description: Option<String>,
    pub pkgs: Vec<ApkPkg>,
}

/// `APKINDEX.tar.gz` is two gzip members back to back: the signature tar,
/// cut before its end-of-archive marker, then the `DESCRIPTION` + `APKINDEX`
/// tar. A single-member gzip decoder stops after the signature, so this reads
/// all members; the cut makes the concatenation one valid tar (an `.apk` is
/// built the same way).
pub fn extract_index(bytes: &[u8]) -> Result<ApkIndex> {
    let tar = MultiGzDecoder::new(bytes);
    let mut archive = Archive::new(tar);

    let mut index = ApkIndex::default();
    let mut found = false;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        match path.as_str() {
            "APKINDEX" => {
                let mut text = String::new();
                entry.read_to_string(&mut text)?;
                index.pkgs = parse_index(&text)?;
                found = true;
            }
            "DESCRIPTION" => {
                let mut text = String::new();
                entry.read_to_string(&mut text)?;
                index.description = Some(text.trim().to_string());
            }
            _ => (),
        }
    }
    if !found {
        bail!("No APKINDEX member in index archive");
    }
    Ok(index)
}

/// Parses the `APKINDEX` text: records separated by blank lines, one
/// `<letter>:<value>` field per line.
pub fn parse_index(text: &str) -> Result<Vec<ApkPkg>> {
    let mut pkgs = Vec::new();
    for block in text.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }

        let mut pkg = ApkPkg::default();
        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            match key {
                "C" => pkg.checksum = value.to_string(),
                "P" => pkg.name = value.to_string(),
                "V" => pkg.version = value.to_string(),
                "A" => pkg.architecture = value.to_string(),
                "o" => pkg.origin = value.to_string(),
                "m" => pkg.maintainer = Some(value.to_string()),
                "c" => pkg.commit = Some(value.to_string()),
                _ => (),
            }
        }

        for (field, value) in [
            ("C", &pkg.checksum),
            ("P", &pkg.name),
            ("V", &pkg.version),
            ("A", &pkg.architecture),
        ] {
            if value.is_empty() {
                bail!("APKINDEX record is missing {field}: {block:?}");
            }
        }
        if pkg.origin.is_empty() {
            // abuild always writes `origin`; a hand-made index might not
            debug!(
                "APKINDEX record has no origin, using its name: {:?}",
                pkg.name
            );
            pkg.origin = pkg.name.clone();
        }

        pkgs.push(pkg);
    }
    Ok(pkgs)
}

/// The URLs the sync reads and the ones it hands to the daemon. `source` is
/// the base URL of an Alpine archive that serves the routes below (see
/// docs/alpine.md). Everything below the branch is the mirror's layout, so
/// `latest/` could be swapped for `https://dl-cdn.alpinelinux.org/alpine` to
/// read the live index; the artifact and build input URLs cannot.
pub struct Layout<'a> {
    pub source: &'a str,
}

impl Layout<'_> {
    pub fn index_url(&self, release: &str, component: &str, arch: &str) -> String {
        format!(
            "{}/latest/{release}/{component}/{arch}/APKINDEX.tar.gz",
            self.source
        )
    }

    /// The published bytes. The worker names its download after the last path
    /// segment, so the disambiguating control checksum goes in the query
    /// string; without it, `archive/` returns the newest archived record of
    /// that name and version.
    pub fn artifact_url(&self, release: &str, component: &str, arch: &str, pkg: &ApkPkg) -> String {
        let checksum =
            url::form_urlencoded::byte_serialize(pkg.checksum.as_bytes()).collect::<String>();
        format!(
            "{}/archive/{release}/{component}/{arch}/{}?c={checksum}",
            self.source,
            pkg.filename()
        )
    }

    /// The build input, passed to the rebuild command unparsed. The worker
    /// re-downloads it on every retry, so the archive can serve a better
    /// build input once it has more information about the official build.
    pub fn buildinfo_url(
        &self,
        release: &str,
        component: &str,
        arch: &str,
        origin: &str,
        version: &str,
    ) -> String {
        format!(
            "{}/buildinfo/{release}/{component}/{arch}/{origin}/{version}/apkbuildinfo.json",
            self.source
        )
    }
}

type SourceKey = (String, String, String); // component, origin, version

#[derive(Debug, Default)]
pub struct SyncState {
    // (release, architecture) -> (component, origin, version) -> source package
    reports: BTreeMap<(String, String), BTreeMap<SourceKey, SourcePackageReport>>,
}

impl SyncState {
    pub fn new() -> SyncState {
        SyncState::default()
    }

    /// Creates the report for a release and architecture once one of its
    /// indexes has been read, even if nothing is imported from it: an empty
    /// report tells the daemon every package of that scope is gone. A scope
    /// whose indexes could not be fetched gets no report, so an unreachable
    /// archive doesn't drop its queued jobs.
    fn group(
        &mut self,
        release: &str,
        arch: &str,
    ) -> &mut BTreeMap<SourceKey, SourcePackageReport> {
        self.reports
            .entry((release.to_string(), arch.to_string()))
            .or_default()
    }

    pub fn import_index(
        &mut self,
        layout: &Layout,
        release: &str,
        component: &str,
        arch: &str,
        pkgs: Vec<ApkPkg>,
        sync: &PkgsSync,
    ) {
        let group = self.group(release, arch);
        for pkg in pkgs {
            if !rules::matches(sync, &pkg, component) {
                continue;
            }
            if pkg.architecture != arch {
                // cannot happen with a `--rewrite-arch` index; the arch
                // directory is what identifies the build either way
                debug!(
                    "Record {:?} says arch {:?} in the {arch} directory",
                    pkg.name, pkg.architecture
                );
            }

            let key = (
                component.to_string(),
                pkg.origin.clone(),
                pkg.version.clone(),
            );
            let source = group.entry(key).or_insert_with(|| SourcePackageReport {
                name: pkg.origin.clone(),
                version: pkg.version.clone(),
                url: layout.buildinfo_url(release, component, arch, &pkg.origin, &pkg.version),
                artifacts: Vec::new(),
            });
            source.artifacts.push(BinaryPackageReport {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                component: Some(component.to_string()),
                architecture: arch.to_string(),
                url: layout.artifact_url(release, component, arch, &pkg),
            });
        }
    }

    pub fn to_vec(self) -> Vec<PackageReport> {
        self.reports
            .into_iter()
            .map(|((release, architecture), sources)| PackageReport {
                distribution: DISTRO.to_string(),
                release: Some(release),
                architecture,
                packages: sources.into_values().collect(),
            })
            .collect()
    }
}

pub async fn sync(http: &http::Client, sync: &PkgsSync) -> Result<Vec<PackageReport>> {
    let mut state = SyncState::new();

    for release in &sync.releases {
        let source = release.source(&sync.source).trim_end_matches('/');
        let layout = Layout { source };

        for arch in &sync.architectures {
            for component in &sync.components {
                let url = layout.index_url(release.name(), component, arch);
                let bytes = match fetch_url_or_path(http, &url).await {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        // e.g. a repo the archive doesn't carry
                        warn!("{:#}, skipping", err);
                        continue;
                    }
                };

                let index = extract_index(&bytes)?;
                info!(
                    "Parsing index ({} bytes, {} packages, aports {})...",
                    bytes.len(),
                    index.pkgs.len(),
                    index.description.as_deref().unwrap_or("?")
                );
                state.import_index(&layout, release.name(), component, arch, index.pkgs, sync);
            }
        }
    }

    Ok(state.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SyncRelease;

    // real records from edge/main/x86_64, index observed 2026-09-24, trimmed
    // to the fields the sync reads plus `t:`
    const INDEX: &str = "C:Q1UCWI3rGwHXLUm8uDxKKTyRGLYl8=
P:xz
V:5.8.4-r0
A:x86_64
o:xz
m:Natanael Copa <ncopa@alpinelinux.org>
t:1788989661
c:950af2f9ad0e4518d167e8ce0b41e7be0a7a730e

C:Q1IGOtYiUfqAO92P4MkrCW9Jt0w/I=
P:xz-dev
V:5.8.4-r0
A:x86_64
o:xz
m:Natanael Copa <ncopa@alpinelinux.org>
t:1788989661
c:950af2f9ad0e4518d167e8ce0b41e7be0a7a730e

C:Q18LppPR/lKh14h4uoGPQXtA+zNpc=
P:xz-doc
V:5.8.4-r0
A:x86_64
o:xz
m:Natanael Copa <ncopa@alpinelinux.org>
t:1788989661
c:950af2f9ad0e4518d167e8ce0b41e7be0a7a730e

C:Q1Fnp3Gzp9QyRr1djWc2TJHFDGQjY=
P:zlib
V:1.3.2-r0
A:x86_64
o:zlib
m:Natanael Copa <ncopa@alpinelinux.org>
t:1772704820
c:f248b33b5943c7dc69bf691031d7612ab2e8ed93

C:Q1csNyRHLl5/PIarKhaKFxDpEgV/8=
P:zlib-dev
V:1.3.2-r0
A:x86_64
o:zlib
m:Natanael Copa <ncopa@alpinelinux.org>
t:1772704820
c:f248b33b5943c7dc69bf691031d7612ab2e8ed93

";

    fn pkgs_sync() -> PkgsSync {
        PkgsSync {
            distro: "alpine".to_string(),
            components: vec!["main".to_string()],
            source: "https://archive.example.com/alpine".to_string(),
            architectures: vec!["x86_64".to_string()],
            print_json: false,
            include: vec![],
            maintainers: vec![],
            releases: vec![],
            pkgs: vec![],
            excludes: vec![],
            sync_method: None,
        }
    }

    #[test]
    fn test_parse_index() {
        let pkgs = parse_index(INDEX).unwrap();
        assert_eq!(pkgs.len(), 5);
        assert_eq!(
            pkgs[1],
            ApkPkg {
                checksum: "Q1IGOtYiUfqAO92P4MkrCW9Jt0w/I=".to_string(),
                name: "xz-dev".to_string(),
                version: "5.8.4-r0".to_string(),
                architecture: "x86_64".to_string(),
                origin: "xz".to_string(),
                maintainer: Some("Natanael Copa <ncopa@alpinelinux.org>".to_string()),
                commit: Some("950af2f9ad0e4518d167e8ce0b41e7be0a7a730e".to_string()),
            }
        );
        assert_eq!(pkgs[1].filename(), "xz-dev-5.8.4-r0.apk");
    }

    #[test]
    fn test_group_by_origin() {
        let sync = pkgs_sync();
        let layout = Layout {
            source: "https://archive.example.com/alpine",
        };
        let mut state = SyncState::new();
        state.import_index(
            &layout,
            "edge",
            "main",
            "x86_64",
            parse_index(INDEX).unwrap(),
            &sync,
        );
        let reports = state.to_vec();

        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert_eq!(report.distribution, "alpine");
        assert_eq!(report.release.as_deref(), Some("edge"));
        assert_eq!(report.architecture, "x86_64");
        assert_eq!(report.packages.len(), 2);

        let xz = &report.packages[0];
        assert_eq!((xz.name.as_str(), xz.version.as_str()), ("xz", "5.8.4-r0"));
        assert_eq!(
            xz.url,
            "https://archive.example.com/alpine/buildinfo/edge/main/x86_64/xz/5.8.4-r0/apkbuildinfo.json"
        );
        assert_eq!(xz.artifacts.len(), 3);
        assert_eq!(
            xz.artifacts[1],
            BinaryPackageReport {
                name: "xz-dev".to_string(),
                version: "5.8.4-r0".to_string(),
                component: Some("main".to_string()),
                architecture: "x86_64".to_string(),
                // `+`, `/` and `=` of the base64 checksum are percent-encoded;
                // the filename stays the last path segment
                url: "https://archive.example.com/alpine/archive/edge/main/x86_64/xz-dev-5.8.4-r0.apk?c=Q1IGOtYiUfqAO92P4MkrCW9Jt0w%2FI%3D".to_string(),
            }
        );

        let zlib = &report.packages[1];
        assert_eq!(zlib.name, "zlib");
        assert_eq!(zlib.artifacts.len(), 2);
    }

    #[test]
    fn test_same_version_on_two_branches_is_two_inputs() {
        // zlib 1.3.2-r0 is on edge and v3.24 with the same commit and
        // different bytes; the build inputs must differ so the daemon does not
        // copy one branch's result to the other
        let sync = pkgs_sync();
        let layout = Layout {
            source: "https://archive.example.com/alpine",
        };
        let mut state = SyncState::new();
        for release in ["edge", "v3.24"] {
            state.import_index(
                &layout,
                release,
                "main",
                "x86_64",
                parse_index(INDEX).unwrap(),
                &sync,
            );
        }
        let reports = state.to_vec();
        assert_eq!(reports.len(), 2);
        let urls = reports
            .iter()
            .map(|r| r.packages[1].url.clone())
            .collect::<Vec<_>>();
        assert_ne!(urls[0], urls[1]);
    }

    #[test]
    fn test_empty_index_still_reports() {
        let layout = Layout {
            source: "https://archive.example.com/alpine",
        };
        let mut state = SyncState::new();
        state.import_index(&layout, "v3.24", "main", "aarch64", vec![], &pkgs_sync());
        assert_eq!(
            state.to_vec(),
            vec![PackageReport {
                distribution: "alpine".to_string(),
                release: Some("v3.24".to_string()),
                architecture: "aarch64".to_string(),
                packages: vec![],
            }]
        );
    }

    #[tokio::test]
    async fn test_unreachable_index_reports_nothing() {
        let mut sync = pkgs_sync();
        sync.source = "/nonexistent/alpine".to_string();
        sync.releases = vec![SyncRelease::new("v3.24")];
        let reports = super::sync(&http::client().unwrap(), &sync).await.unwrap();
        assert_eq!(reports, vec![]);
    }
}
