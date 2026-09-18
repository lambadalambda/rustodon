#![cfg(feature = "test-support")]

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use image::ImageReader;
use rustodon::media::{AUDIO_VIDEO_SIZE_LIMIT, IMAGE_SIZE_LIMIT, MediaKind};
use rustodon::paperclip::{
    MediaAttachmentError, MediaProcessorConfig, prepare_rich_media_attachment,
    prepare_rich_media_attachment_with_config, validate_media_processor_capabilities,
    validate_media_processor_capabilities_with_config,
};
use serde_json::Value;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new("tests/fixtures/media").join(name)).expect("read media fixture")
}

fn probe_bytes(bytes: &[u8], demuxer: &str) -> Value {
    let mut child = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "pipe",
            "-f",
            demuxer,
            "-i",
            "pipe:0",
            "-show_entries",
            "format=format_name,duration:stream=codec_type,codec_name,pix_fmt",
            "-of",
            "json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ffprobe");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bytes)
        .expect("feed ffprobe");
    let output = child.wait_with_output().expect("wait for ffprobe");
    assert!(
        output.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse ffprobe JSON")
}

#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe with AVIF, HEIC, H.264, and MP3 support"]
async fn rich_media_processor_generates_bounded_browser_outputs() {
    let artifacts_before = media_task_artifacts();
    for (name, content_type) in [
        ("600x400.avif", "image/avif"),
        ("600x400.heic", "image/heic"),
    ] {
        let prepared = prepare_rich_media_attachment(7, name, content_type, &fixture(name))
            .await
            .expect("convert modern still");
        assert_eq!(prepared.media_kind, MediaKind::Image);
        assert_eq!(prepared.content_type, "image/jpeg");
        assert!(prepared.file_name.ends_with(".jpg"));
        assert_eq!(prepared.small_content_type.as_deref(), Some("image/jpeg"));
        assert!(
            prepared
                .small_file_name
                .as_deref()
                .unwrap()
                .ends_with(".jpg")
        );
        assert!(prepared.original_bytes.len() < IMAGE_SIZE_LIMIT);
        assert!(prepared.small_bytes.as_ref().unwrap().len() < IMAGE_SIZE_LIMIT);
        ImageReader::new(std::io::Cursor::new(&prepared.original_bytes))
            .with_guessed_format()
            .unwrap()
            .decode()
            .expect("converted original is JPEG-decodable");
    }

    let video = prepare_rich_media_attachment(
        7,
        "attachment.webm",
        "video/webm",
        &fixture("attachment.webm"),
    )
    .await
    .expect("convert video");
    assert_eq!(video.media_kind, MediaKind::Video);
    assert_eq!(video.content_type, "video/mp4");
    assert!(video.file_name.ends_with(".mp4"));
    assert_eq!(video.small_content_type.as_deref(), Some("image/png"));
    assert!(video.small_file_name.as_deref().unwrap().ends_with(".png"));
    assert!(video.original_bytes.len() < AUDIO_VIDEO_SIZE_LIMIT);
    assert!(video.small_bytes.as_ref().unwrap().len() < IMAGE_SIZE_LIMIT);
    assert!(video.file_meta["original"]["duration"].as_f64().unwrap() > 0.0);
    let video_probe = probe_bytes(&video.original_bytes, "mov");
    assert_eq!(
        video_probe["format"]["format_name"],
        "mov,mp4,m4a,3gp,3g2,mj2"
    );
    let streams = video_probe["streams"].as_array().unwrap();
    assert_eq!(
        streams
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap()["codec_name"],
        "h264"
    );
    assert_eq!(
        streams
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap()["pix_fmt"],
        "yuv420p"
    );
    assert!(
        streams
            .iter()
            .all(|stream| { stream["codec_type"] != "audio" || stream["codec_name"] == "aac" })
    );
    ImageReader::new(std::io::Cursor::new(video.small_bytes.as_ref().unwrap()))
        .with_guessed_format()
        .unwrap()
        .decode()
        .expect("video preview is image-decodable");

    let audio = prepare_rich_media_attachment(7, "boop.ogg", "audio/ogg", &fixture("boop.ogg"))
        .await
        .expect("convert audio");
    assert_eq!(audio.media_kind, MediaKind::Audio);
    assert_eq!(audio.content_type, "audio/mpeg");
    assert!(audio.file_name.ends_with(".mp3"));
    assert!(audio.original_bytes.len() < AUDIO_VIDEO_SIZE_LIMIT);
    assert!(audio.small_file_name.is_none());
    assert!(audio.small_content_type.is_none());
    assert!(audio.small_bytes.is_none());
    assert!(audio.file_meta["original"]["duration"].as_f64().unwrap() > 0.0);
    let audio_probe = probe_bytes(&audio.original_bytes, "mp3");
    assert_eq!(audio_probe["format"]["format_name"], "mp3");
    assert!(
        audio_probe["streams"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stream| stream["codec_type"] != "audio" || stream["codec_name"] == "mp3")
    );
    assert_eq!(media_task_artifacts(), artifacts_before);
}

