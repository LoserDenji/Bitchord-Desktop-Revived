//! Release notes: a renderer for GitHub release bodies — headings by level, paragraphs,
//! bullets and numbered lists, rules, blockquote callouts, **bold**, *italic*,
//! ~~strike~~, `code`, [links](…), bare URLs, ![images](…) and `[![badges](…)](…)`,
//! plus the inline HTML releases lean on (`<img>` with widths, `align="center"`
//! containers, `<br>`), scripts and unsafe URLs kept out — the dialog shown when an
//! update is ready (or to read a version's notes), and the card shown after updating.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Margin, Sense, Stroke, Ui, vec2};

use super::{App, widgets};
use crate::theme::{self, icon};

/// Where the lightbox's image lives between frames: `ctx` data, keyed once.
fn lightbox_id() -> egui::Id {
    egui::Id::unique("notes-lightbox")
}

// ---- the model ----------------------------------------------------------------

/// A width an `<img>` asked for: a share of the space, or pixels.
#[derive(Clone, PartialEq, Debug)]
pub enum Width {
    Pct(f32),
    Px(f32),
}

fn width_of(s: &str) -> Option<Width> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%').filter(|p| !p.is_empty()) {
        return p.parse::<f32>().ok().map(|p| Width::Pct((p / 100.0).clamp(0.01, 1.0)));
    }
    let px = s.strip_suffix("px").unwrap_or(s);
    px.parse::<f32>().ok().filter(|p| *p > 0.0).map(Width::Px)
}

/// An image in the notes: markdown `![alt](url)`, a badge, or an `<img>` tag. `link`
/// is the target a badge opens; `width` the `<img>` asked for.
#[derive(Clone, PartialEq, Debug)]
pub struct Img {
    pub url: String,
    pub alt: String,
    pub width: Option<Width>,
    pub link: Option<String>,
}

impl Img {
    fn md(alt: &str, url: &str) -> Self {
        Self { url: url.to_owned(), alt: unescape(alt), width: None, link: None }
    }
}

/// The formatting one stretch of text carries.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

/// A stretch of inline content: text, a link, or an image (a badge is an image with a
/// `link`). Bare URLs are links too, their punctuation left out of them.
#[derive(Clone, PartialEq, Debug)]
pub enum Span {
    Text(String, Style),
    Link { text: String, url: String, style: Style },
    Image(Img),
}

#[derive(Debug)]
pub enum Block {
    Heading { level: u8, text: String },
    Para(String),
    /// `number` for an item of a numbered list; `None` is a dash bullet.
    Bullet { text: String, number: Option<u32> },
    /// A blockquote: the lines under `>`, parsed as blocks (so a quote can hold
    /// headings, bullets, links), drawn as a callout.
    Quote(Vec<Block>),
    Rule,
    Image(Img),
    /// An `align="center"` container's content, parsed as blocks and centered.
    Center(Vec<Block>),
}

// ---- parsing ------------------------------------------------------------------

/// The release body as blocks. Scripts go first (dropped whole), `<br>`s become real
/// line breaks, then the lines are split into blocks — with blockquotes and centered
/// containers parsed recursively, so what they hold is formatted too.
pub fn parse(md: &str) -> Vec<Block> {
    parse_lines(&brs_to_newlines(&drop_scripts(md)))
}

fn parse_lines(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para = String::new();
    let flush = |para: &mut String, out: &mut Vec<Block>| {
        if !para.trim().is_empty() {
            out.push(Block::Para(para.trim().to_owned()));
        }
        para.clear();
    };
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.is_empty() {
            flush(&mut para, &mut out);
        } else if let Some((level, text)) = heading(t) {
            flush(&mut para, &mut out);
            out.push(Block::Heading { level, text: text.to_owned() });
        } else if is_rule(t) {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
        } else if t.starts_with('>') {
            flush(&mut para, &mut out);
            // Every consecutive quoted line, one marker stripped so a `> >` nests.
            let mut quoted = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if !t.starts_with('>') {
                    break;
                }
                quoted.push(t.strip_prefix('>').map(|l| l.strip_prefix(' ').unwrap_or(l)).unwrap_or_default().to_owned());
                i += 1;
            }
            out.push(Block::Quote(parse_lines(&quoted.join("\n"))));
            continue;
        } else if let Some(close) = center_open(t) {
            flush(&mut para, &mut out);
            // The container's own line may carry content after the tag; the line that
            // closes it may carry some before the closing tag.
            let mut inner = vec![strip_tags(t)];
            i += 1;
            while i < lines.len() {
                let line = lines[i].trim();
                if let Some(tag) = tag_of(line)
                    && tag.close
                    && tag.name == close
                {
                    inner.push(strip_tags(line));
                    i += 1;
                    break;
                }
                inner.push(line.to_owned());
                i += 1;
            }
            // An unclosed container ends at the end of the body, rendered all the same.
            out.push(Block::Center(parse_lines(&inner.join("\n"))));
            continue;
        } else if let Some((img, rest)) = image_at(t)
            && rest.trim().is_empty()
        {
            flush(&mut para, &mut out);
            out.push(Block::Image(img));
        } else if let Some((tag, rest)) = leading_tag(t)
            && rest.trim().is_empty()
            && matches!(tag.name.as_str(), "img" | "hr")
        {
            flush(&mut para, &mut out);
            match tag.name.as_str() {
                "img" => {
                    if let Some(img) = img_from(&tag) {
                        out.push(Block::Image(img));
                    }
                }
                _ => out.push(Block::Rule),
            }
        } else if let Some((b, n)) = bullet(t) {
            flush(&mut para, &mut out);
            out.push(Block::Bullet { text: b.trim().to_owned(), number: n });
        } else if t.starts_with('<') {
            // HTML the block layer has no look of its own for (a `<summary>`, a stray
            // `</div>`): the line flows as a paragraph, its tags read by the inline
            // pass, so their text and pictures stay and the markup doesn't.
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(t);
        } else if let (true, Some(Block::Bullet { text: b, .. })) = (para.is_empty(), out.last_mut()) {
            // A wrapped bullet continues on the next line.
            b.push(' ');
            b.push_str(t);
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(t);
        }
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

/// `#` through `######`, with the text after it.
fn heading(t: &str) -> Option<(u8, &str)> {
    let marks = t.chars().take_while(|c| *c == '#').count().min(6);
    (marks > 0 && t.chars().nth(marks).is_some_and(char::is_whitespace))
        .then(|| (marks as u8, t[marks..].trim()))
}

/// A horizontal rule: three or more of one dash-ish character, spaces allowed between.
fn is_rule(t: &str) -> bool {
    let dense: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    dense.len() >= 3 && dense.chars().all(|c| matches!(c, '-' | '*' | '_'))
}

/// A list item: `- `, `* `, `+ `, or `1. ` / `1) ` for the numbered ones.
fn bullet(t: &str) -> Option<(&str, Option<u32>)> {
    if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
        return Some((b, None));
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    (digits > 0 && digits < 10 && matches!(t.chars().nth(digits), Some('.' | ')') if t.chars().nth(digits + 1).is_some_and(char::is_whitespace)))
        .then(|| (t[digits + 2..].trim(), Some(t[..digits].parse().ok()).flatten()))
}

