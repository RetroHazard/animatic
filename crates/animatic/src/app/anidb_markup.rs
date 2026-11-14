//! AniDB's stored text (anime descriptions and review bodies, from the HTTP API or an AOM
//! import): BBCode style tags (`[b]`, `[i]`, `[u]`, `[s]`, `[url]`, `[quote]`, `[spoiler]`,
//! `[img]`), rules and lists (`[hr]`, `[ul]`/`[ol]` with `[li]` or `[*]`), the HTTP API's link
//! form `http://anidb.net/ch123 [Name]`, HTML line breaks (`<br />`), HTML character references
//! and the backtick AniDB stores for an apostrophe. They are shown as styled text; images are
//! shown as their address and never downloaded; anything not recognised stays literal, so no
//! text is ever dropped.

use eframe::egui::{self, Color32, Stroke, text::LayoutJob};

/// Styling of a run of text. Each field counts open tags, so nesting and repeats balance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Depth {
    bold: u8,
    italic: u8,
    underline: u8,
    strike: u8,
    link: u8,
}

/// A run of text and the style it is shown with.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Span {
    text: String,
    style: Depth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bold,
    Italic,
    Underline,
    Strike,
    Link,
}

impl Depth {
    fn counter(&mut self, kind: Kind) -> &mut u8 {
        match kind {
            Kind::Bold => &mut self.bold,
            Kind::Italic => &mut self.italic,
            Kind::Underline => &mut self.underline,
            Kind::Strike => &mut self.strike,
            Kind::Link => &mut self.link,
        }
    }
}

/// Longest tag and character reference looked at; longer brackets are plain text, which also
/// keeps parsing linear in the body's length.
const MAX_TAG: usize = 512;
const MAX_ENTITY: usize = 12;

/// What a recognised `[tag]` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    /// `[b]`, `[/b]` and the like: start or end a style.
    Style(Kind, bool),
    /// A style whose opening tag is announced by a label in the surrounding style: `[img]`
    /// (the address is shown as a link, never fetched) and `[spoiler]`.
    Labelled(Kind, bool, &'static str),
    /// `[hr]`: a horizontal rule on its own line.
    Rule,
    /// `[ul]`/`[ol]` (true = ordered) and their closing tags (`List(_, false)`).
    List(bool, bool),
    /// `[li]`/`[*]`: a list item; `[/li]` ends it.
    Item(bool),
}

/// Characters of a horizontal rule.
const RULE: &str = "────────────────────────────────";

/// Deepest list nesting that is indented further; deeper items keep this indentation, so
/// pathological nesting cannot make the output grow quadratically.
const MAX_INDENT: usize = 4;

/// Whether the next text starts a line.
fn at_line_start(spans: &[Span], text: &str) -> bool {
    match text.chars().last() {
        Some(c) => c == '\n',
        None => spans.last().is_none_or(|s| s.text.ends_with('\n')),
    }
}