#[tokio::test]
#[ignore = "requires the complete production ffmpeg and ffprobe capability set"]
async fn rich_media_processor_capability_check_exercises_every_required_path() {
    for (name, content_type) in [
        ("600x400.avif", "image/avif"),
        ("600x400.heic", "image/heic"),
        ("attachment.webm", "video/webm"),
        ("boop.ogg", "audio/ogg"),
        ("capability.mp4", "video/mp4"),
        ("capability.mov", "video/quicktime"),
        ("capability-audio.webm", "audio/webm"),
        ("capability-video.ogg", "video/ogg"),
        ("capability.wav", "audio/wav"),
        ("capability.mp3", "audio/mpeg"),
        ("capability.flac", "audio/flac"),
        ("capability.aac", "audio/aac"),
        ("capability.m4a", "audio/m4a"),
        ("capability.3gp", "audio/3gpp"),
        ("capability.asf", "video/x-ms-asf"),
    ] {
        prepare_rich_media_attachment(1, name, content_type, &fixture(name))
            .await
            .unwrap_or_else(|error| panic!("{name} ({content_type}) capability failed: {error}"));
    }
    validate_media_processor_capabilities()
        .await
        .expect("production processor capabilities");
}

#[tokio::test]
async fn rich_media_processor_capability_check_fails_when_tools_are_missing() {
    let missing = std::env::temp_dir().join(format!(
        "rustodon-missing-media-tool-{}",
        std::process::id()
    ));
    let config = MediaProcessorConfig::new(&missing, &missing, Duration::from_millis(100));
    assert_eq!(
        validate_media_processor_capabilities_with_config(&config)
            .await
            .unwrap_err(),
        MediaAttachmentError::ProcessingUnavailable,
    );
}

#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe"]
async fn rich_media_processor_rejects_declared_type_mismatch() {
    let error =
        prepare_rich_media_attachment(7, "not-video.mp4", "video/mp4", &fixture("600x400.avif"))
            .await
            .expect_err("still image must not pass as video");
    assert_eq!(error, MediaAttachmentError::InvalidMedia);

    let error = prepare_rich_media_attachment(
        7,
        "not-quicktime.mov",
        "video/quicktime",
        &fixture("capability.mp4"),
    )
    .await
    .expect_err("MP4 brand must not pass as QuickTime");
    assert_eq!(error, MediaAttachmentError::InvalidMedia);
}

fn media_task_artifacts() -> Vec<String> {
    let mut entries = std::fs::read_dir(std::env::temp_dir())
        .expect("read temporary directory")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("rustodon-media-"))
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