/// An image the whole line is: markdown `![alt](url)` or `<img …>`. `None` when the
/// line isn't one, or points somewhere this app won't fetch.
fn image_at(t: &str) -> Option<(Img, &str)> {
    if let Some(rest) = t.strip_prefix("![") {
        let close = rest.find("](")?;
        let end = rest[close..].find(')')?;
        let url = &rest[close + 2..close + end];
        if !is_http(url) {
            return None;
        }
        return Some((Img::md(&rest[..close], url), &rest[close + end + 1..]));
    }
    if !t.starts_with('<') {
        return None;
    }
    let (tag, rest) = tag(t)?;
    if tag.name != "img" {
        return None;
    }
    let img = img_from(&tag)?;
    Some((img, rest))
}

/// The `align="center"` containers (`<div>`/`<p>`): the tag name whose closing tag
/// ends one, so the container's lines can be gathered.
fn center_open(t: &str) -> Option<&'static str> {
    if !t.starts_with('<') {
        return None;
    }
    let (tag, _) = tag(t)?;
    if tag.close {
        return None;
    }
    let centered = tag.attrs.iter().any(|(k, v)| k == "align" && v.eq_ignore_ascii_case("center"));
    let name = match tag.name.as_str() {
        "div" => "div",
        "p" => "p",
        _ => return None,
    };
    centered.then_some(name)
}

fn tag_of(t: &str) -> Option<Tag> {
    if !t.starts_with('<') {
        return None;
    }
    let (tag, _) = tag(t)?;
    Some(tag)
}

/// A `<tag>` at the start of `t`, and the rest of the line after it.
fn leading_tag(t: &str) -> Option<(Tag, &str)> {
    if !t.starts_with('<') {
        return None;
    }
    tag(t)
}

/// An `<img>` tag's picture, or `None` when it points nowhere this app may fetch.
fn img_from(tag: &Tag) -> Option<Img> {
    let attr = |k: &str| tag.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let url = attr("src")?;
    if !is_http(url) {
        return None;
    }
    Some(Img {
        url: url.to_owned(),
        alt: attr("alt").map_or_else(String::new, unescape),
        width: attr("width").and_then(width_of),
        link: attr("href").and_then(|h| safe_url(h)),
    })
}

/// A parsed HTML tag: name, whether it closes, and its attributes.
struct Tag {
    name: String,
    close: bool,
    attrs: Vec<(String, String)>,
}

/// The tag at the start of `s` and what follows it. `None` when the text isn't a tag
/// (a lone `<`, an unterminated one), so it stays literal text and nothing crashes.
fn tag(s: &str) -> Option<(Tag, &str)> {
    let mut rest = s.strip_prefix('<')?;
    let close = rest.starts_with('/');
    rest = rest.strip_prefix('/').unwrap_or(rest);
    let name_len = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').count();
    if name_len == 0 || !rest.is_char_boundary(name_len) {
        return None;
    }
    let name = rest[..name_len].to_ascii_lowercase();
    rest = &rest[name_len..];
    let mut attrs = Vec::new();
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix('>') {
            return Some((Tag { name, close, attrs }, r));
        }
        if rest.is_empty() {
            return None; // never closed: not a tag, plain text
        }
        // One attribute: a name, and a value when `=` follows.
        let key_len = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').count();
        if key_len == 0 {
            // Not attribute syntax (a stray `?`, say): past this character, and on.
            let skip = rest.char_indices().nth(1).map_or(rest.len(), |(i, _)| i);
            rest = &rest[skip..];
            continue;
        }
        let key = rest[..key_len].to_ascii_lowercase();
        rest = &rest[key_len..];
        let mut value = String::new();
        if let Some(r) = rest.strip_prefix('=') {
            rest = r;
            let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'');
            if let Some(q) = quote {
                rest = &rest[1..];
                if let Some(end) = rest.find(q) {
                    value = rest[..end].to_owned();
                    rest = &rest[end + 1..];
                }
            } else {
                let len = rest.chars().take_while(|c| !c.is_whitespace() && *c != '>').count();
                value = rest[..len].to_owned();
                rest = &rest[len..];
            }
        }
        attrs.push((key, value));
    }
}

/// Everything `<script>`/`<style>` covers, tags and content both: dropped whole, so
/// no script text can render and no handler can run.
fn drop_scripts(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(at) = find_ci(rest, "<script").or_else(|| find_ci(rest, "<style")) {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        // The whole element: its open tag, its content, its close tag. A close that
        // never comes takes the rest of the body, so nothing of it leaks back in.
        let open_end = tag(after).map(|(_, r)| after.len() - r.len()).unwrap_or(after.len());
        let content_end = find_ci(&after[open_end..], "</script>").or_else(|| find_ci(&after[open_end..], "</style")).map_or(after.len(), |e| open_end + e);
        let close_end = if content_end < after.len() { content_end + 9 } else { content_end };
        let skip = close_end.max(open_end).min(after.len());
        rest = &after[skip..];
    }
    out.push_str(rest);
    out
}

fn find_ci(s: &str, needle: &str) -> Option<usize> {
    let hay = s.to_ascii_lowercase();
    hay.find(&needle.to_ascii_lowercase())
}

