use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const REVISION: &str = "1440d55b139e39ec722c2a3db7f60b66cd889048";
const SCHEMA_SHA256: &str = "e7915a2fadcb1a5f4cb1c5d5fe4aeccb97ecdfaed37759d6b66873efffbde54a";
const MASTODON_IMAGE: &str = "ghcr.io/mastodon/mastodon@sha256:77f11d1a6c674664217372d94ccdb9203524c60447827fe74ab6e11466825815";
const REDIS_IMAGE: &str = "docker.io/library/redis@sha256:e7723ff73d963f5cc6d9c4643ea3d989527a402a319239054e9472a7fb9219a2";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_dir() -> PathBuf {
    repository_root().join("fixtures/mastodon/v4.6.5")
}

fn fixture_tool() -> PathBuf {
    repository_root().join("tools/mastodon-fixture")
}

fn temporary_fixture_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after the Unix epoch")
        .as_nanos();
    repository_root().join("target").join(format!(
        "mastodon-v4.6.5-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("temporary fixture directory should be created");
    for entry in fs::read_dir(source).expect("fixture directory should be readable") {
        let entry = entry.expect("fixture directory entry should be readable");
        let destination_path = destination.join(entry.file_name());
        if entry
            .file_type()
            .expect("file type should be readable")
            .is_dir()
        {
            copy_tree(&entry.path(), &destination_path);
        } else {
            fs::copy(entry.path(), destination_path).expect("fixture file should be copied");
        }
    }
}

fn run_git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn temporary_git_repository(label: &str) -> PathBuf {
    let repository = temporary_fixture_dir(label);
    fs::create_dir_all(&repository).expect("temporary Git repository should be created");
    run_git(&repository, &["init", "--quiet"]);
    fs::write(repository.join("tracked.txt"), "clean\n").expect("tracked file should be written");
    run_git(&repository, &["add", "tracked.txt"]);
    run_git(
        &repository,
        &[
            "-c",
            "user.name=Rustodon fixture test",
            "-c",
            "user.email=fixture-test@fixture.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture test repository",
        ],
    );
    repository
}

fn checksums() -> HashMap<String, String> {
    fs::read_to_string(fixture_dir().join("SHA256SUMS"))
        .expect("fixture checksums should exist")
        .lines()
        .map(|line| {
            let (hash, path) = line
                .split_once("  ")
                .expect("checksum line should contain a hash and path");
            (path.to_owned(), hash.to_owned())
        })
        .collect()
}

fn manifest() -> Value {
    let text = fs::read_to_string(fixture_dir().join("manifest.json"))
        .expect("the Mastodon fixture manifest should exist");
    serde_json::from_str(&text).expect("fixture manifest should be valid JSON")
}

#[test]
fn manifest_pins_the_exact_mastodon_baseline_and_fixture_labels() {
    let manifest = manifest();

    assert_eq!(manifest["release"], "v4.6.5");
    assert_eq!(manifest["revision"], REVISION);
    assert_eq!(manifest["schema_version"], "20260611150940");
    assert_eq!(manifest["schema_sha256"], SCHEMA_SHA256);
    assert_eq!(
        manifest["database_name"],
        "rustodon_mastodon_v4_6_5_fixture"
    );
    assert_eq!(manifest["local_domain"], "fixture-v4-6-5.rustodon.invalid");
    assert_eq!(manifest["platform"], "linux/amd64");
    assert_eq!(manifest["images"]["mastodon"]["index"], MASTODON_IMAGE);
    assert_eq!(
        manifest["images"]["mastodon"]["manifest_digest"],
        "sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf"
    );
    assert_eq!(
        manifest["images"]["postgres"]["manifest_digest"],
        "sha256:525844ca03edbc43a4c5fb8ca09ddef2a82bd96a9b4f826542833b7183d44c60"
    );
    assert_eq!(manifest["images"]["redis"]["index"], REDIS_IMAGE);
    assert_eq!(
        manifest["images"]["redis"]["manifest_digest"],
        "sha256:9702d01c1f10c3ea9f48211b4362e44f154ff02d063e6f7268eba804059f53bf"
    );
    assert_eq!(
        manifest["normalization"],
        serde_json::json!([
            "fixed timestamp_id() salt: rustodon-mastodon-v4.6.5-fixture-salt",
            "fixed Rails ar_internal_metadata timestamps: 2026-07-01T00:00:00Z",
            "fixed pg_dump restrict/unrestrict token: rustodonMastodon465FixtureDump",
            "remove only trailing empty pg_dump lines and enforce exactly one terminal LF, preserving all internal blank lines"
        ])
    );

    assert_eq!(manifest["labels"]["account.instance_actor"], -99);

    let expected_labels = [
        ("account.local.alice", 116_844_606_259_201_001_u64),
        ("status.visibility.public", 116_844_842_188_805_001),
        ("media.status.image", 116_844_842_188_806_001),
        ("quote.local_quotes_remote", 116_845_314_048_008_702),
        ("collection.remote", 116_845_549_977_608_801),
        ("collection_item.local_account", 116_845_549_977_608_802),
    ];
    for (label, id) in expected_labels {
        assert_eq!(manifest["labels"][label], id, "wrong ID for {label}");
    }

    for notification_type in [
        "mention",
        "status",
        "reblog",
        "follow",
        "follow_request",
        "favourite",
        "poll",
        "update",
        "severed_relationships",
        "moderation_warning",
        "annual_report",
        "admin.sign_up",
        "admin.report",
        "quote",
        "quoted_update",
        "added_to_collection",
        "collection_update",
    ] {
        assert!(
            manifest["labels"]
                .get(format!("notification.{notification_type}"))
                .is_some(),
            "manifest should label the {notification_type:?} notification"
        );
    }
}

#[test]
fn manifest_artifact_metadata_matches_checked_files() {
    let manifest = manifest();
    let sums = checksums();
    let database =
        fs::read(fixture_dir().join("database.sql")).expect("database dump should be readable");
    assert!(database.ends_with(b"\n"));
    assert!(!database.ends_with(b"\n\n"));

    for artifact in ["database", "catalog", "migrations"] {
        let metadata = &manifest["artifacts"][artifact];
        let path = metadata["path"]
            .as_str()
            .expect("artifact path should be a string");
        assert_eq!(
            metadata["bytes"].as_u64(),
            Some(
                fs::metadata(fixture_dir().join(path))
                    .expect("artifact should exist")
                    .len()
            ),
            "manifest byte count mismatch for {path}"
        );
        assert_eq!(
            metadata["sha256"].as_str(),
            sums.get(path).map(String::as_str),
            "manifest hash mismatch for {path}"
        );
    }

    for medium in manifest["media"]
        .as_array()
        .expect("media metadata should be an array")
    {
        let path = medium["path"]
            .as_str()
            .expect("media path should be a string");
        assert_eq!(
            medium["bytes"].as_u64(),
            Some(
                fs::metadata(fixture_dir().join(path))
                    .expect("media file should exist")
                    .len()
            )
        );
        assert_eq!(
            medium["sha256"].as_str(),
            sums.get(path).map(String::as_str)
        );
    }

    assert_eq!(
        manifest["media"][0]["path"],
        "media/accounts/avatars/116/844/606/259/201/001/original/0112603425bb49c1.png"
    );
    assert_eq!(manifest["media"][0]["dimensions"], "400x400");
    assert_eq!(
        manifest["media"][2]["path"],
        "media/media_attachments/files/116/844/842/188/806/001/small/cd63911ad76f4d5d.jpg"
    );
    assert_eq!(manifest["media"][2]["dimensions"], "588x392");
}

#[test]
fn safety_report_ignores_inherited_production_configuration() {
    let output = Command::new(fixture_tool())
        .arg("safety")
        .env("DATABASE_URL", "postgres://production.example/production")
        .env("DB_NAME", "production")
        .env("LOCAL_DOMAIN", "social.example")
        .env("SECRET_KEY_BASE", "production-secret-must-not-leak")
        .output()
        .expect("fixture safety report should run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("safety report should be UTF-8");
    assert!(stdout.contains("database=rustodon_mastodon_v4_6_5_fixture"));
    assert!(stdout.contains("domain=fixture-v4-6-5.rustodon.invalid"));
    assert!(!stdout.contains("production"));
    assert!(!stdout.contains("social.example"));
}

#[test]
fn generator_rejects_database_or_domain_overrides_before_starting_podman() {
    let output = Command::new(fixture_tool())
        .args(["generate", "--database=production"])
        .output()
        .expect("fixture generator should reject unsafe arguments");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("generator error should be UTF-8");
    assert!(
        stderr.contains("refusing unsupported fixture destination or override"),
        "unexpected error: {stderr}"
    );
}

#[test]
fn differential_command_is_documented_and_rejects_unsafe_case_names() {
    let help = Command::new(fixture_tool())
        .arg("help")
        .output()
        .expect("fixture help should run");
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("differential-test [CASE]"));
    assert!(String::from_utf8_lossy(&help.stdout).contains("preflight-test"));

    let output = Command::new(fixture_tool())
        .args(["differential-test", "../../unsafe"])
        .env("DATABASE_URL", "postgresql://production.example/production")
        .env("PAPERCLIP_ROOT_PATH", "/production/media")
        .output()
        .expect("unsafe differential case should be rejected");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("case must use lowercase letters, digits, or underscores")
    );

    let output = Command::new(fixture_tool())
        .args(["differential-test", "missing_case"])
        .output()
        .expect("unknown differential case should be rejected");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown differential case"));
}