#[cfg(unix)]
mod supervisor_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SCRIPT_ID: AtomicU64 = AtomicU64::new(0);

    struct ScriptWorkspace {
        path: std::path::PathBuf,
    }

    impl ScriptWorkspace {
        fn new() -> Self {
            let id = SCRIPT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rustodon-processor-test-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create script workspace");
            Self { path }
        }

        fn script(&self, name: &str, body: &str) -> std::path::PathBuf {
            let path = self.path.join(name);
            std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n"))
                .expect("write fake executable");
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&path, permissions).unwrap();
            path
        }
    }

    impl Drop for ScriptWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn config(script: std::path::PathBuf, timeout: Duration) -> MediaProcessorConfig {
        MediaProcessorConfig::new(&script, &script, timeout)
    }

    #[tokio::test]
    async fn supervisor_bounds_stdout_and_leaves_no_media_artifacts() {
        let before = media_task_artifacts();
        let workspace = ScriptWorkspace::new();
        let executable = workspace.script("overflow-stdout", "head -c 70000 /dev/zero\nsleep 30");
        let error = prepare_rich_media_attachment_with_config(
            1,
            "input.webm",
            "video/webm",
            &fixture("attachment.webm"),
            &config(executable, Duration::from_secs(5)),
        )
        .await
        .expect_err("probe stdout overflow must fail");
        assert_eq!(error, MediaAttachmentError::SizeOverflow);
        assert_eq!(media_task_artifacts(), before);
    }

    #[tokio::test]
    async fn supervisor_bounds_stderr_while_reading() {
        let workspace = ScriptWorkspace::new();
        let executable =
            workspace.script("overflow-stderr", "head -c 70000 /dev/zero >&2\nsleep 30");
        let error = prepare_rich_media_attachment_with_config(
            1,
            "input.webm",
            "video/webm",
            &fixture("attachment.webm"),
            &config(executable, Duration::from_secs(5)),
        )
        .await
        .expect_err("probe stderr overflow must fail");
        assert_eq!(error, MediaAttachmentError::SizeOverflow);
    }

    #[tokio::test]
    async fn supervisor_enforces_total_deadline() {
        let workspace = ScriptWorkspace::new();
        let executable = workspace.script("timeout", "sleep 30");
        let error = prepare_rich_media_attachment_with_config(
            1,
            "input.webm",
            "video/webm",
            &fixture("attachment.webm"),
            &config(executable, Duration::from_millis(50)),
        )
        .await
        .expect_err("deadline must fail");
        assert_eq!(error, MediaAttachmentError::ProcessingTimedOut);
    }

    #[tokio::test]
    async fn successful_early_stdin_close_is_not_a_processing_failure() {
        let workspace = ScriptWorkspace::new();
        let executable = workspace.script(
            "early-close",
            r#"if [ "$1" = "-v" ]; then
  printf '%s' '{"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2"},"streams":[{"codec_type":"video","codec_name":"av1","width":600,"height":400,"nb_read_frames":"1","disposition":{"attached_pic":0}}]}'
else
  cat tests/fixtures/media/attachment.jpg
fi"#,
        );
        let prepared = prepare_rich_media_attachment_with_config(
            1,
            "input.avif",
            "image/avif",
            &fixture("600x400.avif"),
            &config(executable, Duration::from_secs(5)),
        )
        .await
        .expect("successful early-closing children must be accepted");
        assert_eq!(prepared.content_type, "image/jpeg");
    }

    #[tokio::test]
    async fn processor_deadline_is_shared_across_children() {
        let workspace = ScriptWorkspace::new();
        let executable = workspace.script(
            "shared-deadline",
            r#"cat >/dev/null
sleep 0.07
if [ "$1" = "-v" ]; then
  printf '%s' '{"format":{"format_name":"matroska,webm","duration":"1.0"},"streams":[{"codec_type":"video","codec_name":"vp8","pix_fmt":"yuv420p","width":640,"height":480,"avg_frame_rate":"30/1","nb_read_frames":"30","disposition":{"attached_pic":0}}]}'
else
  printf 'normalized-output'
fi"#,
        );
        let error = prepare_rich_media_attachment_with_config(
            1,
            "input.webm",
            "video/webm",
            &fixture("attachment.webm"),
            &config(executable, Duration::from_millis(100)),
        )
        .await
        .expect_err("sequential children must share one deadline");
        assert_eq!(error, MediaAttachmentError::ProcessingTimedOut);
    }

    #[tokio::test]
    async fn cancelling_outer_future_kills_and_reaps_child() {
        let workspace = ScriptWorkspace::new();
        let executable = workspace.script("cancel", "echo $$ > \"$0.pid\"\nsleep 30");
        let pid_path = executable.with_extension("pid");
        let processor_config = config(executable, Duration::from_secs(30));
        let task = tokio::spawn(async move {
            prepare_rich_media_attachment_with_config(
                1,
                "input.webm",
                "video/webm",
                &fixture("attachment.webm"),
                &processor_config,
            )
            .await
        });

        let pid = loop {
            if let Ok(pid) = std::fs::read_to_string(&pid_path) {
                break pid.trim().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        task.abort();
        let _ = task.await;

        for _ in 0..100 {
            if !Command::new("kill")
                .args(["-0", &pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("check child process")
                .success()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("cancelled processor child {pid} was not killed and reaped");
    }
}