/// `<br>`, `<br/>`, `<br />` and case variants: a paragraph break each — the model
/// has no hard in-paragraph line, and a break between paragraphs is the closest a
/// card can come to one.
fn brs_to_newlines(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(at) = find_ci(rest, "<br") {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        match tag(after) {
            Some((t, r)) if t.name == "br" => {
                out.push_str("\n\n");
                rest = r;
            }
            _ => {
                // Not a br after all: keep it as text, past the next `>` if any.
                let skip = after.find('>').map_or(after.len(), |e| e + 1);
                out.push_str(&after[..skip]);
                rest = &after[skip..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The tag markup out of a stretch of mixed HTML and text, entities decoded: what a
/// container's leftover line, or an unknown tag's content, renders as.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('<') {
        match tag(&rest[at..]) {
            Some((_, r)) => {
                out.push_str(&rest[..at]);
                rest = r;
            }
            None => {
                out.push('<');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    unescape(&out)
}

/// The handful of entities release bodies actually use.
fn unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ")
}

fn is_http(u: &str) -> bool {
    u.starts_with("https://") || u.starts_with("http://")
}

/// Where a link may go: plain web addresses (and `mailto:`) only — `javascript:` and
/// the other schemes the app has no handler for stay unopened, their label as text.
fn safe_url(u: &str) -> Option<String> {
    let u = u.trim();
    if is_http(u) || u.starts_with("mailto:") {
        return Some(u.to_owned());
    }
    None
}

// ---- inline ------------------------------------------------------------------

/// A line's (or paragraph's) content: text with formatting, links, images.
fn spans(s: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut text = String::new();
    let flush = |out: &mut Vec<Span>, text: &mut String, style: Style| {
        if !text.is_empty() {
            out.push(Span::Text(std::mem::take(text), style));
        }
    };
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('\\')
            && r.chars().next().is_some_and(|c| "*_~[]()>#!".contains(c))
        {
            text.push(r.chars().next().unwrap());
            rest = &r[1..];
        } else if rest.starts_with('<') {
            match tag(rest) {
                Some((t, r)) => {
                    rest = r;
                    // `on` is the style after the tag: an opening tag turns it on, a
                    // closing one off.
                    match (t.name.as_str(), !t.close) {
                        ("b" | "strong", on) => {
                            flush(&mut out, &mut text, style);
                            style.bold = on;
                        }
                        ("i" | "em", on) => {
                            flush(&mut out, &mut text, style);
                            style.italic = on;
                        }
                        ("s" | "del" | "strike", on) => {
                            flush(&mut out, &mut text, style);
                            style.strike = on;
                        }
                        ("code", on) => {
                            flush(&mut out, &mut text, style);
                            style.code = on;
                        }
                        ("br", _) => text.push(' '),
                        ("a", true) => {
                            // The anchor's label until its closing tag; a link that
                            // would go nowhere renders as its label.
                            let href = t.attrs.iter().find(|(k, _)| k == "href").and_then(|(_, v)| safe_url(v));
                            let end = find_ci(rest, "</a>").unwrap_or(rest.len());
                            let label = strip_tags(&rest[..end]);
                            flush(&mut out, &mut text, style);
                            match href {
                                Some(url) => out.push(Span::Link { text: label, url, style }),
                                None => text.push_str(&label),
                            }
                            rest = rest.get(end..).and_then(|r| r.strip_prefix("</a>")).unwrap_or("");
                        }
                        ("img", _) => {
                            if let Some(img) = img_from(&t) {
                                flush(&mut out, &mut text, style);
                                out.push(Span::Image(img));
                            }
                        }
                        _ => {} // tags with no look of their own: markup dropped, content stays
                    }
                }
                None => {
                    text.push('<');
                    rest = &rest[1..];
                }
            }
        } else if let Some(r) = rest.strip_prefix("**").or_else(|| rest.strip_prefix("__")) {
            // Toggle the marker: the text before it ends with the style it had.
            flush(&mut out, &mut text, style);
            style.bold = !style.bold;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("~~") {
            flush(&mut out, &mut text, style);
            style.strike = !style.strike;
            rest = r;
        } else if rest.starts_with('*') && rest[1..].chars().next().is_some_and(|c| !c.is_whitespace()) {
            flush(&mut out, &mut text, style);
            style.italic = !style.italic;
            rest = &rest[1..];
        } else if rest.starts_with('_') && underscore(&text, &rest[1..], style.italic) {
            flush(&mut out, &mut text, style);
            style.italic = !style.italic;
            rest = &rest[1..];
        } else if let Some(r) = rest.strip_prefix('`') {
            let end = r.find('`').unwrap_or(r.len());
            flush(&mut out, &mut text, style);
            out.push(Span::Text(r[..end].to_owned(), Style { code: true, ..style }));
            rest = r.get(end + 1..).unwrap_or("");
        } else if rest.starts_with("[![") {
            // A badge: an image that opens its target.
            if let Some((mut img, r2)) = image_at(&rest[1..])
                && let Some((target, r3)) = link_tail(r2)
            {
                img.link = safe_url(&target);
                flush(&mut out, &mut text, style);
                out.push(Span::Image(img));
                rest = r3;
            } else {
                text.push('[');
                rest = &rest[1..];
            }
        } else if let Some(r) = rest.strip_prefix('[')
            && let Some((label, url, r2)) = link_at(r)
        {
            flush(&mut out, &mut text, style);
            match safe_url(&url) {
                // A link that would go nowhere is its label, kept apart from the text
                // around it.
                Some(url) => out.push(Span::Link { text: unescape(label), url, style }),
                None => out.push(Span::Text(unescape(label), style)),
            }
            rest = r2;
        } else if rest.starts_with('!') && rest[1..].starts_with('[') {
            if let Some((img, r)) = image_at(rest) {
                flush(&mut out, &mut text, style);
                out.push(Span::Image(img));
                rest = r;
            } else {
                text.push('!');
                rest = &rest[1..];
            }
        } else {
            let next = rest.char_indices().skip(1).find(|(_, c)| matches!(c, '*' | '`' | '[' | '<' | '!' | '\\' | '_' | '~')).map_or(rest.len(), |(i, _)| i);
            let run = &rest[..next];
            match bare_link(run) {
                // Text up to the URL, the URL as a link, then the rest goes round
                // again (its punctuation first, glued back on in `words`).
                Some((off, url, after)) => {
                    if off > 0 {
                        text.push_str(&unescape(&run[..off]));
                    }
                    flush(&mut out, &mut text, style);
                    out.push(Span::Link { text: url.to_owned(), url: url.to_owned(), style });
                    rest = after;
                }
                None => {
                    text.push_str(&unescape(run));
                    rest = &rest[next..];
                }
            }
        }
    }
    flush(&mut out, &mut text, style);
    out
}

/// Whether a `_` can open or close emphasis where it sits: opening after a word
/// edge into a letter; closing out of text, into anything but a letter (`*` doesn't
/// need this — it pairs anywhere, so `5 * 3` stays plain because its `*` isn't
/// followed by anything wordy).
fn underscore(before: &str, after: &str, italic: bool) -> bool {
    let word_before = before.chars().next_back().is_some_and(|c| !c.is_whitespace());
    let next = after.chars().next();
    if italic {
        word_before && next.is_none_or(|c| !c.is_alphanumeric())
    } else {
        !word_before && next.is_some_and(char::is_alphanumeric)
    }
}

/// `[label](url)`, with `rest` starting after the `[`.
fn link_at(r: &str) -> Option<(&str, &str, &str)> {
    let close = r.find("](")?;
    let url = &r[close + 2..];
    // The URL ends at the `)` that balances what it opened — a plain first `)`
    // would cut one short that carries parens of its own (a wiki link).
    let end = close_paren(url)? + close + 2;
    Some((&r[..close], &r[close + 2..end], &r[end + 1..]))
}

/// Where the `)` that closes a `(`-balanced stretch is.
fn close_paren(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// A badge's `](target)` tail, with `rest` starting after the image.
fn link_tail(r: &str) -> Option<(String, &str)> {
    let close = r.find(']')?;
    let rest = r[close + 1..].strip_prefix('(')?;
    let end = rest.find(')')?;
    Some((rest[..end].to_owned(), &rest[end + 1..]))
}

/// The first bare URL in `s`, starting a word (after a space, a bracket or the like):
/// where it starts, the URL without the punctuation that trails it, and the rest from
/// there. `None` when there's no URL before the line's next markdown.
fn bare_link(s: &str) -> Option<(usize, &str, &str)> {
    let mut best: Option<usize> = None;
    for prefix in ["https://", "http://"] {
        let mut from = 0;
        while let Some(found) = s[from..].find(prefix) {
            let at = from + found;
            let boundary = s[..at].chars().next_back().is_none_or(|c| c.is_whitespace() || "(*_~".contains(c));
            if boundary {
                best = Some(best.map_or(at, |b: usize| b.min(at)));
                break;
            }
            from = at + 1;
        }
    }
    let at = best?;
    let end = s[at..].find(char::is_whitespace).map_or(s.len(), |e| at + e);
    let url = s[at..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '\'', '"']);
    Some((at, url, &s[at + url.len()..]))
}

// ---- the layout words ----------------------------------------------------------

/// One word of a paragraph: the pieces it's made of (bold beside plain, a link's
/// words, a bare URL's trailing period glued back on), or an image flowing with them
/// (a badge). Between words there was a space; between the pieces of one there wasn't.
#[derive(Clone, PartialEq, Debug)]
enum Word {
    Text(Vec<(String, Style, Option<String>)>),
    Image(Img),
}

fn words(s: &str) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    let mut word: Vec<(String, Style, Option<String>)> = Vec::new();
    // Whether what comes next starts a word of its own: a space just went by, or
    // nothing has been written yet.
    let mut gap = true;
    let mut buf = String::new();
    let flush = |buf: &mut String, style: Style, gap: &mut bool, word: &mut Vec<(String, Style, Option<String>)>, out: &mut Vec<Word>| {
        if buf.is_empty() {
            return;
        }
        if !*gap
            && word.is_empty()
            && let Some(Word::Text(last)) = out.last_mut()
        {
            // No space since the last word: this piece is part of it — a bare URL's
            // ".", text a link ran into.
            last.push((std::mem::take(buf), style, None));
        } else {
            if *gap && !word.is_empty() {
                out.push(Word::Text(std::mem::take(word)));
            }
            word.push((std::mem::take(buf), style, None));
        }
        *gap = false;
    };
    for span in spans(s) {
        match span {
            Span::Image(img) => {
                flush(&mut buf, Style::default(), &mut gap, &mut word, &mut out);
                if !word.is_empty() {
                    out.push(Word::Text(std::mem::take(&mut word)));
                }
                out.push(Word::Image(img));
                gap = false;
            }
            Span::Link { text, url, style } => {
                flush(&mut buf, style, &mut gap, &mut word, &mut out);
                // A link's own text is words of its own, each opening the same place.
                for w in text.split_whitespace() {
                    if !word.is_empty() {
                        out.push(Word::Text(std::mem::take(&mut word)));
                    }
                    out.push(Word::Text(vec![(w.to_owned(), style, Some(url.clone()))]));
                }
                gap = false;
            }
            Span::Text(t, style) => {
                for c in t.chars() {
                    if c.is_whitespace() {
                        flush(&mut buf, style, &mut gap, &mut word, &mut out);
                        if !word.is_empty() {
                            out.push(Word::Text(std::mem::take(&mut word)));
                        }
                        gap = true;
                    } else {
                        buf.push(c);
                    }
                }
                flush(&mut buf, style, &mut gap, &mut word, &mut out);
            }
        }
    }
    if !word.is_empty() {
        out.push(Word::Text(word));
    }
    out
}

/// The text of a stretch of inline content, formatting dropped (images fall away, an
/// image's alt isn't its text): the card's short lines, plain headings.
fn plain(s: &str) -> String {
    spans(s).into_iter().map(|sp| match sp {
        Span::Text(t, _) | Span::Link { text: t, .. } => t,
        Span::Image(_) => String::new(),
    }).collect()
}

/// Whether a stretch of inline content needs the word-by-word layout that makes each
/// link its own widget and each image its own picture.
fn linked(s: &str) -> bool {
    spans(s).iter().any(|sp| !matches!(sp, Span::Text(..)))
}

// ---- rendering -----------------------------------------------------------------

/// The notes laid out like the release page: headings by level, paragraphs, bullets,
/// rules, callout quotes, images and badges, centered where asked.
pub fn show(ui: &mut Ui, blocks: &[Block], size: f32) {
    let width = ui.available_width();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            ui.add_space(6.0);
        }
        show_block(ui, b, size, body_color(), width, false);
    }
    lightbox(ui.ctx());
}

fn show_block(ui: &mut Ui, b: &Block, size: f32, color: Color32, width: f32, center: bool) {
    match b {
        Block::Heading { level, text } => {
            let (font, dy) = heading_font(*level, size);
            if *level < 3 {
                ui.add_space(4.0);
            }
            if linked(text) {
                rich(ui, text, font.size, theme::text(), width, Marker::None, center);
            } else {
                let j = job(text, font.size, theme::text(), width);
                let g = ui.fonts_mut(|f| f.layout_job(j));
                paint_line(ui, g, width, center, theme::text());
            }
            ui.add_space(dy);
        }
        Block::Para(t) => {
            if linked(t) {
                rich(ui, t, size, color, width, Marker::None, center);
            } else {
                let j = job(t, size, color, width);
                let g = ui.fonts_mut(|f| f.layout_job(j));
                paint_line(ui, g, width, center, color);
            }
            ui.add_space(6.0);
        }
        Block::Bullet { text, number } => {
            let marker = match number {
                Some(n) => Marker::Number(*n),
                None => Marker::Dot,
            };
            if linked(text) {
                rich(ui, text, size, color, width, marker, center);
            } else {
                let j = job(text, size, color, width - 18.0);
                let g = ui.fonts_mut(|f| f.layout_job(j));
                let (rect, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
                paint_marker(ui, rect, size, color, marker);
                ui.painter().galley(rect.min + vec2(18.0, 0.0), g, color);
            }
            ui.add_space(4.0);
        }
        Block::Quote(inner) => {
            ui.add_space(4.0);
            let top = ui.cursor().min;
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.set_width(width - 14.0);
                    for (i, b) in inner.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(5.0);
                        }
                        show_block(ui, b, size - 0.5, theme::text_dim(), width - 14.0, false);
                    }
                });
            });
            // The callout's bar, drawn the height the content turned out.
            let bottom = ui.cursor().min.y - ui.spacing().item_spacing.y;
            ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(top.x + 2.0, top.y), egui::pos2(top.x + 5.0, bottom.max(top.y + 8.0))), 1.5, quote_bar());
            ui.add_space(4.0);
        }
        Block::Rule => {
            ui.add_space(8.0);
            hline(ui);
            ui.add_space(8.0);
        }
        Block::Image(img) => {
            image(ui, img, width, center);
            ui.add_space(6.0);
        }
        Block::Center(inner) => {
            let mut i = 0;
            while i < inner.len() {
                if let Block::Image(img) = &inner[i] {
                    // Consecutive pictures share one centered row — the usual
                    // side-by-side pair — wrapping onto lines when the window is narrow.
                    let mut row = vec![img.clone()];
                    let mut j = i + 1;
                    while let Some(Block::Image(img)) = inner.get(j) {
                        row.push(img.clone());
                        j += 1;
                    }
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.set_max_width(width);
                            for img in &row {
                                image(ui, img, width, false);
                            }
                        });
                    });
                    ui.add_space(6.0);
                    i = j;
                } else {
                    show_block(ui, &inner[i], size, color, width, true);
                    i += 1;
                }
            }
        }
    }
}

