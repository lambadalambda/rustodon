use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use ammonia::{Builder, UrlRelative};
use linkify::{LinkFinder, LinkKind};
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedHtml(String);

impl RenderedHtml {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MentionTarget<'a> {
    pub username: &'a str,
    pub domain: Option<&'a str>,
    pub url: &'a str,
}

#[derive(Clone, Debug)]
pub struct HtmlFormatter<'a> {
    origin: &'a Url,
    local_domain: &'a str,
}

impl<'a> HtmlFormatter<'a> {
    #[must_use]
    pub const fn new(origin: &'a Url, local_domain: &'a str) -> Self {
        Self {
            origin,
            local_domain,
        }
    }

    #[must_use]
    pub fn local_text(
        &self,
        text: &str,
        mentions: &[MentionTarget<'_>],
        quoted_status_url: Option<&str>,
    ) -> RenderedHtml {
        let inline = self.linkify_local(text, mentions, true);
        let mut html = simple_format(&inline);
        if let Some(url) = quoted_status_url.filter(|url| !url.is_empty() && !html.contains(url)) {
            html = format!(
                "<p class=\"quote-inline\">RE: {}</p>{html}",
                shortened_link(url)
            );
        }
        RenderedHtml(html)
    }

    #[must_use]
    pub fn local_inline(&self, text: &str, mentions: &[MentionTarget<'_>]) -> RenderedHtml {
        RenderedHtml(self.linkify_local(text, mentions, true))
    }

    #[must_use]
    pub fn local_profile_text(&self, text: &str, mentions: &[MentionTarget<'_>]) -> RenderedHtml {
        RenderedHtml(simple_format(&self.linkify_local(text, mentions, false)))
    }

    #[must_use]
    pub fn local_profile_inline(&self, text: &str, mentions: &[MentionTarget<'_>]) -> RenderedHtml {
        RenderedHtml(self.linkify_local(text, mentions, false))
    }

    #[must_use]
    pub fn remote_fragment(&self, html: &str) -> RenderedHtml {
        let transformed = transform_math(transform_headings(html.to_owned()));
        let builder = strict_builder();
        let sanitized = builder.clean(&transformed).to_string().replace(
            " target=\"_blank\" rel=\"nofollow noopener\"",
            " rel=\"nofollow noopener\" target=\"_blank\"",
        );
        RenderedHtml(strip_invalid_anchors(&sanitized))
    }

    #[must_use]
    pub fn oembed_fragment(&self, html: &str) -> RenderedHtml {
        let builder = oembed_builder();
        RenderedHtml(builder.clean(html).to_string())
    }

    #[must_use]
    pub fn remote_plain_text(html: &str) -> String {
        let with_newlines = insert_html_newlines(html);
        let clean_content_tags = [
            "iframe",
            "math",
            "noembed",
            "noframes",
            "noscript",
            "plaintext",
            "script",
            "style",
            "svg",
            "xmp",
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        let mut builder = Builder::new();
        builder
            .tags(HashSet::new())
            .generic_attributes(HashSet::new())
            .clean_content_tags(clean_content_tags);
        html_escape::decode_html_entities(&builder.clean(&with_newlines).to_string())
            .trim_end_matches('\n')
            .to_owned()
    }

    fn linkify_local(
        &self,
        text: &str,
        mentions: &[MentionTarget<'_>],
        display_domains: bool,
    ) -> String {
        let mut finder = LinkFinder::new();
        finder.kinds(&[LinkKind::Url]);
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;
        for link in finder.links(text) {
            output.push_str(&self.linkify_mentions_and_tags(
                &text[cursor..link.start()],
                mentions,
                display_domains,
            ));
            output.push_str(&shortened_link(link.as_str()));
            cursor = link.end();
        }
        output.push_str(&self.linkify_mentions_and_tags(
            &text[cursor..],
            mentions,
            display_domains,
        ));
        output
    }

    fn linkify_mentions_and_tags(
        &self,
        text: &str,
        mentions: &[MentionTarget<'_>],
        display_domains: bool,
    ) -> String {
        let mut output = String::with_capacity(text.len());
        let mut plain_start = 0;
        let mut cursor = 0;
        while cursor < text.len() {
            let Some(character) = text[cursor..].chars().next() else {
                break;
            };
            if character == '@'
                && let Some((end, target, with_domain)) =
                    mention_at(text, cursor, mentions, self.local_domain)
            {
                output.push_str(&escape_html(&text[plain_start..cursor]));
                let display = if with_domain && display_domains {
                    match target.domain {
                        Some(domain) => format!("{}@{}", target.username, idna_display(domain)),
                        None => target.username.to_owned(),
                    }
                } else {
                    target.username.to_owned()
                };
                write!(
                    output,
                    "<span class=\"h-card\" translate=\"no\"><a href=\"{}\" class=\"u-url mention\">@<span>{}</span></a></span>",
                    escape_attribute(target.url),
                    escape_html(&display)
                )
                .expect("writing to a String cannot fail");
                cursor = end;
                plain_start = end;
                continue;
            }
            if character == '#'
                && let Some(end) = hashtag_end(text, cursor)
            {
                output.push_str(&escape_html(&text[plain_start..cursor]));
                let tag = &text[cursor + 1..end];
                let path = format!("tags/{tag}");
                let url = self
                    .origin
                    .join(&path)
                    .map_or_else(|_| path, |url| url.to_string());
                write!(
                    output,
                    "<a href=\"{}\" class=\"mention hashtag\" rel=\"tag\">#<span>{}</span></a>",
                    escape_attribute(&url),
                    escape_html(tag)
                )
                .expect("writing to a String cannot fail");
                cursor = end;
                plain_start = end;
                continue;
            }
            cursor += character.len_utf8();
        }
        output.push_str(&escape_html(&text[plain_start..]));
        output
    }
}

fn mention_at<'a>(
    text: &str,
    start: usize,
    mentions: &'a [MentionTarget<'a>],
    local_domain: &str,
) -> Option<(usize, &'a MentionTarget<'a>, bool)> {
    if start > 0
        && text[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_')
    {
        return None;
    }
    let mut end = start + 1;
    while let Some(character) = text[end..].chars().next() {
        if character.is_alphanumeric() || character == '_' {
            end += character.len_utf8();
        } else {
            break;
        }
    }
    if end == start + 1 {
        return None;
    }
    let username = &text[start + 1..end];
    let mut domain = None;
    if text[end..].starts_with('@') {
        let domain_start = end + 1;
        let mut domain_end = domain_start;
        while let Some(character) = text[domain_end..].chars().next() {
            if character.is_alphanumeric() || matches!(character, '.' | '-') {
                domain_end += character.len_utf8();
            } else {
                break;
            }
        }
        if domain_end > domain_start {
            domain = Some(&text[domain_start..domain_end]);
            end = domain_end;
        }
    }
    let normalized_domain = domain.filter(|domain| !domain.eq_ignore_ascii_case(local_domain));
    let target = mentions.iter().find(|target| {
        target.username.eq_ignore_ascii_case(username)
            && match (target.domain, normalized_domain) {
                (None, None) => true,
                (Some(target), Some(actual)) => target.eq_ignore_ascii_case(actual),
                _ => false,
            }
    })?;
    Some((end, target, domain.is_some()))
}

fn hashtag_end(text: &str, start: usize) -> Option<usize> {
    if start > 0
        && text[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_')
    {
        return None;
    }
    let mut end = start + 1;
    while let Some(character) = text[end..].chars().next() {
        if character.is_alphanumeric() || character == '_' {
            end += character.len_utf8();
        } else {
            break;
        }
    }
    (end > start + 1).then_some(end)
}

fn simple_format(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }
    let normalized = html.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .split("\n\n")
        .filter(|paragraph| !paragraph.is_empty())
        .fold(String::new(), |mut output, paragraph| {
            write!(output, "<p>{}</p>", paragraph.replace('\n', "<br />"))
                .expect("writing to a String cannot fail");
            output
        })
}

fn shortened_link(url: &str) -> String {
    let escaped_url = escape_attribute(url);
    let prefix_length = [
        "https://www.",
        "http://www.",
        "https://",
        "http://",
        "xmpp:",
    ]
    .into_iter()
    .find(|prefix| url.starts_with(prefix))
    .map_or(0, str::len);
    let prefix = &url[..prefix_length];
    let remainder = &url[prefix_length..];
    let mut display_end = remainder
        .char_indices()
        .nth(30)
        .map_or(remainder.len(), |(index, _)| index);
    let mut suffix = &remainder[display_end..];
    if suffix.chars().count() == 1 {
        display_end = remainder.len();
        suffix = "";
    }
    let display = &remainder[..display_end];
    let ellipsis_class = if suffix.is_empty() { "" } else { "ellipsis" };
    format!(
        "<a href=\"{escaped_url}\" target=\"_blank\" rel=\"nofollow noopener\" translate=\"no\"><span class=\"invisible\">{}</span><span class=\"{ellipsis_class}\">{}</span><span class=\"invisible\">{}</span></a>",
        escape_html(prefix),
        escape_html(display),
        escape_html(suffix)
    )
}

fn strict_builder() -> Builder<'static> {
    let tags = [
        "p",
        "br",
        "span",
        "a",
        "del",
        "s",
        "pre",
        "blockquote",
        "code",
        "b",
        "strong",
        "u",
        "i",
        "em",
        "ul",
        "ol",
        "li",
        "ruby",
        "rt",
        "rp",
    ]
    .into_iter()
    .collect::<HashSet<_>>();
    let tag_attributes = HashMap::from([
        ("a", HashSet::from(["href", "class", "translate"])),
        ("span", HashSet::from(["class", "translate"])),
        ("ol", HashSet::from(["start", "reversed"])),
        ("li", HashSet::from(["value"])),
        ("p", HashSet::from(["class"])),
    ]);
    let schemes = [
        "http", "https", "dat", "dweb", "ipfs", "ipns", "ssb", "gopher", "xmpp", "magnet", "gemini",
    ]
    .into_iter()
    .collect::<HashSet<_>>();
    let mut builder = Builder::new();
    builder
        .tags(tags)
        .generic_attributes(HashSet::from(["lang"]))
        .tag_attributes(tag_attributes)
        .url_schemes(schemes)
        .url_relative(UrlRelative::Deny)
        .link_rel(Some("nofollow noopener"))
        .set_tag_attribute_value("a", "target", "_blank")
        .attribute_filter(|_element, attribute, value| match attribute {
            "translate" => (value == "no").then_some(Cow::Borrowed(value)),
            "class" => {
                let classes = value
                    .split_ascii_whitespace()
                    .filter(|class| {
                        matches!(
                            *class,
                            "mention" | "hashtag" | "ellipsis" | "invisible" | "quote-inline"
                        ) || ["h-", "p-", "u-", "dt-", "e-"]
                            .iter()
                            .any(|prefix| class.starts_with(prefix))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                (!classes.is_empty()).then_some(Cow::Owned(classes))
            }
            _ => Some(Cow::Borrowed(value)),
        });
    builder
}

fn oembed_builder() -> Builder<'static> {
    let tags = ["audio", "iframe", "source", "video"]
        .into_iter()
        .collect::<HashSet<_>>();
    let tag_attributes = HashMap::from([
        ("audio", HashSet::from(["controls"])),
        (
            "iframe",
            HashSet::from([
                "allowfullscreen",
                "frameborder",
                "height",
                "scrolling",
                "src",
                "width",
            ]),
        ),
        ("source", HashSet::from(["src", "type"])),
        (
            "video",
            HashSet::from(["controls", "height", "loop", "width"]),
        ),
    ]);
    let mut builder = Builder::new();
    builder
        .tags(tags)
        .generic_attributes(HashSet::new())
        .tag_attributes(tag_attributes)
        .url_schemes(HashSet::from(["http", "https"]))
        .url_relative(UrlRelative::Deny)
        .link_rel(None)
        .set_tag_attribute_value(
            "iframe",
            "sandbox",
            "allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox allow-forms",
        );
    builder
}

fn transform_headings(mut html: String) -> String {
    for level in 1..=6 {
        html = html.replace(&format!("<h{level}>"), "<p><strong>");
        html = html.replace(&format!("</h{level}>"), "</strong></p>");
    }
    html
}

fn transform_math(mut html: String) -> String {
    let mut cursor = 0;
    while let Some(relative_start) = html[cursor..].find("<math") {
        let start = cursor + relative_start;
        let Some(relative_end) = html[start..].find("</math>") else {
            html.truncate(start);
            break;
        };
        let end = start + relative_end + "</math>".len();
        let math = &html[start..end];
        let block = math
            .split_once('>')
            .is_some_and(|(opening, _)| opening.contains("display=\"block\""));
        let replacement = annotation(math, "application/x-tex")
            .map(|text| {
                if block {
                    format!("$${text}$$")
                } else {
                    format!("${text}$")
                }
            })
            .or_else(|| annotation(math, "text/plain"))
            .unwrap_or_default();
        html.replace_range(start..end, &escape_html(&replacement));
        cursor = start + replacement.len();
    }
    html
}

fn annotation(math: &str, encoding: &str) -> Option<String> {
    let marker = format!("encoding=\"{encoding}\"");
    let marker_start = math.find(&marker)?;
    let content_start = marker_start + math[marker_start..].find('>')? + 1;
    let content_end = content_start + math[content_start..].find("</annotation>")?;
    Some(strip_tags(&math[content_start..content_end]))
}

fn strip_invalid_anchors(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(relative_start) = html[cursor..].find("<a") {
        let start = cursor + relative_start;
        output.push_str(&html[cursor..start]);
        let Some(relative_open_end) = html[start..].find('>') else {
            output.push_str(&html[start..]);
            return output;
        };
        let open_end = start + relative_open_end + 1;
        let opening = &html[start..open_end];
        let Some(relative_close) = html[open_end..].find("</a>") else {
            output.push_str(&html[start..]);
            return output;
        };
        let close = open_end + relative_close;
        if opening.contains(" href=\"") {
            output.push_str(&html[start..close + "</a>".len()]);
        } else {
            output.push_str(&strip_tags(&html[open_end..close]));
        }
        cursor = close + "</a>".len();
    }
    output.push_str(&html[cursor..]);
    output
}

fn strip_tags(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output
}

fn insert_html_newlines(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut cursor = 0;
    while cursor < html.len() {
        let suffix = &html[cursor..];
        let Some(relative) = ["<br />", "<br>", "</p>"]
            .into_iter()
            .filter_map(|tag| suffix.find(tag).map(|index| (index, tag)))
            .min_by_key(|(index, _)| *index)
        else {
            output.push_str(suffix);
            break;
        };
        let start = cursor + relative.0;
        output.push_str(&html[cursor..start]);
        let mut end = start;
        while let Some(tag) = ["<br />", "<br>", "</p>"]
            .into_iter()
            .find(|tag| html[end..].starts_with(tag))
        {
            output.push_str(tag);
            end += tag.len();
        }
        output.push('\n');
        cursor = end;
    }
    output
}

fn idna_display(domain: &str) -> String {
    idna::domain_to_unicode(domain).0
}

fn escape_html(value: &str) -> String {
    value.chars().fold(
        String::with_capacity(value.len()),
        |mut output, character| {
            output.push_str(match character {
                '&' => "&amp;",
                '<' => "&lt;",
                '>' => "&gt;",
                '"' => "&quot;",
                '\'' => "&#39;",
                _ => {
                    output.push(character);
                    return output;
                }
            });
            output
        },
    )
}

fn escape_attribute(value: &str) -> String {
    escape_html(value)
}
