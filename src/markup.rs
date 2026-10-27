//! Markdown and HTML as silent shows them rendered: the pictures they draw,
//! found before the text view parses them, so that pictures beside the file
//! are decoded ahead and the others, which silent never fetches, become
//! their alternative text.

use std::ops::Range;
use std::path::{Path, PathBuf};

use markdown::mdast::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Markup {
    Markdown,
    Html,
}

impl Markup {
    /// The markup a file's name says it is in.
    pub fn of(path: &Path) -> Option<Markup> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "md" | "markdown" | "mdown" | "mkd" | "mkdn" => Some(Markup::Markdown),
            "html" | "htm" | "xhtml" => Some(Markup::Html),
            _ => None,
        }
    }
}

/// A picture in the text: the bytes that draw it, where it comes from, and
/// what it says in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub span: Range<usize>,
    pub url: String,
    pub alt: String,
    /// An `<img>` tag rather than Markdown's `![]()`.
    pub tag: bool,
}

/// The pictures of `text`, in order.
pub fn pictures(text: &str, markup: Markup) -> Vec<Picture> {
    // Parsing a long text costs as much as showing it; most have no picture.
    if !text.contains("![") && !text.to_ascii_lowercase().contains("<img") {
        return Vec::new();
    }
    match markup {
        Markup::Html => tags(text, 0),
        Markup::Markdown => {
            let Ok(root) = markdown::to_mdast(text, &markdown::ParseOptions::gfm()) else {
                return Vec::new();
            };
            let mut definitions = Vec::new();
            collect_definitions(&root, &mut definitions);
            let mut found = Vec::new();
            collect(&root, &definitions, text, &mut found);
            found.sort_by_key(|p| p.span.start);
            found
        }
    }
}

fn collect_definitions(node: &Node, out: &mut Vec<(String, String)>) {
    if let Node::Definition(definition) = node {
        out.push((definition.identifier.clone(), definition.url.clone()));
    }
    for child in node.children().into_iter().flatten() {
        collect_definitions(child, out);
    }
}

fn collect(node: &Node, definitions: &[(String, String)], text: &str, out: &mut Vec<Picture>) {
    let span = |node: &Node| node.position().map(|p| p.start.offset..p.end.offset);
    match node {
        Node::Image(image) => out.extend(span(node).map(|span| Picture {
            span,
            url: image.url.clone(),
            alt: image.alt.clone(),
            tag: false,
        })),
        Node::ImageReference(image) => {
            let url = definitions
                .iter()
                .find(|(identifier, _)| *identifier == image.identifier)
                .map(|(_, url)| url.clone());
            if let (Some(span), Some(url)) = (span(node), url) {
                out.push(Picture {
                    span,
                    url,
                    alt: image.alt.clone(),
                    tag: false,
                });
            }
        }
        // The source rather than the value, which loses a blockquote's `>`.
        Node::Html(_) => {
            if let Some(span) = span(node).filter(|span| text.get(span.clone()).is_some()) {
                out.extend(tags(&text[span.clone()], span.start));
            }
        }
        _ => {}
    }
    for child in node.children().into_iter().flatten() {
        collect(child, definitions, text, out);
    }
}

/// The `<img>` tags of `html`, which starts `offset` bytes into the text.
fn tags(html: &str, offset: usize) -> Vec<Picture> {
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<img") {
        let start = from + at;
        let after = lower.as_bytes().get(start + 4).copied().unwrap_or(b'>');
        let Some(end) = tag_end(html, start) else {
            break;
        };
        from = end;
        if !(after.is_ascii_whitespace() || after == b'>' || after == b'/') {
            continue;
        }
        let tag = &html[start..end];
        if let Some(url) = attribute(tag, "src") {
            found.push(Picture {
                span: offset + start..offset + end,
                url,
                alt: attribute(tag, "alt").unwrap_or_default(),
                tag: true,
            });
        }
    }
    found
}

/// Where the tag starting at `start` ends, past its `>`, quotes respected.
fn tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (i, c) in html[start..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(start + i + 1),
            _ => {}
        }
    }
    None
}

/// The value of attribute `name` in `tag`, its entities decoded.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let lower = tag.to_ascii_lowercase();
    let mut from = 4;
    while let Some(at) = lower[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        let before = bytes[start - 1];
        if !before.is_ascii_whitespace() && before != b'/' && before != b'"' && before != b'\'' {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest
                .split(|c: char| c.is_ascii_whitespace() || c == '>')
                .next()
                .unwrap_or("")
                .trim_end_matches('/'),
        };
        return Some(entities(value));
    }
    None
}