fn heading_font(level: u8, size: f32) -> (egui::FontId, f32) {
    match level {
        1 => (theme::bold(size + 5.0), 10.0),
        2 => (theme::bold(size + 3.0), 8.0),
        3 => (theme::semibold(size + 1.5), 6.0),
        4 => (theme::semibold(size + 1.0), 5.0),
        5 => (theme::semibold(size), 4.0),
        _ => (theme::semibold(size - 0.5), 3.0),
    }
}

/// A laid-out paragraph: left-aligned by default, centered inside an `align=center`.
fn paint_line(ui: &mut Ui, g: std::sync::Arc<egui::Galley>, width: f32, center: bool, color: Color32) {
    let h = g.size().y;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let left = if center { rect.left() + (width - g.size().x).max(0.0) / 2.0 } else { rect.left() };
    ui.painter().galley(egui::pos2(left, rect.top()), g, color);
}

enum Marker {
    None,
    Dot,
    Number(u32),
}

fn paint_marker(ui: &mut Ui, rect: egui::Rect, size: f32, color: Color32, marker: Marker) {
    match marker {
        Marker::None => {}
        Marker::Dot => {
            ui.painter().circle_filled(egui::pos2(rect.left() + 6.0, rect.top() + size * 0.75), 2.2, color);
        }
        Marker::Number(n) => {
            ui.painter().text(egui::pos2(rect.left() + 18.0, rect.top() + size * 0.6), egui::Align2::RIGHT_CENTER, format!("{n}."), theme::regular(size * 0.92), color);
        }
    }
}

