//! A small reader for project descriptions from Modrinth: Markdown with bits of
//! HTML mixed in. It does not aim to be complete, only to show the text
//! readably: headings, paragraphs, lists, quotes, code, tables, links and pictures.

/// A piece of text with one look.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    /// List item: nesting depth, marker ("•" or "1."), text.
    Item(u8, String, Vec<Span>),
    Quote(Vec<Span>),
    Code(String),
    Rule,
    /// Table row: the cells.
    Row(Vec<Vec<Span>>),
    /// Picture (optionally a link).
    Image { url: String, alt: String, link: Option<String> },
}

/// Read a description into blocks.
pub fn parse(src: &str) -> Vec<Block> {
    let text = resolve_references(&html_to_markdown(&src.replace("\r\n", "\n")));
    let mut blocks = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut code: Option<String> = None;

    let flush = |para: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if para.is_empty() {
            return;
        }
        let joined = para.join(" ");
        para.clear();
        push_inline(blocks, &joined, Block::Paragraph);
    };

    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(buf) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                blocks.push(Block::Code(code.take().unwrap_or_default().trim_end_matches('\n').to_string()));
            } else {
                buf.push_str(line);
                buf.push('\n');
            }
            continue;
        }
        let t = line.trim_start();
        if t.starts_with("```") {
            flush(&mut para, &mut blocks);
            code = Some(String::new());
            continue;
        }
        if t.is_empty() {
            flush(&mut para, &mut blocks);
            continue;
        }
        if is_rule(t) {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some((level, rest)) = heading(t) {
            flush(&mut para, &mut blocks);
            push_inline(&mut blocks, rest, |s| Block::Heading(level, s));
            continue;
        }
        if let Some(rest) = t.strip_prefix('>') {
            flush(&mut para, &mut blocks);
            push_inline(&mut blocks, rest.trim_start_matches('>').trim(), Block::Quote);
            continue;
        }
        if t.starts_with('|') {
            flush(&mut para, &mut blocks);
            let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
            let separator = cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
            if !separator {
                blocks.push(Block::Row(cells.iter().map(|c| inline(c).0).collect()));
            }
            continue;
        }
        if let Some((marker, rest)) = list_item(t) {
            flush(&mut para, &mut blocks);
            let indent = line.len() - t.len();
            let depth = (indent / 2).min(4) as u8;
            push_inline(&mut blocks, rest, |s| Block::Item(depth, marker.clone(), s));
            continue;
        }
        para.push(t.to_string());
    }
    if let Some(c) = code {
        blocks.push(Block::Code(c.trim_end_matches('\n').to_string()));
    }
    flush(&mut para, &mut blocks);
    blocks
}

/// `[text][id]` with `[id]: url` somewhere in the text becomes `[text](url)`;
/// the definition lines themselves are dropped.
fn resolve_references(text: &str) -> String {
    let mut refs: Vec<(String, String)> = Vec::new();
    let mut body = Vec::new();
    for line in text.lines() {
        let def = line.trim().strip_prefix('[').and_then(|r| r.split_once("]:")).filter(|(id, _)| !id.is_empty() && !id.contains('['));
        match def {
            Some((id, url)) if !url.trim().is_empty() => refs.push((id.to_lowercase(), url.split_whitespace().next().unwrap_or("").to_string())),
            _ => body.push(line),
        }
    }
    let mut out = body.join("\n");
    for (id, url) in refs {
        let mut from = 0;
        while let Some(k) = out[from..].find("][").map(|k| k + from) {
            let rest = &out[k + 2..];
            // ids ignore case
            let hit = rest.is_char_boundary(id.len().min(rest.len()))
                && rest.get(..id.len()).is_some_and(|r| r.eq_ignore_ascii_case(&id))
                && rest[id.len()..].starts_with(']');
            if hit {
                let link = format!("]({url})");
                out.replace_range(k..k + 3 + id.len(), &link);
                from = k + link.len();
            } else {
                from = k + 2;
            }
        }
    }
    out
}