fn entities(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// An HTML page without its `<head>`, whose title the text view would show
/// as a line of the page.
pub fn without_head(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let start = lower.match_indices("<head").map(|(at, _)| at).find(|&at| {
        matches!(
            lower.as_bytes().get(at + 5),
            Some(b'>' | b' ' | b'\n' | b'\t' | b'\r')
        )
    });
    match (start, lower.find("</head>")) {
        (Some(start), Some(end)) if end > start => {
            format!("{}{}", &html[..start], &html[end + "</head>".len()..])
        }
        _ => html.to_owned(),
    }
}

/// Whether `url` names something away from this machine, which silent never
/// fetches.
pub fn remote(url: &str) -> bool {
    let url = url.trim();
    url.starts_with("//")
        || url.split_once(':').is_some_and(|(scheme, _)| {
            scheme.len() > 1
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                && !scheme.eq_ignore_ascii_case("file")
                && !scheme.eq_ignore_ascii_case("data")
        })
}

/// The file a local `url` in a document in `dir` names: relative to the
/// document, or a `file://` URL, its query and fragment dropped.
pub fn local_path(url: &str, dir: &Path) -> Option<PathBuf> {
    let url = url.trim();
    if remote(url) || url.starts_with("data:") || url.is_empty() {
        return None;
    }
    let path = url.strip_prefix("file://").unwrap_or(url);
    let path = path.split(['?', '#']).next()?;
    let path = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    let path = Path::new(path.as_ref());
    Some(if path.is_absolute() {
        path.to_owned()
    } else {
        dir.join(path)
    })
}

/// `text` with every picture `shown` does not keep put as its alternative
/// text, so that the text view draws the words where the picture would be.
pub fn rewrite(text: &str, pictures: &[Picture], shown: impl Fn(&Picture) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for picture in pictures {
        if shown(picture) || picture.span.start < at || picture.span.end > text.len() {
            continue;
        }
        out.push_str(&text[at..picture.span.start]);
        if picture.tag {
            out.push_str(&escape_html(&picture.alt));
        } else {
            out.push_str(&escape_markdown(&picture.alt));
        }
        at = picture.span.end;
    }
    out.push_str(&text[at..]);
    out
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_pictures_are_found_where_they_are_written() {
        let text = "# T\n\n[![Build](https://ci.example/b.svg)](https://ci.example)\n\
                    ![logo](docs/logo.png \"Logo\")\n\n![ref][r]\n\n[r]: shots/a.png\n\n\
                    <p align=\"center\"><img src=\"https://x.example/l.png\" alt=\"Big &amp; bold\" width=200></p>\n\n\
                    ```\n![not](a/picture.png)\n```\n";
        let found = pictures(text, Markup::Markdown);
        let urls: Vec<&str> = found.iter().map(|p| p.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://ci.example/b.svg",
                "docs/logo.png",
                "shots/a.png",
                "https://x.example/l.png"
            ]
        );
        assert_eq!(found[3].alt, "Big & bold");
        assert!(found[3].tag && !found[0].tag);
        assert_eq!(
            &text[found[1].span.clone()],
            "![logo](docs/logo.png \"Logo\")"
        );
    }

    #[test]
    fn pictures_away_become_their_words() {
        let text = "[![Build Status](https://ci.example/b.svg)](https://ci.example) and \
                    <img alt='Q &amp; A' src=\"//cdn.example/x.png\"> and ![here](here.png)";
        let found = pictures(text, Markup::Markdown);
        let rewritten = rewrite(text, &found, |p| !remote(&p.url));
        assert_eq!(
            rewritten,
            "[Build Status](https://ci.example) and Q &amp; A and ![here](here.png)"
        );
        let found = pictures("x ![a*b](https://e.example/p.png)", Markup::Markdown);
        assert_eq!(
            rewrite("x ![a*b](https://e.example/p.png)", &found, |_| false),
            "x a\\*b"
        );
    }

    #[test]
    fn html_pictures_are_found_in_their_tags() {
        let html =
            "<IMG SRC=\"a.png\" ALT=\"one\"><imgx src=b><img\nsrc='c d.png' alt=two/><img alt=x>";
        let found = pictures(html, Markup::Html);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].url.as_str(), found[0].alt.as_str()),
            ("a.png", "one")
        );
        assert_eq!(
            (found[1].url.as_str(), found[1].alt.as_str()),
            ("c d.png", "two")
        );
        assert_eq!(
            &html[found[1].span.clone()],
            "<img\nsrc='c d.png' alt=two/>"
        );
    }

    #[test]
    fn only_this_machines_files_are_local() {
        let dir = Path::new("/r/docs");
        assert!(remote("https://x.example/a.png") && remote("//cdn/a.png") && remote("mailto:x"));
        assert!(!remote("a.png") && !remote("C:/x") && !remote("file:///a.png"));
        assert_eq!(
            local_path("img/a%20b.png?raw=1#x", dir),
            Some("/r/docs/img/a b.png".into())
        );
        assert_eq!(
            local_path("file:///abs/c.png", dir),
            Some("/abs/c.png".into())
        );
        assert_eq!(local_path("https://x/a.png", dir), None);
        assert_eq!(local_path("data:image/png;base64,AAAA", dir), None);
    }

    #[test]
    fn a_page_loses_its_head_and_keeps_its_body() {
        let page =
            "<html><HEAD><title>T</title></head><body><header>H</header><p>x</p></body></html>";
        assert_eq!(
            without_head(page),
            "<html><body><header>H</header><p>x</p></body></html>"
        );
        assert_eq!(without_head("<p>no head</p>"), "<p>no head</p>");
    }

    #[test]
    fn the_file_name_says_the_markup() {
        assert_eq!(Markup::of(Path::new("README.md")), Some(Markup::Markdown));
        assert_eq!(Markup::of(Path::new("a/Page.HTM")), Some(Markup::Html));
        assert_eq!(Markup::of(Path::new("notes.txt")), None);
    }
}