/// A paragraph, bullet or heading that carries links or images, laid out word by word
/// so each link is its own widget and each image its own picture: the same fonts,
/// colors and line height as the galley-built lines, with a real space's width between
/// the words.
fn rich(ui: &mut Ui, text: &str, size: f32, color: Color32, width: f32, marker: Marker, center: bool) {
    let top = ui.cursor().min;
    let space = ui.painter().layout_no_wrap(" ".to_owned(), theme::regular(size), color).size().x;
    let indent = if matches!(marker, Marker::None) { 0.0 } else { 18.0 };
    ui.horizontal(|ui| {
        if center {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(width - indent);
                    ui.spacing_mut().item_spacing = vec2(space, 0.0);
                    rich_words(ui, text, size, color, width - indent);
                });
            });
        } else {
            ui.add_space(indent);
            ui.vertical(|ui| {
                ui.set_width(width - indent);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(space, 0.0);
                    rich_words(ui, text, size, color, width - indent);
                });
            });
        }
    });
    paint_marker(ui, egui::Rect::from_min_size(top, vec2(18.0, size)), size, color, marker);
}

fn rich_words(ui: &mut Ui, text: &str, size: f32, color: Color32, width: f32) {
    for word in words(text) {
        match word {
            Word::Image(img) => {
                image(ui, &img, width, false);
            }
            Word::Text(parts) => {
                // One word: its pieces sit tight against each other, whatever mixes
                // inside it (a bare URL and its trailing period).
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (t, style, url) in parts {
                        let shown = hard_wrap(ui, &t, size, color, width);
                        let mut j = LayoutJob::default();
                        j.append(&shown, 0.0, piece_format(style, size, color));
                        match url {
                            Some(url) => {
                                ui.hyperlink_to(j, url);
                            }
                            None => {
                                ui.label(j);
                            }
                        }
                    }
                });
            }
        }
    }
}

/// A word too wide for the dialog (a long URL, a hash): broken anywhere rather than
/// pushed out of it — the breaks land wherever the width runs out, like GitHub's.
fn hard_wrap(ui: &Ui, t: &str, size: f32, color: Color32, width: f32) -> String {
    if width < 24.0 || t.chars().count() < 12 {
        return t.to_owned();
    }
    let g = ui.painter().layout_no_wrap(t.to_owned(), theme::regular(size), color);
    if g.size().x <= width {
        return t.to_owned();
    }
    let chars: Vec<char> = t.chars().collect();
    let per = ((chars.len() as f32) * (width / g.size().x)).floor().max(6.0) as usize;
    let mut out = String::with_capacity(t.len() + chars.len() / per + 1);
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && i % per == 0 {
            out.push('\n');
        }
        out.push(*c);
    }
    out
}

/// One piece of a word as a job: the fonts, colors, and code/bold/italic/strike looks.
fn piece_format(style: Style, size: f32, color: Color32) -> TextFormat {
    let font = if style.code {
        egui::FontId::monospace(size - 1.0)
    } else if style.bold {
        theme::semibold(size)
    } else {
        theme::regular(size)
    };
    TextFormat {
        font_id: font,
        color: if style.bold { theme::text() } else { color },
        line_height: Some(size * 1.5),
        italics: style.italic,
        strikethrough: if style.strike { Stroke::new(1.0, color) } else { Stroke::NONE },
        background: if style.code { code_bg() } else { Color32::TRANSPARENT },
        expand_bg: if style.code { 2.0 } else { 0.0 },
        ..Default::default()
    }
}

/// A paragraph (or heading) with no links or images, as one wrapped job.
fn job(text: &str, size: f32, color: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width.max(10.0);
    // Long URLs and hashes break mid-token rather than push the dialog wide.
    job.wrap.break_anywhere = true;
    for span in spans(text) {
        if let Span::Text(t, style) | Span::Link { text: t, style, .. } = span {
            job.append(&t, 0.0, piece_format(style, size, color));
        }
    }
    job
}

/// An image: a badge flowing with the text when it has no width of its own, otherwise
/// a picture sized to what it asked for within the space it has. Loading shows a
/// spinner in the box it will take, a picture that won't load shows its alt text, and
/// neither moves anything else around. A linked one opens its page; a plain one
/// opens the lightbox.
fn image(ui: &mut Ui, img: &Img, width: f32, center: bool) -> egui::Response {
    let w = match img.width {
        Some(Width::Pct(p)) => (p * width).min(width),
        Some(Width::Px(px)) => px.min(width),
        None => if img.link.is_some() { width.min(160.0) } else { width.min(360.0) },
    };
    // The box it will fill: a picture is capped wide, a badge short, so what loads
    // lands close to the space already saved for it.
    let h = if img.link.is_some() && img.width.is_none() { 22.0 } else { w * 9.0 / 16.0 };
    let picture = egui::Image::from_uri(img.url.clone())
        .max_size(vec2(w, h))
        .show_loading_spinner(true)
        .alt_text(if img.alt.is_empty() { "image" } else { img.alt.as_str() })
        .sense(Sense::click());
    let resp = if center {
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| ui.add(picture)).inner
    } else {
        ui.add(picture)
    };
    if resp.clicked() {
        if let Some(url) = &img.link {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url.clone()));
        } else {
            ui.ctx().data_mut(|d| d.insert_temp(lightbox_id(), img.url.clone()));
        }
    }
    resp
}