/// Inline text into a block, with the pictures it holds as blocks after it.
fn push_inline(blocks: &mut Vec<Block>, text: &str, make: impl FnOnce(Vec<Span>) -> Block) {
    let (spans, images) = inline(text);
    if spans.iter().any(|s| !s.text.trim().is_empty()) {
        blocks.push(make(spans));
    }
    blocks.extend(images);
}

fn is_rule(t: &str) -> bool {
    let t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn heading(t: &str) -> Option<(u8, &str)> {
    let level = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&level) {
        let rest = &t[level..];
        if rest.is_empty() || rest.starts_with(' ') {
            return Some((level as u8, rest.trim().trim_end_matches('#').trim()));
        }
    }
    None
}

fn list_item(t: &str) -> Option<(String, &str)> {
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some(("•".to_string(), rest));
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits < 4 {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((format!("{}.", &t[..digits]), r));
        }
    }
    None
}

/// Inline Markdown: spans of text and the pictures met on the way.
pub fn inline(text: &str) -> (Vec<Span>, Vec<Block>) {
    let mut out = Vec::new();
    let mut images = Vec::new();
    inline_into(text, Span::default(), &mut out, &mut images);
    // glue neighbours with the same look
    let mut merged: Vec<Span> = Vec::new();
    for s in out {
        match merged.last_mut() {
            Some(last) if Span { text: String::new(), ..last.clone() } == Span { text: String::new(), ..s.clone() } => last.text.push_str(&s.text),
            _ => merged.push(s),
        }
    }
    (merged, images)
}

fn inline_into(text: &str, style: Span, out: &mut Vec<Span>, images: &mut Vec<Block>) {
    let c: Vec<char> = text.chars().collect();
    let mut st = style.clone();
    let mut buf = String::new();
    let mut i = 0;
    let push = |buf: &mut String, st: &Span, out: &mut Vec<Span>| {
        if !buf.is_empty() {
            out.push(Span { text: std::mem::take(buf), ..st.clone() });
        }
    };
    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        if ch == '\\' && next.is_some_and(|n| n.is_ascii_punctuation()) {
            buf.push(c[i + 1]);
            i += 2;
            continue;
        }
        if ch == '`' {
            if let Some(end) = find(&c, i + 1, "`") {
                push(&mut buf, &st, out);
                out.push(Span { text: c[i + 1..end].iter().collect(), code: true, ..st.clone() });
                i = end + 1;
                continue;
            }
        }
        if ch == '!' && next == Some('[') {
            if let Some((alt, url, end)) = link_at(&c, i + 1) {
                images.push(Block::Image { url, alt, link: style.link.clone() });
                i = end;
                continue;
            }
        }
        if ch == '[' {
            if let Some((label, url, end)) = link_at(&c, i) {
                push(&mut buf, &st, out);
                inline_into(&label, Span { link: Some(url), ..st.clone() }, out, images);
                i = end;
                continue;
            }
        }
        if (ch == '*' && next == Some('*')) || (ch == '_' && next == Some('_')) {
            push(&mut buf, &st, out);
            st.bold = !st.bold;
            i += 2;
            continue;
        }
        if ch == '~' && next == Some('~') {
            push(&mut buf, &st, out);
            st.strike = !st.strike;
            i += 2;
            continue;
        }
        if ch == '*' {
            // a lone star between spaces is just a star
            let opens = next.is_some_and(|n| !n.is_whitespace());
            let closes = i > 0 && !c[i - 1].is_whitespace();
            if (st.italic && closes) || (!st.italic && opens && find(&c, i + 1, "*").is_some()) {
                push(&mut buf, &st, out);
                st.italic = !st.italic;
                i += 1;
                continue;
            }
        }
        buf.push(ch);
        i += 1;
    }
    push(&mut buf, &st, out);
}

fn find(c: &[char], from: usize, what: &str) -> Option<usize> {
    let w: Vec<char> = what.chars().collect();
    (from..c.len().saturating_sub(w.len() - 1)).find(|&k| c[k..k + w.len()] == w[..])
}

