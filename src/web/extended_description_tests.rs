//! Source-backed serializer corpus for the cached Mastodon 4.6.5 contract.
//! See docs/extended-description.md for provenance and dialect limitations.
use super::extended_description::{description, render_markdown};
use crate::mastodon::RawYamlText;
use chrono::NaiveDate;
use serde_json::json;

#[test]
fn extended_description_empty_and_blank_discard_timestamp() {
    let timestamp = NaiveDate::from_ymd_opt(2024, 11, 28)
        .unwrap()
        .and_hms_opt(16, 20, 0)
        .unwrap();
    for yaml in [
        None,
        Some("---\n"),
        Some("--- ''\n"),
        Some("--- false\n"),
        Some("--- \" \\t\\n\\u00a0\"\n"),
    ] {
        let raw = yaml.map(|raw| RawYamlText::new(raw.to_owned()));
        assert_eq!(
            description(raw.as_ref(), Some(timestamp)).unwrap(),
            json!({"updated_at": null, "content": ""}),
            "{yaml:?}",
        );
    }
}

#[test]
fn extended_description_yaml_and_timestamp_match_pinned_serializer() {
    let timestamp = NaiveDate::from_ymd_opt(2024, 11, 28)
        .unwrap()
        .and_hms_micro_opt(16, 20, 0, 987_654)
        .unwrap();
    for yaml in [
        "--- Hello world\n",
        "--- 'Hello world'\n",
        "--- |\n  Hello world\n",
        "--- >-\n  Hello\n  world\n",
    ] {
        let raw = RawYamlText::new(yaml.to_owned());
        assert_eq!(
            description(Some(&raw), Some(timestamp)).unwrap(),
            json!({"updated_at": "2024-11-28T16:20:00+00:00", "content": "<p>Hello world</p>\n"}),
        );
        assert_eq!(
            description(Some(&raw), None).unwrap(),
            json!({"updated_at": null, "content": "<p>Hello world</p>\n"}),
        );
    }
}

#[test]
fn extended_description_invalid_configuration_is_not_silently_empty() {
    for yaml in [
        "--- [",
        "--- 42\n",
        "--- {text: hi}\n",
        "--- hello\n--- world\n",
    ] {
        assert!(description(Some(&RawYamlText::new(yaml.to_owned())), None).is_err());
    }
}

#[test]
fn extended_description_redcarpet_supported_corpus() {
    for (markdown, html) in [
        ("Hello world", "<p>Hello world</p>\n"),
        ("first\n\nsecond", "<p>first</p>\n\n<p>second</p>\n"),
        (
            "## About\n\nHello **world** & friends.",
            "<h2>About</h2>\n\n<p>Hello <strong>world</strong> &amp; friends.</p>\n",
        ),
        (
            "[Rules](https://example.org/rules) and <em>welcome</em>",
            "<p><a href=\"https://example.org/rules\">Rules</a> and <em>welcome</em></p>\n",
        ),
        ("- one\n- two", "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n"),
        (
            "> hello\n>\n> world",
            "<blockquote>\n<p>hello</p>\n\n<p>world</p>\n</blockquote>\n",
        ),
        ("hello  \nworld", "<p>hello<br>\nworld</p>\n"),
        ("hello\nworld", "<p>hello\nworld</p>\n"),
        (
            "*emphasis* and `a < b`",
            "<p><em>emphasis</em> and <code>a &lt; b</code></p>\n",
        ),
        ("---", "<hr>\n"),
        (
            "<div>Trusted admin HTML</div>\n",
            "<div>Trusted admin HTML</div>\n",
        ),
    ] {
        assert_eq!(render_markdown(markdown), html, "{markdown:?}");
    }
}

#[test]
fn extended_description_does_not_rewrite_trusted_html_block_contents() {
    let markdown = "<pre>\n</p>\n<p>not Markdown</p>\n</pre>\n";
    assert_eq!(render_markdown(markdown), markdown);
}
