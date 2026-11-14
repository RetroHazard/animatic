//! Review bodies (imported from AOM) are AniDB's stored text: BBCode style tags (`[b]`, `[i]`,
//! `[u]`, `[s]`, `[url]`), HTML line breaks (`<br />`) and HTML character references. They are
//! shown as styled text; anything not recognised stays literal, so no text is ever dropped.

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

/// Splits `body` into styled runs.
fn parse(body: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut style = Depth::default();
    let mut rest = body;
    while let Some(c) = rest.chars().next() {
        let consumed = match c {
            '[' => tag(rest).map(|(len, kind, open)| {
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
                len
            }),
            '<' => line_break(rest).inspect(|_| text.push('\n')),
            '&' => entity(rest).map(|(len, ch)| {
                text.push(ch);
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

/// A recognised tag at the start of `s`: its byte length, kind and whether it opens.
fn tag(s: &str) -> Option<(usize, Kind, bool)> {
    let end = s.bytes().take(MAX_TAG).position(|b| b == b']')?;
    let inner = s[1..end].trim();
    let (open, name) = match inner.strip_prefix('/') {
        Some(name) => (false, name.trim()),
        None => (true, inner),
    };
    let lower = name.to_ascii_lowercase();
    let kind = match lower.as_str() {
        "b" => Kind::Bold,
        "i" => Kind::Italic,
        "u" => Kind::Underline,
        "s" => Kind::Strike,
        "url" => Kind::Link,
        _ if open && lower.starts_with("url=") => Kind::Link,
        _ => return None,
    };
    Some((end + 1, kind, open))
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
    fn unknown_or_malformed_markup_stays_literal() {
        for body in [
            "[spoiler]x[/spoiler]",
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