/// The click-to-view: a picture clicked without a link of its own opens over the
/// dialog, as big as it goes, and any click (or Esc) puts it away.
fn lightbox(ctx: &egui::Context) {
    let Some(url): Option<String> = ctx.data(|d| d.get_temp(lightbox_id())) else { return };
    let mut close = false;
    let modal = egui::Modal::new(lightbox_id())
        .backdrop_color(theme::shade(0.85))
        .show(ctx, |ui| {
            let max = ctx.content_rect().size() * 0.85;
            let picture = egui::Image::from_uri(url.clone()).max_size(max).show_loading_spinner(true).sense(Sense::click());
            close |= ui.add(picture).clicked();
        });
    if close || modal.should_close() {
        ctx.data_mut(|d| d.remove_temp::<String>(lightbox_id()));
    }
}

fn code_bg() -> Color32 {
    if theme::light() { Color32::from_gray(234) } else { Color32::from_gray(40) }
}

fn quote_bar() -> Color32 {
    if theme::light() { Color32::from_gray(190) } else { Color32::from_gray(70) }
}

/// The card's short list: feature headings and bullets (first sentence), up to
/// three, plus how many more there are. A "Fixes" heading isn't an item itself.
pub fn summary(blocks: &[Block]) -> (Vec<String>, usize) {
    let mut items = Vec::new();
    for b in blocks {
        match b {
            Block::Heading { text, .. } if !text.to_lowercase().starts_with("fix") => items.push(plain(text)),
            Block::Bullet { text, .. } => {
                let t = plain(text);
                let first = t.find(". ").map_or(t.as_str(), |i| &t[..i]);
                items.push(first.trim_end_matches('.').to_owned());
            }
            _ => {}
        }
    }
    let more = items.len().saturating_sub(3);
    items.truncate(3);
    (items, more)
}

/// The notes' paragraphs and bullets.
fn body_color() -> Color32 {
    if theme::light() { theme::text() } else { Color32::from_gray(225) }
}

/// Lines between parts of the card.
fn rule_color() -> Color32 {
    if theme::light() { Color32::from_gray(222) } else { Color32::from_gray(44) }
}

pub(super) fn surface() -> egui::Frame {
    egui::Frame::new()
        .fill(if theme::light() { theme::surface_fill() } else { Color32::from_gray(28) })
        .stroke(Stroke::new(1.0, rule_color()))
        .corner_radius(16)
        .shadow(egui::epaint::Shadow { offset: [0, 24], blur: 70, spread: 0, color: theme::shade(0.55) })
}

fn hline(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, rule_color());
}

pub fn github_link(ui: &mut Ui) {
    ui.hyperlink_to(egui::RichText::new("Full release notes on GitHub").font(theme::regular(13.0)).color(if theme::light() { Color32::from_rgb(0, 102, 204) } else { Color32::from_rgb(62, 166, 255) }), crate::update::RELEASES);
}

/// What the notes dialog shows: a downloaded update (with Restart), or a version's notes.
#[derive(Clone)]
pub struct NotesDialog {
    pub version: String,
    pub ready: bool,
}

pub fn dialog(app: &mut App, ctx: &egui::Context) {
    let Some(d) = app.notes_dialog.clone() else { return };
    let body = app.updater.as_ref().and_then(|u| u.notes(&d.version));
    // For a packaged copy, which can't install it itself: the new package to download,
    // except in a Flatpak, which Flatpak updates.
    let available = app.updater.as_ref().and_then(|u| match u.state() {
        crate::update::State::Available { version, url } if d.ready && version == d.version => Some(url),
        _ => None,
    });
    let deb_url = available.clone().filter(|_| !crate::update::flatpak());
    // Only while that version is still the one downloaded: a later check moves the
    // state on (an Available one to Checking), and Restart would then install nothing.
    let ready = app.updater.as_ref().is_some_and(|u| matches!(u.state(), crate::update::State::Ready { version, .. } if d.ready && version == d.version));
    let (mut close, mut restart) = (false, false);
    let accent = app.accent_color();
    let modal = egui::Modal::new(egui::Id::unique("release-notes")).frame(surface()).backdrop_color(theme::shade(0.55)).show(ctx, |ui| {
        let w = 540.0_f32.min(ctx.content_rect().width() - 48.0);
        ui.set_width(w);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        egui::Frame::new().inner_margin(Margin { left: 28, right: 28, top: 24, bottom: 18 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::icon_at(ui, ui.cursor().min + vec2(8.0, 8.0), icon::SPARKLES, 15.0, accent);
                ui.add_space(22.0);
                let label = if available.is_some() { "UPDATE AVAILABLE" } else if ready { "UPDATE READY" } else { "WHAT'S NEW" };
                let mut j = LayoutJob::default();
                j.append(label, 0.0, TextFormat { font_id: theme::semibold(12.0), color: theme::text_dim(), extra_letter_spacing: 1.0, ..Default::default() });
                ui.label(j);
            });
            ui.add_space(8.0);
            ui.label(egui::RichText::new(format!("BitChord {}", d.version)).font(theme::bold(26.0)).color(theme::text()));
            ui.add_space(3.0);
            ui.label(egui::RichText::new("by Loserdenji").font(theme::semibold(13.0)).color(accent));
            ui.add_space(3.0);
            let sub = if available.is_some() {
                format!("You're on {}. {}", crate::update::current_version(), crate::update::package_step())
            } else if ready {
                format!("You're on {}. It's downloaded and installs when you close the app.", crate::update::current_version())
            } else if d.version == crate::update::current_version() {
                "The version you're running.".to_owned()
            } else {
                // An update's dialog whose state a later check has moved on.
                format!("BitChord {} is out.", d.version)
            };
            ui.label(egui::RichText::new(sub).font(theme::regular(13.5)).color(theme::text_dim()));
        });
        hline(ui);
        let max_h = (ctx.content_rect().height() - 330.0).clamp(160.0, 460.0);
        egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 28, right: 28, top: 18, bottom: 18 }).show(ui, |ui| {
                match &body {
                    Some(md) => show(ui, &parse(md), 14.0),
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Loading release notes…").color(theme::text_dim()));
                        });
                    }
                }
                ui.add_space(8.0);
                github_link(ui);
            });
        });
        hline(ui);
        egui::Frame::new().inner_margin(Margin { left: 28, right: 28, top: 16, bottom: 16 }).show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if let Some(url) = &deb_url {
                    if widgets::pill(ui, "Download", None, true).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        close = true;
                    }
                    close |= widgets::pill(ui, "Later", None, false).clicked();
                } else if available.is_some() {
                    close = widgets::pill(ui, "Close", None, true).clicked();
                } else if ready {
                    restart = widgets::pill(ui, "Restart to update", None, true).clicked();
                    close |= widgets::pill(ui, "Later", None, false).clicked();
                } else {
                    close = widgets::pill(ui, "Close", None, true).clicked();
                }
                // Where the news lands first, beside whatever else the bar offers.
                if widgets::pill(ui, "Join the Discord server", Some(icon::USERS), false).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(crate::discord::DISCORD_INVITE));
                }
            });
        });
    });
    if close || modal.should_close() {
        app.notes_dialog = None;
    }
    if restart {
        app.notes_dialog = None;
        app.restart_to_update(ctx);
    }
}