#[test]
fn source_verifier_reports_a_revision_mismatch_clearly() {
    let repository = temporary_git_repository("wrong-revision");
    let output = Command::new(fixture_tool())
        .args(["verify-source", repository.to_str().expect("UTF-8 path")])
        .output()
        .expect("source verifier should run");
    fs::remove_dir_all(&repository).expect("temporary Git repository should be removable");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("source error should be UTF-8");
    assert!(
        stderr.contains("Mastodon source revision mismatch"),
        "unexpected error: {stderr}"
    );
}

#[test]
fn source_verifier_rejects_a_dirty_checkout_before_reading_files() {
    let repository = temporary_git_repository("dirty-source");
    fs::write(repository.join("tracked.txt"), "dirty\n").expect("tracked file should be modified");

    let output = Command::new(fixture_tool())
        .args(["verify-source", repository.to_str().expect("UTF-8 path")])
        .output()
        .expect("source verifier should run");
    fs::remove_dir_all(&repository).expect("temporary Git repository should be removable");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("source error should be UTF-8");
    assert!(
        stderr.contains("Mastodon source checkout is dirty"),
        "unexpected error: {stderr}"
    );
}

#[test]
fn static_verifier_rejects_a_tampered_artifact() {
    let temporary = temporary_fixture_dir("tampered");
    copy_tree(&fixture_dir(), &temporary);
    let catalog = temporary.join("catalog.txt");
    let mut contents = fs::read_to_string(&catalog).expect("catalog should be readable");
    contents.push_str("tampered\n");
    fs::write(catalog, contents).expect("temporary catalog should be writable");

    let output = Command::new(fixture_tool())
        .args(["verify", temporary.to_str().expect("UTF-8 path")])
        .output()
        .expect("static verifier should run");
    fs::remove_dir_all(&temporary).expect("temporary fixture should be removable");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("verification error should be UTF-8");
    assert!(
        stderr.contains("checksum mismatch"),
        "unexpected error: {stderr}"
    );
}