/// `[label](url)` starting at `c[start] == '['`: (label, url, index after it).
fn link_at(c: &[char], start: usize) -> Option<(String, String, usize)> {
    if c.get(start) != Some(&'[') {
        return None;
    }
    // the label may hold brackets itself (a picture inside a link)
    let mut depth = 0;
    let mut k = start;
    let close = loop {
        match c.get(k)? {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break k;
                }
            }
            _ => {}
        }
        k += 1;
    };
    if c.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = (close + 2..c.len()).find(|&k| c[k] == ')')?;
    let target: String = c[close + 2..end].iter().collect();
    // `(url "title")`
    let url = target.split_whitespace().next().unwrap_or("").trim_matches(['<', '>']).to_string();
    Some((c[start + 1..close].iter().collect(), url, end + 1))
}

// ---------------------------------------------------------------- HTML

/// Turn the HTML that descriptions use into Markdown, drop the rest of the tags.
pub fn html_to_markdown(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        if let Some(body) = after.strip_prefix("!--") {
            rest = body.find("-->").map(|e| &body[e + 3..]).unwrap_or("");
            continue;
        }
        let first = after.chars().next();
        let is_tag = first.is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '/' || ch == '!');
        let Some(gt) = after.find('>').filter(|_| is_tag) else {
            out.push('<');
            rest = after;
            continue;
        };
        let tag = &after[..gt];
        rest = &after[gt + 1..];
        // <https://example.com> is a link, not a tag
        if tag.starts_with("http://") || tag.starts_with("https://") {
            out.push_str(&format!("[{tag}]({tag})"));
            continue;
        }
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
        let md = match (name.as_str(), closing) {
            ("br", _) | ("p", _) | ("div", _) | ("center", _) | ("ul", _) | ("ol", _) | ("table", _) | ("tr", true) | ("details", _) => "\n".to_string(),
            ("summary", false) => "\n**".to_string(),
            ("summary", true) => "**\n".to_string(),
            ("hr", _) => "\n\n---\n\n".to_string(),
            ("li", false) => "\n- ".to_string(),
            ("li", true) => "\n".to_string(),
            ("b" | "strong", _) => "**".to_string(),
            ("i" | "em", _) => "*".to_string(),
            ("s" | "del" | "strike", _) => "~~".to_string(),
            ("code", _) => "`".to_string(),
            ("td" | "th", false) => "| ".to_string(),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                let level = name[1..].parse::<usize>().unwrap_or(2);
                format!("\n\n{} ", "#".repeat(level))
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => "\n\n".to_string(),
            ("img", false) => match attr(tag, "src") {
                Some(src) => format!(" ![{}]({src}) ", attr(tag, "alt").unwrap_or_default()),
                None => String::new(),
            },
            ("a", false) => match attr(tag, "href") {
                // the text up to </a> becomes the link label
                Some(href) => {
                    let lower = rest.to_lowercase();
                    match lower.find("</a>") {
                        Some(end) => {
                            let label = html_to_markdown(&rest[..end]);
                            rest = &rest[end + 4..];
                            let label = label.trim();
                            if label.is_empty() { String::new() } else { format!("[{label}]({href})") }
                        }
                        None => String::new(),
                    }
                }
                None => String::new(),
            },
            ("iframe", false) => match attr(tag, "src") {
                Some(src) => format!("\n\n[Смотреть видео]({src})\n\n"),
                None => String::new(),
            },
            _ => String::new(),
        };
        out.push_str(&md);
    }
    out.push_str(rest);
    decode_entities(&out)
}

