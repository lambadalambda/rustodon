use std::error::Error;

use url::Url;

use super::artifacts::{MediaSnapshot, compare_media, compare_media_with_labels};
use super::comparison::DEFAULT_MISMATCH_LIMIT;
use super::database::{
    DatabaseSnapshot, TableSelection, compare_database_snapshots,
    compare_database_snapshots_with_labels, snapshot_database,
};
use super::harness::{RequestSpec, ResponsePair, send_identically};
use super::safety::{DifferentialConfig, HttpTargets};

pub(crate) struct ReadOnlyGuard {
    config: DifferentialConfig,
    targets: HttpTargets,
    mastodon_database_before: DatabaseSnapshot,
    rust_database_before: DatabaseSnapshot,
    mastodon_media_before: MediaSnapshot,
    rust_media_before: MediaSnapshot,
}

impl ReadOnlyGuard {
    pub(crate) async fn begin(
        config: DifferentialConfig,
        rust_url: &Url,
    ) -> Result<Self, Box<dyn Error>> {
        let targets = HttpTargets::new(config.mastodon_http.as_str(), rust_url.as_str())?;
        let (mastodon_database_before, rust_database_before) = tokio::try_join!(
            snapshot_database(config.mastodon_database.url(), TableSelection::AllPublic),
            snapshot_database(config.rust_database.url(), TableSelection::AllPublic),
        )?;
        compare_database_snapshots(
            &mastodon_database_before,
            &rust_database_before,
            DEFAULT_MISMATCH_LIMIT,
        )?;
        let mastodon_media_before = MediaSnapshot::capture(&config.mastodon_media)?;
        let rust_media_before = MediaSnapshot::capture(&config.rust_media)?;
        compare_media(
            &mastodon_media_before,
            &rust_media_before,
            DEFAULT_MISMATCH_LIMIT,
        )?;
        Ok(Self {
            config,
            targets,
            mastodon_database_before,
            rust_database_before,
            mastodon_media_before,
            rust_media_before,
        })
    }

    pub(crate) async fn send(&self, request: &RequestSpec) -> Result<ResponsePair, Box<dyn Error>> {
        Ok(send_identically(&self.targets, request).await?)
    }

    pub(crate) async fn finish(self) -> Result<(), Box<dyn Error>> {
        let (mastodon_database_after, rust_database_after) = tokio::try_join!(
            snapshot_database(
                self.config.mastodon_database.url(),
                TableSelection::AllPublic,
            ),
            snapshot_database(self.config.rust_database.url(), TableSelection::AllPublic,),
        )?;
        compare_database_snapshots_with_labels(
            &self.mastodon_database_before,
            &mastodon_database_after,
            "Mastodon before",
            "Mastodon after",
            DEFAULT_MISMATCH_LIMIT,
        )?;
        compare_database_snapshots_with_labels(
            &self.rust_database_before,
            &rust_database_after,
            "Rust before",
            "Rust after",
            DEFAULT_MISMATCH_LIMIT,
        )?;
        compare_database_snapshots(
            &mastodon_database_after,
            &rust_database_after,
            DEFAULT_MISMATCH_LIMIT,
        )?;

        let mastodon_media_after = MediaSnapshot::capture(&self.config.mastodon_media)?;
        let rust_media_after = MediaSnapshot::capture(&self.config.rust_media)?;
        compare_media_with_labels(
            &self.mastodon_media_before,
            &mastodon_media_after,
            "Mastodon before",
            "Mastodon after",
            DEFAULT_MISMATCH_LIMIT,
        )?;
        compare_media_with_labels(
            &self.rust_media_before,
            &rust_media_after,
            "Rust before",
            "Rust after",
            DEFAULT_MISMATCH_LIMIT,
        )?;
        compare_media(
            &mastodon_media_after,
            &rust_media_after,
            DEFAULT_MISMATCH_LIMIT,
        )?;
        Ok(())
    }
}