/// "Updated to X" in the corner on the first launch of a new version.
pub fn card(app: &mut App, ctx: &egui::Context) {
    let Some(version) = app.whats_new.clone() else { return };
    let Some(md) = app.updater.as_ref().and_then(|u| u.notes(&version)) else { return };
    let (items, more) = summary(&parse(&md));
    if items.is_empty() {
        app.whats_new = None;
        return;
    }
    let screen = ctx.content_rect();
    let at = egui::pos2(screen.right() - 24.0, screen.bottom() - app.bar_height() - 20.0);
    let t = theme::anim_bool(ctx, "whats-new-in", true, 0.35);
    let (mut dismiss, mut all) = (false, false);
    egui::Area::new(egui::Id::unique("whats-new"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(at + vec2(0.0, 16.0 * (1.0 - t)))
        .show(ctx, |ui| {
            ui.multiply_opacity(t);
            surface().corner_radius(14).inner_margin(Margin { left: 18, right: 12, top: 14, bottom: 16 }).show(ui, |ui| {
                ui.set_width(330.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(format!("Updated to {version}")).font(theme::semibold(15.0)).color(theme::text()));
                        ui.add_space(3.0);
                        ui.label(egui::RichText::new("Here's what changed.").font(theme::regular(13.0)).color(theme::text_dim()));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        dismiss = widgets::icon_button(ui, icon::X, 16.0, "Dismiss", false).clicked();
                    });
                });
                ui.add_space(10.0);
                let mut lines: Vec<String> = items.clone();
                if more > 0 {
                    lines.push(if more == 1 { "+ 1 more change".to_owned() } else { format!("+ {more} more changes") });
                }
                for (i, l) in lines.iter().enumerate() {
                    let faint = more > 0 && i == lines.len() - 1;
                    let color = if faint { theme::text_dim() } else { body_color() };
                    let mut j = LayoutJob::single_section(l.clone(), TextFormat { font_id: theme::regular(13.5), color, ..Default::default() });
                    j.wrap.max_width = 300.0;
                    j.wrap.max_rows = 1;
                    let g = ui.fonts_mut(|f| f.layout_job(j));
                    let (rect, _) = ui.allocate_exact_size(vec2(318.0, 22.0), Sense::hover());
                    if !faint {
                        ui.painter().circle_filled(egui::pos2(rect.left() + 5.0, rect.center().y), 2.2, color);
                    }
                    ui.painter().galley(egui::pos2(rect.left() + 16.0, rect.center().y - g.size().y / 2.0), g, color);
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    all = widgets::pill(ui, "See all notes", None, true).clicked();
                    dismiss |= widgets::pill(ui, "Got it", None, false).clicked();
                });
            });
        });
    if all {
        app.notes_dialog = Some(NotesDialog { version: version.clone(), ready: false });
    }
    if all || dismiss {
        app.whats_new = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "## Uploads\n\nPick **files** or drop them.\n\n## Fixes\n\n- Mixes play again. They broke in 0.2.9.\n- Icons are `centred`\n  in their buttons.\n- See [the notes](https://example.com).\n";

    #[test]
    fn parses_release_markdown() {
        let b = parse(MD);
        assert!(matches!(&b[0], Block::Heading { level: 2, text } if text == "Uploads"));
        assert!(matches!(&b[1], Block::Para(t) if t == "Pick **files** or drop them."));
        assert!(matches!(&b[4], Block::Bullet { text, number: None } if text == "Icons are `centred` in their buttons."));
        assert_eq!(spans("Pick **files** now"), vec![
            Span::Text("Pick ".into(), Style::default()),
            Span::Text("files".into(), Style { bold: true, ..Default::default() }),
            Span::Text(" now".into(), Style::default()),
        ]);
        assert_eq!(plain("See [the notes](https://example.com)."), "See the notes.");
    }

    #[test]
    fn headings_carry_their_level_and_rules_divide() {
        let b = parse("# Big\n\n### Small\n\n---\n\nbody\n\n***\n\nmore\n\n___\n\nend");
        assert!(matches!(&b[0], Block::Heading { level: 1, text } if text == "Big"));
        assert!(matches!(&b[1], Block::Heading { level: 3, text } if text == "Small"));
        assert!(matches!(b[2], Block::Rule));
        assert!(matches!(&b[3], Block::Para(t) if t == "body"));
        assert!(matches!(b[4], Block::Rule));
        assert!(matches!(&b[5], Block::Para(t) if t == "more"));
        assert!(matches!(b[6], Block::Rule));
        // Bullets keep their dashes; a rule isn't one.
        assert!(is_rule("---") && is_rule("****") && is_rule("- - -"));
        assert!(!is_rule("- a list"));
    }

    #[test]
    fn quotes_hold_formatted_content() {
        let b = parse("> **Heads up:** the addon list moved.\n> Second line.\n\nafter");
        assert!(matches!(&b[1], Block::Para(t) if t == "after"));
        let Block::Quote(inner) = &b[0] else { panic!("quote") };
        assert_eq!(inner.len(), 1);
        assert!(matches!(&inner[0], Block::Para(t) if t == "**Heads up:** the addon list moved. Second line."));
        // A quote inside a quote nests.
        let nested = parse("> > deep");
        assert!(matches!(&nested[0], Block::Quote(q) if matches!(&q[0], Block::Quote(q2) if matches!(&q2[0], Block::Para(t) if t == "deep"))));
    }

    #[test]
    fn inline_formatting_and_code() {
        assert_eq!(
            spans("*careful*, __strong__, ~~gone~~, `code`"),
            vec![
                Span::Text("careful".into(), Style { italic: true, ..Default::default() }),
                Span::Text(", ".into(), Style::default()),
                Span::Text("strong".into(), Style { bold: true, ..Default::default() }),
                Span::Text(", ".into(), Style::default()),
                Span::Text("gone".into(), Style { strike: true, ..Default::default() }),
                Span::Text(", ".into(), Style::default()),
                Span::Text("code".into(), Style { code: true, ..Default::default() }),
            ]
        );
        // A marker alone stays a marker; snake_case words aren't italic.
        assert!(spans("snake_case_file").iter().all(|s| matches!(s, Span::Text(_, st) if !st.italic)));
        assert_eq!(plain("a `Ctrl+K` shortcut"), "a Ctrl+K shortcut");
        // Inline HTML formatting applies too.
        let s = spans("<b>bold</b> and <em>it</em> and <del>gone</del>");
        assert!(s.iter().any(|sp| matches!(sp, Span::Text(t, st) if t == "bold" && st.bold)));
        assert!(s.iter().any(|sp| matches!(sp, Span::Text(t, st) if t == "it" && st.italic)));
        assert!(s.iter().any(|sp| matches!(sp, Span::Text(t, st) if t == "gone" && st.strike)));
    }

    #[test]
    fn bare_urls_are_links_without_their_punctuation() {
        assert_eq!(spans("Read https://example.com/a?b=1 now"), vec![
            Span::Text("Read ".into(), Style::default()),
            Span::Link { text: "https://example.com/a?b=1".into(), url: "https://example.com/a?b=1".into(), style: Style::default() },
            Span::Text(" now".into(), Style::default()),
        ]);
        // The period after a URL stays plain text, glued onto it as one word.
        assert_eq!(spans("See https://example.com."), vec![
            Span::Text("See ".into(), Style::default()),
            Span::Link { text: "https://example.com".into(), url: "https://example.com".into(), style: Style::default() },
            Span::Text(".".into(), Style::default()),
        ]);
        assert_eq!(words("See https://example.com."), vec![
            Word::Text(vec![("See".into(), Style::default(), None)]),
            Word::Text(vec![("https://example.com".into(), Style::default(), Some("https://example.com".into())), (".".into(), Style::default(), None)]),
        ]);
        // An https inside a word isn't a link.
        assert!(spans("nothttps://example.com").iter().all(|s| matches!(s, Span::Text(_, _))));
    }

    #[test]
    fn markdown_links_span_words() {
        assert_eq!(words("See [the notes](https://example.com) here"), vec![
            Word::Text(vec![("See".into(), Style::default(), None)]),
            Word::Text(vec![("the".into(), Style::default(), Some("https://example.com".into()))]),
            Word::Text(vec![("notes".into(), Style::default(), Some("https://example.com".into()))]),
            Word::Text(vec![("here".into(), Style::default(), None)]),
        ]);
        assert!(linked("See [the notes](https://example.com) here"));
        assert!(!linked("Plain **bold** and `code` only"));
        assert!(linked("A badge [![build](https://img.shields.io/badge/x-1-green)](https://example.com)"));
    }

    #[test]
    fn images_badges_and_html_img_tags() {
        let b = parse("![screenshot](https://example.com/shot.png)");
        assert!(matches!(&b[0], Block::Image(img) if img.url == "https://example.com/shot.png" && img.alt == "screenshot"));
        // An <img> with a percentage width, alone on its line.
        let b = parse(r#"<img src="https://example.com/a.png" alt="Side by side" width="48%">"#);
        assert!(matches!(&b[0], Block::Image(img) if img.width == Some(Width::Pct(0.48)) && img.alt == "Side by side"));
        // Pixels are pixels, capped where they ask more than there is.
        let b = parse(r#"<img src="https://example.com/b.png" width="600">"#);
        assert!(matches!(&b[0], Block::Image(img) if img.width == Some(Width::Px(600.0))));
        // A badge in a line is an image that opens its target.
        let s = spans("[![build](https://img.shields.io/badge/x-1-green)](https://example.com) passed");
        assert!(matches!(&s[0], Span::Image(img) if img.url == "https://img.shields.io/badge/x-1-green" && img.link.as_deref() == Some("https://example.com")));
        assert!(matches!(&s[1], Span::Text(t, _) if t == " passed"));
        // An <img> mid-line stays inline.
        let s = spans(r#"before <img src="https://example.com/b.png" width="100%"> after"#);
        assert!(matches!(&s[0], Span::Text(t, _) if t == "before "));
        assert!(matches!(&s[1], Span::Image(img) if img.width == Some(Width::Pct(1.0))));
        assert!(matches!(&s[2], Span::Text(t, _) if t == " after"));
    }

    #[test]
    fn centered_containers_parse_their_contents() {
        let b = parse("<div align=\"center\">\n\n![hero](https://example.com/hero.png)\n\n**Now with Atmos.**\n\n</div>\n\nafter");
        assert!(matches!(&b[1], Block::Para(t) if t == "after"));
        let Block::Center(inner) = &b[0] else { panic!("center") };
        assert!(matches!(&inner[0], Block::Image(img) if img.url == "https://example.com/hero.png"));
        assert!(matches!(&inner[1], Block::Para(t) if t == "**Now with Atmos.**"));
        // <p align="center"> too, and an unclosed one ends at the body's end.
        let b = parse("<p align=\"center\">centered text\n");
        assert!(matches!(&b[0], Block::Center(inner) if matches!(&inner[0], Block::Para(t) if t == "centered text")));
        // An align="left" container isn't centered: its line just flows.
        let b = parse("<div align=\"left\">plain</div>");
        assert!(matches!(&b[0], Block::Para(t) if plain(t) == "plain"));
    }

    #[test]
    fn html_line_breaks_and_unknown_tags() {
        // <br> is a line break, so two paragraphs rather than one merged.
        let b = parse("one<br>two");
        assert_eq!(b.len(), 2);
        // Unknown tags drop their markup, keep their text; entities decode.
        let b = parse("<details>\n<summary>New stuff</summary>\n\n- first &amp; second\n\n</details>");
        let text: Vec<String> = b.iter().filter_map(|b| match b {
            Block::Para(t) | Block::Bullet { text: t, .. } => Some(plain(t)),
            _ => None,
        }).collect();
        assert!(text.iter().any(|t| t.trim() == "New stuff"));
        assert!(text.iter().any(|t| t == "first & second"));
        // A bare < with no tag stays text.
        assert_eq!(plain("a < b"), "a < b");
        // A malformed tag never crashes parsing.
        assert_eq!(plain("x <unclosed"), "x <unclosed");
    }

    #[test]
    fn scripts_and_unsafe_urls_go() {
        // A script's whole span — tag, handlers, content — is dropped.
        assert!(!drop_scripts("before <script>alert('x')</script> after").contains("alert"));
        let b = parse("before\n\n<script>alert('x')</script>\n\nafter");
        assert_eq!(b.len(), 2);
        // javascript: links render as their label only.
        let s = spans("[click](javascript:alert(1))");
        assert!(matches!(&s[0], Span::Text(t, _) if t == "click"));
        // data: images never load; their line just stays text.
        let b = parse("![evil](data:text/html;base64,x)");
        assert!(matches!(&b[0], Block::Para(t) if t.contains("data:")));
        // <a href> is a link when it's safe.
        let s = spans(r#"<a href="https://example.com/a">the page</a> and on"#);
        assert!(matches!(&s[0], Span::Link { text, url, .. } if text == "the page" && url == "https://example.com/a"));
        // An onload handler can't ride along: attrs the renderer keeps are src/alt/width only.
        let s = spans(r#"<img src="https://example.com/c.png" onerror="alert(1)" alt="fine">"#);
        assert!(matches!(&s[0], Span::Image(img) if img.url == "https://example.com/c.png" && img.alt == "fine"));
    }

    #[test]
    fn numbered_lists_and_wrapped_bullets() {
        let b = parse("1. first\n2. second\n3. third");
        for (i, n) in b.iter().zip([Some(1), Some(2), Some(3)]) {
            assert!(matches!(i, Block::Bullet { number: m, .. } if *m == n), "{i:?}");
        }
        let b = parse("- wrapped\n  across lines");
        assert!(matches!(&b[0], Block::Bullet { text, number: None } if text == "wrapped across lines"));
    }

    #[test]
    fn summary_takes_features_then_first_sentences() {
        let (items, more) = summary(&parse(MD));
        assert_eq!(items, vec!["Uploads", "Mixes play again", "Icons are centred in their buttons"]);
        assert_eq!(more, 1);
    }
}
