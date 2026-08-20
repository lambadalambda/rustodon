use rustodon::mastodon::rest::{HtmlFormatter, MentionTarget};
use url::Url;

fn formatter() -> HtmlFormatter<'static> {
    let origin = Box::leak(Box::new(
        Url::parse("https://fixture-v4-6-5.rustodon.invalid/").unwrap(),
    ));
    HtmlFormatter::new(origin, "fixture-v4-6-5.rustodon.invalid")
}

#[test]
fn local_text_is_escaped_paragraphized_and_linkified_without_queries() {
    let mentions = [MentionTarget {
        username: "bob",
        domain: Some("remote.fixture.invalid"),
        url: "https://remote.fixture.invalid/@bob",
    }];
    let rendered = formatter().local_text(
        "Hello <script>@bob@remote.fixture.invalid #FixtureTag\nhttps://example.com/path",
        &mentions,
        None,
    );

    assert_eq!(
        rendered.as_str(),
        "<p>Hello &lt;script&gt;<span class=\"h-card\" translate=\"no\"><a href=\"https://remote.fixture.invalid/@bob\" class=\"u-url mention\">@<span>bob@remote.fixture.invalid</span></a></span> <a href=\"https://fixture-v4-6-5.rustodon.invalid/tags/FixtureTag\" class=\"mention hashtag\" rel=\"tag\">#<span>FixtureTag</span></a><br /><a href=\"https://example.com/path\" target=\"_blank\" rel=\"nofollow noopener\" translate=\"no\"><span class=\"invisible\">https://</span><span class=\"\">example.com/path</span><span class=\"invisible\"></span></a></p>"
    );
}

#[test]
fn local_quote_fallback_is_added_only_when_the_url_is_absent() {
    let url = "https://remote.fixture.invalid/@bob/123";
    let rendered = formatter().local_text("A local quote", &[], Some(url));
    assert!(
        rendered
            .as_str()
            .starts_with("<p class=\"quote-inline\">RE: ")
    );
    assert!(rendered.as_str().ends_with("<p>A local quote</p>"));

    let already_linked = formatter().local_text(url, &[], Some(url));
    assert!(!already_linked.as_str().contains("quote-inline"));
}

#[test]
fn local_profile_mentions_hide_remote_domains() {
    let mentions = [MentionTarget {
        username: "bob",
        domain: Some("remote.fixture.invalid"),
        url: "https://remote.fixture.invalid/@bob",
    }];

    assert_eq!(
        formatter()
            .local_profile_text("Hello @bob@remote.fixture.invalid", &mentions)
            .as_str(),
        "<p>Hello <span class=\"h-card\" translate=\"no\"><a href=\"https://remote.fixture.invalid/@bob\" class=\"u-url mention\">@<span>bob</span></a></span></p>"
    );
}

#[test]
fn strict_remote_sanitizer_matches_pinned_heading_link_list_and_ruby_cases() {
    let formatter = formatter();
    assert_eq!(
        formatter.remote_fragment("<h1>Foo</h1>").as_str(),
        "<p><strong>Foo</strong></p>"
    );
    assert_eq!(
        formatter
            .remote_fragment(
                "<p>Check:</p><ol start=\"3\" reversed=\"\"><li value=\"4\">Foo</li></ol>"
            )
            .as_str(),
        "<p>Check:</p><ol start=\"3\" reversed=\"\"><li value=\"4\">Foo</li></ol>"
    );
    assert_eq!(
        formatter
            .remote_fragment("<p><ruby>明日 <rp>(</rp><rt>Ashita</rt><rp>)</rp></ruby></p>")
            .as_str(),
        "<p><ruby>明日 <rp>(</rp><rt>Ashita</rt><rp>)</rp></ruby></p>"
    );
    assert_eq!(
        formatter
            .remote_fragment("<a href=\"http://example.com\" translate=\"no\">Test</a>")
            .as_str(),
        "<a href=\"http://example.com\" translate=\"no\" rel=\"nofollow noopener\" target=\"_blank\">Test</a>"
    );
}

#[test]
fn remote_plain_text_matches_filter_extraction_boundaries_and_entities() {
    assert_eq!(
        HtmlFormatter::remote_plain_text(
            "<p>first&#32;line<script>hidden</script></p></p><p>second<br><br />third &amp; fourth</p>"
        ),
        "first line\nsecond\nthird & fourth"
    );
}

#[test]
fn strict_remote_sanitizer_removes_scripts_classes_and_unsupported_links() {
    let formatter = formatter();
    let rendered = formatter.remote_fragment(
        "<script>alert(1)</script><p class=\"evil quote-inline\">Safe</p><a href=\"foo://bar\"><span class=\"invisible\">foo&amp;</span><span>Test</span></a>",
    );
    assert!(!rendered.as_str().contains("script"));
    assert!(!rendered.as_str().contains("evil"));
    assert_eq!(
        rendered.as_str(),
        "<p class=\"quote-inline\">Safe</p>foo&amp;Test"
    );
}

#[test]
fn strict_remote_sanitizer_uses_math_annotations() {
    let formatter = formatter();
    let inline = "<math><semantics><mi>x</mi><annotation encoding=\"application/x-tex\">x^n+y</annotation></semantics></math>";
    let block = "<math display=\"block\"><semantics><mi>x</mi><annotation encoding=\"application/x-tex\">x^n+y</annotation></semantics></math>";
    let plain = "<math><semantics><mi>x</mi><annotation encoding=\"text/plain\">sqrt(x)</annotation></semantics></math>";
    assert_eq!(formatter.remote_fragment(inline).as_str(), "$x^n+y$");
    assert_eq!(formatter.remote_fragment(block).as_str(), "$$x^n+y$$");
    assert_eq!(formatter.remote_fragment(plain).as_str(), "sqrt(x)");
    assert_eq!(
        formatter
            .remote_fragment("<math><semantics><annotation>x</annotation></semantics></math>")
            .as_str(),
        ""
    );
}

#[test]
fn oembed_sanitizer_forces_the_mastodon_iframe_sandbox() {
    let rendered = formatter().oembed_fragment(
        "<iframe src=\"https://video.invalid/embed\" onload=\"steal()\"></iframe><iframe src=\"javascript:steal()\"></iframe>",
    );
    assert_eq!(
        rendered.as_str(),
        "<iframe src=\"https://video.invalid/embed\" sandbox=\"allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox allow-forms\"></iframe><iframe sandbox=\"allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox allow-forms\"></iframe>"
    );
}
