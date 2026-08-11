use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::comparison::{DEFAULT_MISMATCH_LIMIT, canonicalize_json, json_differences};

const SAFETY_MARKER: &str = ".rustodon-differential-media-root";

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ActivityPubSnapshot {
    documents: BTreeMap<String, Value>,
}

impl ActivityPubSnapshot {
    pub(crate) fn new(documents: Vec<Value>) -> Result<Self, ArtifactError> {
        let mut by_id = BTreeMap::new();
        for document in documents {
            let id = document
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    ArtifactError("ActivityPub document must have a nonempty string id".to_owned())
                })?
                .to_owned();
            if by_id
                .insert(id.clone(), canonicalize_json(&document))
                .is_some()
            {
                return Err(ArtifactError(format!(
                    "duplicate ActivityPub document id {id}"
                )));
            }
        }
        Ok(Self { documents: by_id })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DurableJob {
    pub(crate) queue: String,
    pub(crate) job_type: String,
    pub(crate) arguments: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DurableJobSnapshot {
    jobs: Vec<DurableJob>,
}

impl DurableJobSnapshot {
    pub(crate) fn new(jobs: Vec<DurableJob>) -> Self {
        Self { jobs }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MediaFile {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MediaSnapshot {
    pub(crate) files: BTreeMap<String, MediaFile>,
}

impl MediaSnapshot {
    pub(crate) fn capture(root: &Path) -> Result<Self, ArtifactError> {
        let metadata = fs::symlink_metadata(root).map_err(ArtifactError::from)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ArtifactError(
                "media root must be a real directory, not a symlink".to_owned(),
            ));
        }
        let mut files = BTreeMap::new();
        capture_directory(root, root, &mut files)?;
        Ok(Self { files })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArtifactKind {
    ActivityPub,
    DurableJob,
    Media,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArtifactMismatch {
    pub(crate) kind: ArtifactKind,
    pub(crate) identity: String,
    pub(crate) path: String,
    pub(crate) mastodon: Option<Value>,
    pub(crate) rust: Option<Value>,
}

impl fmt::Display for ArtifactMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_with_labels(formatter, "Mastodon", "Rust")
    }
}

impl ArtifactMismatch {
    fn fmt_with_labels(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        mastodon_label: &str,
        rust_label: &str,
    ) -> fmt::Result {
        write!(
            formatter,
            "{:?} artifact {} at {}: {}={}, {}={}",
            self.kind,
            self.identity,
            self.path,
            mastodon_label,
            self.mastodon
                .as_ref()
                .map_or_else(|| "<missing>".to_owned(), Value::to_string),
            rust_label,
            self.rust
                .as_ref()
                .map_or_else(|| "<missing>".to_owned(), Value::to_string)
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArtifactMismatchReport {
    pub(crate) mismatches: Vec<ArtifactMismatch>,
    pub(crate) omitted: usize,
    mastodon_label: String,
    rust_label: String,
}

impl fmt::Display for ArtifactMismatchReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "artifact comparison found {} mismatch(es):",
            self.mismatches.len() + self.omitted
        )?;
        for mismatch in &self.mismatches {
            formatter.write_str("- ")?;
            mismatch.fmt_with_labels(formatter, &self.mastodon_label, &self.rust_label)?;
            formatter.write_str("\n")?;
        }
        if self.omitted > 0 {
            writeln!(
                formatter,
                "- ... {} additional mismatch(es) omitted",
                self.omitted
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ArtifactMismatchReport {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactError(String);

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArtifactError {}

impl From<io::Error> for ArtifactError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

pub(crate) fn compare_activity_pub(
    mastodon: &ActivityPubSnapshot,
    rust: &ActivityPubSnapshot,
    limit: usize,
) -> Result<(), ArtifactMismatchReport> {
    let mut collector = ArtifactCollector::new(limit, "Mastodon", "Rust");
    let ids = mastodon
        .documents
        .keys()
        .chain(rust.documents.keys())
        .collect::<BTreeSet<_>>();
    for id in ids {
        match (mastodon.documents.get(id), rust.documents.get(id)) {
            (Some(mastodon), Some(rust)) => {
                let remaining = collector.remaining();
                let (differences, omitted) = json_differences(mastodon, rust, remaining);
                for difference in differences {
                    collector.push(ArtifactMismatch {
                        kind: ArtifactKind::ActivityPub,
                        identity: id.clone(),
                        path: difference.path,
                        mastodon: observed_value(difference.mastodon),
                        rust: observed_value(difference.rust),
                    });
                }
                collector.omitted += omitted;
            }
            (Some(mastodon), None) => collector.push(ArtifactMismatch {
                kind: ArtifactKind::ActivityPub,
                identity: id.clone(),
                path: "$".to_owned(),
                mastodon: Some(mastodon.clone()),
                rust: None,
            }),
            (None, Some(rust)) => collector.push(ArtifactMismatch {
                kind: ArtifactKind::ActivityPub,
                identity: id.clone(),
                path: "$".to_owned(),
                mastodon: None,
                rust: Some(rust.clone()),
            }),
            (None, None) => unreachable!("a union ActivityPub id must exist in one snapshot"),
        }
    }
    collector.finish()
}

pub(crate) fn compare_durable_jobs(
    mastodon: &DurableJobSnapshot,
    rust: &DurableJobSnapshot,
    limit: usize,
) -> Result<(), ArtifactMismatchReport> {
    let mastodon = job_multiset(&mastodon.jobs);
    let rust = job_multiset(&rust.jobs);
    let jobs = mastodon.keys().chain(rust.keys()).collect::<BTreeSet<_>>();
    let mut collector = ArtifactCollector::new(limit, "Mastodon", "Rust");
    for job in jobs {
        let mastodon_count = mastodon.get(job).copied().unwrap_or_default();
        let rust_count = rust.get(job).copied().unwrap_or_default();
        if mastodon_count != rust_count {
            collector.push(ArtifactMismatch {
                kind: ArtifactKind::DurableJob,
                identity: job.clone(),
                path: "$count".to_owned(),
                mastodon: Some(Value::from(mastodon_count)),
                rust: Some(Value::from(rust_count)),
            });
        }
    }
    collector.finish()
}

pub(crate) fn compare_media(
    mastodon: &MediaSnapshot,
    rust: &MediaSnapshot,
    limit: usize,
) -> Result<(), ArtifactMismatchReport> {
    compare_media_with_labels(mastodon, rust, "Mastodon", "Rust", limit)
}

pub(crate) fn compare_media_with_labels(
    mastodon: &MediaSnapshot,
    rust: &MediaSnapshot,
    mastodon_label: &str,
    rust_label: &str,
    limit: usize,
) -> Result<(), ArtifactMismatchReport> {
    let paths = mastodon
        .files
        .keys()
        .chain(rust.files.keys())
        .collect::<BTreeSet<_>>();
    let mut collector =
        ArtifactCollector::new(limit, mastodon_label.to_owned(), rust_label.to_owned());
    for path in paths {
        match (mastodon.files.get(path), rust.files.get(path)) {
            (Some(mastodon), Some(rust)) => {
                if mastodon.bytes != rust.bytes {
                    collector.push(ArtifactMismatch {
                        kind: ArtifactKind::Media,
                        identity: path.clone(),
                        path: "$.bytes".to_owned(),
                        mastodon: Some(Value::from(mastodon.bytes)),
                        rust: Some(Value::from(rust.bytes)),
                    });
                }
                if mastodon.sha256 != rust.sha256 {
                    collector.push(ArtifactMismatch {
                        kind: ArtifactKind::Media,
                        identity: path.clone(),
                        path: "$.sha256".to_owned(),
                        mastodon: Some(Value::String(mastodon.sha256.clone())),
                        rust: Some(Value::String(rust.sha256.clone())),
                    });
                }
            }
            (Some(mastodon), None) => collector.push(ArtifactMismatch {
                kind: ArtifactKind::Media,
                identity: path.clone(),
                path: "$".to_owned(),
                mastodon: Some(media_value(mastodon)),
                rust: None,
            }),
            (None, Some(rust)) => collector.push(ArtifactMismatch {
                kind: ArtifactKind::Media,
                identity: path.clone(),
                path: "$".to_owned(),
                mastodon: None,
                rust: Some(media_value(rust)),
            }),
            (None, None) => unreachable!("a union media path must exist in one snapshot"),
        }
    }
    collector.finish()
}

fn capture_directory(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, MediaFile>,
) -> Result<(), ArtifactError> {
    for entry in fs::read_dir(directory).map_err(ArtifactError::from)? {
        let entry = entry.map_err(ArtifactError::from)?;
        let file_type = entry.file_type().map_err(ArtifactError::from)?;
        if file_type.is_symlink() {
            return Err(ArtifactError(format!(
                "media tree contains symlink: {}",
                entry.path().display()
            )));
        }
        if file_type.is_dir() {
            capture_directory(root, &entry.path(), files)?;
        } else if file_type.is_file() {
            let relative = relative_path(root, &entry.path())?;
            // This file is safety metadata, not part of the observable media tree.
            if relative == SAFETY_MARKER {
                continue;
            }
            let bytes = entry.metadata().map_err(ArtifactError::from)?.len();
            let sha256 = sha256_file(&entry.path())?;
            files.insert(
                relative.clone(),
                MediaFile {
                    path: relative,
                    bytes,
                    sha256,
                },
            );
        } else {
            return Err(ArtifactError(format!(
                "media tree contains unsupported entry: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<String, ArtifactError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| ArtifactError("media file escaped its root".to_owned()))?;
    let components = relative
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .ok_or_else(|| ArtifactError("media path is not UTF-8".to_owned()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(components.join("/"))
}

fn sha256_file(path: &Path) -> Result<String, ArtifactError> {
    let mut file = File::open(path).map_err(ArtifactError::from)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(ArtifactError::from)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn job_multiset(jobs: &[DurableJob]) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for job in jobs {
        let value = canonicalize_json(&json!({
            "queue": job.queue,
            "job_type": job.job_type,
            "arguments": job.arguments,
        }));
        *counts.entry(value.to_string()).or_default() += 1;
    }
    counts
}

fn observed_value(value: super::comparison::ObservedJson) -> Option<Value> {
    match value {
        super::comparison::ObservedJson::Missing => None,
        super::comparison::ObservedJson::Value(value) => Some(value),
    }
}

fn media_value(file: &MediaFile) -> Value {
    json!({
        "path": file.path,
        "bytes": file.bytes,
        "sha256": file.sha256,
    })
}

struct ArtifactCollector {
    limit: usize,
    mismatches: Vec<ArtifactMismatch>,
    omitted: usize,
    mastodon_label: String,
    rust_label: String,
}

impl ArtifactCollector {
    fn new(limit: usize, mastodon_label: impl Into<String>, rust_label: impl Into<String>) -> Self {
        Self {
            limit,
            mismatches: Vec::with_capacity(limit),
            omitted: 0,
            mastodon_label: mastodon_label.into(),
            rust_label: rust_label.into(),
        }
    }

    fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.mismatches.len())
    }

    fn push(&mut self, mismatch: ArtifactMismatch) {
        if self.mismatches.len() < self.limit {
            self.mismatches.push(mismatch);
        } else {
            self.omitted += 1;
        }
    }

    fn finish(self) -> Result<(), ArtifactMismatchReport> {
        if self.mismatches.is_empty() && self.omitted == 0 {
            Ok(())
        } else {
            Err(ArtifactMismatchReport {
                mismatches: self.mismatches,
                omitted: self.omitted,
                mastodon_label: self.mastodon_label,
                rust_label: self.rust_label,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    fn temporary_directory(label: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "differential-artifact-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock should follow Unix epoch")
                    .as_nanos()
            ))
    }

    #[test]
    fn activity_pub_comparison_is_typed_and_has_json_path_diagnostics() {
        let mastodon = ActivityPubSnapshot::new(vec![json!({
            "id": "https://fixture.invalid/activity/1",
            "type": "Create",
            "object": {"content": "old"},
        })])
        .expect("valid ActivityPub snapshot");
        let rust = ActivityPubSnapshot::new(vec![json!({
            "type": "Create",
            "id": "https://fixture.invalid/activity/1",
            "object": {"content": "new"},
        })])
        .expect("valid ActivityPub snapshot");

        let report = compare_activity_pub(&mastodon, &rust, DEFAULT_MISMATCH_LIMIT)
            .expect_err("documents intentionally differ");
        assert_eq!(report.mismatches[0].kind, ArtifactKind::ActivityPub);
        assert!(report.to_string().contains("$.object.content"));
    }

    #[test]
    fn durable_job_comparison_preserves_argument_array_order() {
        let mastodon = DurableJobSnapshot::new(vec![DurableJob {
            queue: "default".to_owned(),
            job_type: "Deliver".to_owned(),
            arguments: json!([1, 2]),
        }]);
        let rust = DurableJobSnapshot::new(vec![DurableJob {
            queue: "default".to_owned(),
            job_type: "Deliver".to_owned(),
            arguments: json!([2, 1]),
        }]);

        let report = compare_durable_jobs(&mastodon, &rust, DEFAULT_MISMATCH_LIMIT)
            .expect_err("job arguments intentionally differ");
        assert!(
            report
                .mismatches
                .iter()
                .all(|mismatch| mismatch.kind == ArtifactKind::DurableJob)
        );
        assert!(report.to_string().contains("$count"));
    }

    #[test]
    fn media_snapshot_uses_relative_path_size_and_sha256() {
        let root = temporary_directory("hash");
        fs::create_dir_all(root.join("nested")).expect("test directory should be created");
        fs::write(root.join("nested/file.txt"), b"abc").expect("test file should be written");

        let snapshot = MediaSnapshot::capture(&root).expect("media snapshot should succeed");
        let file = &snapshot.files["nested/file.txt"];
        assert_eq!(file.path, "nested/file.txt");
        assert_eq!(file.bytes, 3);
        assert_eq!(
            file.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_dir_all(root).expect("test directory should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn media_snapshot_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("symlink");
        fs::create_dir_all(&root).expect("test directory should be created");
        fs::write(root.join("file.txt"), b"abc").expect("test file should be written");
        symlink(root.join("file.txt"), root.join("link.txt")).expect("test symlink should work");

        let error = MediaSnapshot::capture(&root).expect_err("symlink must be rejected");
        assert!(error.to_string().contains("symlink"));
        fs::remove_dir_all(root).expect("test directory should be removable");
    }

    #[test]
    fn media_comparison_identifies_file_and_changed_field() {
        let mastodon = MediaSnapshot {
            files: BTreeMap::from([(
                "image.png".to_owned(),
                MediaFile {
                    path: "image.png".to_owned(),
                    bytes: 3,
                    sha256: "aaa".to_owned(),
                },
            )]),
        };
        let rust = MediaSnapshot {
            files: BTreeMap::from([(
                "image.png".to_owned(),
                MediaFile {
                    path: "image.png".to_owned(),
                    bytes: 4,
                    sha256: "bbb".to_owned(),
                },
            )]),
        };

        let report = compare_media(&mastodon, &rust, DEFAULT_MISMATCH_LIMIT)
            .expect_err("media intentionally differs");
        let diagnostic = report.to_string();
        assert!(diagnostic.contains("image.png"));
        assert!(diagnostic.contains("$.bytes"));
        assert!(diagnostic.contains("$.sha256"));
    }

    #[test]
    fn media_self_comparison_diagnostics_name_the_side_and_phase() {
        let before = MediaSnapshot {
            files: BTreeMap::from([(
                "image.png".to_owned(),
                MediaFile {
                    path: "image.png".to_owned(),
                    bytes: 3,
                    sha256: "aaa".to_owned(),
                },
            )]),
        };
        let after = MediaSnapshot {
            files: BTreeMap::from([(
                "image.png".to_owned(),
                MediaFile {
                    path: "image.png".to_owned(),
                    bytes: 4,
                    sha256: "bbb".to_owned(),
                },
            )]),
        };

        let report = compare_media_with_labels(
            &before,
            &after,
            "Rust before",
            "Rust after",
            DEFAULT_MISMATCH_LIMIT,
        )
        .expect_err("the test file intentionally changed");
        let diagnostic = report.to_string();
        assert!(diagnostic.contains("Rust before=3"));
        assert!(diagnostic.contains("Rust after=4"));
        assert!(!diagnostic.contains("Mastodon="));
    }
}