/// Value of an attribute inside a tag: `src="x"`, `src='x'` or `src=x`.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let at = from + pos;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[from..].trim_start();
        if !before_ok || !after.starts_with('=') {
            continue;
        }
        let value_start = tag.len() - after.len() + 1;
        let v = tag[value_start..].trim_start();
        let value = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
            _ => v.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or(""),
        };
        return Some(decode_entities(value));
    }
    None
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after.find(';').filter(|&k| k <= 8);
        let decoded = semi.and_then(|k| {
            let ent = &after[..k];
            let ch = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "hellip" => Some('…'),
                "copy" => Some('©'),
                _ => ent
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            }?;
            Some((ch, k))
        });
        match decoded {
            Some((ch, k)) => {
                out.push(ch);
                rest = &after[k + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Plain text of spans (tests, short previews).
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_lists_quotes_and_code() {
        let b = parse("# Title\n\nSome *soft*\ntext with **bold**.\n\n- one\n  - two\n1. first\n> quoted\n\n```\nlet x = 1;\n```\n---\n");
        assert_eq!(b[0], Block::Heading(1, vec![Span { text: "Title".into(), ..Default::default() }]));
        let Block::Paragraph(p) = &b[1] else { panic!("{:?}", b[1]) };
        assert_eq!(plain(p), "Some soft text with bold.");
        assert!(p.iter().any(|s| s.italic && s.text == "soft"));
        assert!(p.iter().any(|s| s.bold && s.text == "bold"));
        assert!(matches!(&b[2], Block::Item(0, m, s) if m == "•" && plain(s) == "one"));
        assert!(matches!(&b[3], Block::Item(1, _, s) if plain(s) == "two"));
        assert!(matches!(&b[4], Block::Item(0, m, _) if m == "1."));
        assert!(matches!(&b[5], Block::Quote(s) if plain(s) == "quoted"));
        assert_eq!(b[6], Block::Code("let x = 1;".into()));
        assert_eq!(b[7], Block::Rule);
    }

    #[test]
    fn links_and_pictures() {
        let b = parse("See [the wiki](https://w.example) and `code`.\n\n[![Banner](https://img/b.png)](https://patreon.com/x)\n");
        let Block::Paragraph(p) = &b[0] else { panic!() };
        assert!(p.iter().any(|s| s.text == "the wiki" && s.link.as_deref() == Some("https://w.example")));
        assert!(p.iter().any(|s| s.code && s.text == "code"));
        assert_eq!(b[1], Block::Image { url: "https://img/b.png".into(), alt: "Banner".into(), link: Some("https://patreon.com/x".into()) });
    }

    #[test]
    fn html_is_turned_into_markdown() {
        let b = parse(
            "<center><h2>Big &amp; bold</h2><img src=\"https://i/a.png\" alt='A'><br>\
             <a href=\"https://x.y\"><b>Go</b></a> 1 < 2</center><!-- hidden --><iframe src=\"https://youtube.com/embed/q\"></iframe>",
        );
        assert!(matches!(&b[0], Block::Heading(2, s) if plain(s) == "Big & bold"), "{b:?}");
        assert!(b.contains(&Block::Image { url: "https://i/a.png".into(), alt: "A".into(), link: None }));
        let link = b.iter().find_map(|x| match x {
            Block::Paragraph(p) => p.iter().find(|s| s.link.as_deref() == Some("https://x.y")).cloned(),
            _ => None,
        });
        assert!(link.is_some_and(|s| s.bold && s.text == "Go"), "{b:?}");
        assert!(b.iter().any(|x| matches!(x, Block::Paragraph(p) if plain(p).contains("1 < 2"))), "{b:?}");
        assert!(b.iter().any(|x| matches!(x, Block::Paragraph(p) if p.iter().any(|s| s.link.as_deref() == Some("https://youtube.com/embed/q")))));
        assert!(!format!("{b:?}").contains("hidden"));
    }

    #[test]
    fn reference_links_are_resolved() {
        let b = parse("Read [the rules][R1] first.\n\n[r1]: https://example.com/rules \"Rules\"\n");
        assert_eq!(b.len(), 1, "{b:?}");
        let Block::Paragraph(p) = &b[0] else { panic!() };
        assert!(p.iter().any(|s| s.text == "the rules" && s.link.as_deref() == Some("https://example.com/rules")), "{p:?}");
    }

    #[test]
    fn tables_skip_the_separator_row() {
        let b = parse("| A | B |\n|---|:-:|\n| 1 | **2** |\n");
        assert_eq!(b.len(), 2);
        let Block::Row(cells) = &b[1] else { panic!() };
        assert!(cells[1][0].bold);
    }

    #[test]
    fn stray_stars_and_entities_stay_text() {
        let b = parse("2 * 3 = 6 &copy; &#169; &#xA9; &bogus; a_b_c");
        let Block::Paragraph(s) = &b[0] else { panic!("{b:?}") };
        assert_eq!(plain(s), "2 * 3 = 6 © © © &bogus; a_b_c");
        assert!(s.iter().all(|x| !x.italic));
    }
}