/// Splits `body` into styled runs.
fn parse(body: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut style = Depth::default();
    // Open lists, innermost last: `Some(next number)` for ordered ones.
    let mut lists: Vec<Option<u32>> = Vec::new();
    let mut rest = body;
    while let Some(c) = rest.chars().next() {
        let consumed = match c {
            '[' => tag(rest).map(|(len, tag)| {
                match tag {
                    Tag::Style(kind, open) | Tag::Labelled(kind, open, _) => {
                        if let Tag::Labelled(_, true, label) = tag {
                            text.push_str(label);
                        }
                        if !text.is_empty() {
                            spans.push(Span {
                                text: std::mem::take(&mut text),
                                style,
                            });
                        }
                        let counter = style.counter(kind);
                        *counter = if open {
                            counter.saturating_add(1)
                        } else {
                            counter.saturating_sub(1)
                        };
                    }
                    Tag::Rule => {
                        if !at_line_start(&spans, &text) {
                            text.push('\n');
                        }
                        text.push_str(RULE);
                        text.push('\n');
                    }
                    Tag::List(ordered, open) => {
                        if open {
                            lists.push(ordered.then_some(1));
                        } else {
                            lists.pop();
                        }
                        if !at_line_start(&spans, &text) {
                            text.push('\n');
                        }
                    }
                    Tag::Item(true) => {
                        if !at_line_start(&spans, &text) {
                            text.push('\n');
                        }
                        for _ in 1..lists.len().min(MAX_INDENT) {
                            text.push_str("    ");
                        }
                        match lists.last_mut() {
                            Some(Some(n)) => {
                                text.push_str(&format!("{n}. "));
                                *n += 1;
                            }
                            _ => text.push_str("• "),
                        }
                    }
                    Tag::Item(false) => {}
                }
                len
            }),
            '<' => line_break(rest).inspect(|_| text.push('\n')),
            '&' => entity(rest).map(|(len, ch)| {
                text.push(ch);
                len
            }),
            '`' => {
                text.push('\'');
                Some(1)
            }
            'h' => bare_link(rest).map(|(len, label)| {
                if !text.is_empty() {
                    spans.push(Span {
                        text: std::mem::take(&mut text),
                        style,
                    });
                }
                let mut linked = style;
                linked.link = linked.link.saturating_add(1);
                spans.push(Span {
                    text: label.to_owned(),
                    style: linked,
                });
                len
            }),
            '\r' => Some(1),
            _ => None,
        };
        match consumed {
            Some(len) => rest = &rest[len..],
            None => {
                text.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !text.is_empty() {
        spans.push(Span { text, style });
    }
    spans
}

/// The HTTP API's link form at the start of `s`: a URL, one space and its label in brackets
/// (`http://anidb.net/ch123 [Name]`); its byte length and the label.
fn bare_link(s: &str) -> Option<(usize, &str)> {
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        return None;
    }
    let url_end = s
        .bytes()
        .take(MAX_TAG)
        .position(|b| b.is_ascii_whitespace() || b == b'[')?;
    let rest = s[url_end..].strip_prefix(" [")?;
    let label_len = rest.bytes().take(MAX_TAG).position(|b| b == b']')?;
    let label = &rest[..label_len];
    (!label.trim().is_empty() && !label.contains('['))
        .then_some((url_end + 2 + label_len + 1, label))
}

/// A recognised tag at the start of `s`: its byte length and what it does.
fn tag(s: &str) -> Option<(usize, Tag)> {
    let end = s.bytes().take(MAX_TAG).position(|b| b == b']')?;
    let inner = s[1..end].trim();
    let (open, name) = match inner.strip_prefix('/') {
        Some(name) => (false, name.trim()),
        None => (true, inner),
    };
    let lower = name.to_ascii_lowercase();
    let style = |kind| Some(Tag::Style(kind, open));
    let tag = match lower.as_str() {
        "b" => style(Kind::Bold),
        "i" => style(Kind::Italic),
        "u" => style(Kind::Underline),
        "s" => style(Kind::Strike),
        "url" => style(Kind::Link),
        _ if open && lower.starts_with("url=") => style(Kind::Link),
        "quote" => style(Kind::Italic),
        "img" => Some(Tag::Labelled(Kind::Link, open, "Image: ")),
        "spoiler" => Some(Tag::Labelled(Kind::Italic, open, "Spoiler: ")),
        "hr" if open => Some(Tag::Rule),
        "ul" => Some(Tag::List(false, open)),
        "ol" => Some(Tag::List(true, open)),
        "li" => Some(Tag::Item(open)),
        "*" if open => Some(Tag::Item(true)),
        _ => None,
    }?;
    Some((end + 1, tag))
}

/// Length of an HTML line break (`<br>`, `<br/>`, `<br />`, any case) at the start of `s`.
fn line_break(s: &str) -> Option<usize> {
    let end = s.bytes().take(8).position(|b| b == b'>')?;
    let inner: String = s[1..end].chars().filter(|c| !c.is_whitespace()).collect();
    matches!(inner.to_ascii_lowercase().as_str(), "br" | "br/").then_some(end + 1)
}

/// A character reference (`&amp;`, `&#39;`, `&#x2019;`, ...) at the start of `s`: its byte
/// length and character. Control characters are not produced.
fn entity(s: &str) -> Option<(usize, char)> {
    let end = s.bytes().take(MAX_ENTITY).position(|b| b == b';')?;
    let name = &s[1..end];
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ => {
            let code = name.strip_prefix('#')?;
            let value = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(value).filter(|c| !c.is_control())?
        }
    };
    Some((end + 1, ch))
}

/// `body` laid out for a label in `ui`: bold and links in the strong and hyperlink colours,
/// italics, underline and strike-through as such. Wrapping is left to the label.
pub(super) fn layout(ui: &egui::Ui, body: &str) -> LayoutJob {
    let visuals = ui.visuals();
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let mut job = LayoutJob::default();
    for span in parse(body) {
        let s = span.style;
        let color = if s.link > 0 {
            visuals.hyperlink_color
        } else if s.bold > 0 {
            visuals.strong_text_color()
        } else {
            visuals.text_color()
        };
        let line = |on: u8| {
            if on > 0 {
                Stroke::new(1.0, color)
            } else {
                Stroke::NONE
            }
        };
        job.append(
            &span.text,
            0.0,
            egui::TextFormat {
                font_id: font_id.clone(),
                color,
                background: Color32::TRANSPARENT,
                italics: s.italic > 0,
                underline: line(s.underline.max(s.link)),
                strikethrough: line(s.strike),
                ..Default::default()
            },
        );
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(body: &str) -> String {
        parse(body).into_iter().map(|s| s.text).collect()
    }

    fn styled(text: &str, f: impl FnOnce(&mut Depth)) -> Span {
        let mut style = Depth::default();
        f(&mut style);
        Span {
            text: text.into(),
            style,
        }
    }

    #[test]
    fn tags_style_text_and_disappear() {
        assert_eq!(
            parse("[b] NOTICE: [/b]a [u][B]both[/b][/U]"),
            vec![
                styled(" NOTICE: ", |s| s.bold = 1),
                styled("a ", |_| {}),
                styled("both", |s| {
                    s.underline = 1;
                    s.bold = 1;
                }),
            ]
        );
        assert_eq!(
            parse("[i]x[/i][s]y[/s][url=https://example.invalid/]z[/url]"),
            vec![
                styled("x", |s| s.italic = 1),
                styled("y", |s| s.strike = 1),
                styled("z", |s| s.link = 1),
            ]
        );
    }

    #[test]
    fn line_breaks_and_references_become_characters() {
        assert_eq!(
            plain("one.<br /><br />two<BR>three<br/>&amp; &lt;b&gt; it&#39;s &#x2019;\r\n"),
            "one.\n\ntwo\nthree\n& <b> it's \u{2019}\n"
        );
    }

    #[test]
    fn descriptions_show_api_links_and_backtick_apostrophes() {
        assert_eq!(
            parse(
                "By http://anidb.net/cr991 [Tanaka Yoshiki]`s novels; [url=http://anidb.net/ch1]Yang[/url]."
            ),
            vec![
                styled("By ", |_| {}),
                styled("Tanaka Yoshiki", |s| s.link = 1),
                styled("'s novels; ", |_| {}),
                styled("Yang", |s| s.link = 1),
                styled(".", |_| {}),
            ]
        );
        // A URL without the bracketed label, or a label that never closes, stays as written.
        for body in [
            "see http://anidb.net/a1 for more",
            "http://x [no end",
            "http://x [] y",
        ] {
            assert_eq!(plain(body), body);
        }
    }

    #[test]
    fn rules_lists_and_images_become_lines() {
        assert_eq!(
            plain(
                "end.[hr][u]Art[/u]\nbody[ol][li]one[/li][li]two[/li][/ol]after[ul][*]a[*]b[/ul]"
            ),
            format!("end.\n{RULE}\nArt\nbody\n1. one\n2. two\nafter\n• a\n• b\n")
        );
        assert_eq!(
            plain("[ol]Heading[/ol]text"),
            "Heading\ntext",
            "a list without items only breaks the line"
        );
        assert_eq!(
            parse("see [img]http://example.invalid/a.jpg[/img] and [spoiler]x[/spoiler]"),
            vec![
                styled("see Image: ", |_| {}),
                styled("http://example.invalid/a.jpg", |s| s.link = 1),
                styled(" and Spoiler: ", |_| {}),
                styled("x", |s| s.italic = 1),
            ]
        );
    }

    #[test]
    fn deep_list_nesting_keeps_output_linear() {
        let depth = 10_000;
        let body = format!("{}{}", "[ol]".repeat(depth), "[*]x".repeat(depth));
        let text = plain(&body);
        // Each item is a line with at most MAX_INDENT - 1 indents and its number.
        assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), depth);
        assert!(
            text.len() < depth * 32,
            "output grew to {} bytes",
            text.len()
        );
        // Every item belongs to the innermost list: indented at the cap and numbered in order.
        let indent = "    ".repeat(MAX_INDENT - 1);
        assert!(text.starts_with(&format!("{indent}1. x\n{indent}2. x\n")));
    }

    #[test]
    fn unknown_or_malformed_markup_stays_literal() {
        for body in [
            "[blink]x[/blink]",
            "a [b unclosed",
            "x [/url=y] z",
            "<b>bold</b> <br",
            "&bogus; &#0; &#xZZ; & alone",
            "[]",
        ] {
            assert_eq!(plain(body), body);
        }
    }

    #[test]
    fn unbalanced_closing_tags_do_not_underflow() {
        assert_eq!(
            parse("[/b][/b]x[b]y"),
            vec![styled("x", |_| {}), styled("y", |s| s.bold = 1)]
        );
    }
}
