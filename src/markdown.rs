const STYLE: &str = "body{font:16px/1.6 'Segoe UI',sans-serif;max-width:46em;margin:2em auto;padding:0 1em;color:#222}pre,code{background:#f3f3f3;font-family:Consolas,monospace}code{padding:.1em .3em}pre{padding:.8em;overflow:auto}pre code{padding:0}blockquote{margin:1em 0;padding-left:1em;border-left:4px solid #ccc;color:#555}table{border-collapse:collapse}td,th{border:1px solid #ccc;padding:.3em .7em}img{max-width:100%}";

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn page(title: &str, body: &str, base: Option<&str>) -> String {
    let base = base
        .map(|b| format!("<base href=\"{}\">", escape(b)))
        .unwrap_or_default();
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">{base}<title>{}</title><style>{STYLE}</style></head><body>\n{body}</body></html>\n",
        escape(title)
    )
}

pub fn to_html(src: &str) -> String {
    blocks(&src.lines().collect::<Vec<_>>())
}

fn heading(t: &str) -> Option<(usize, &str)> {
    let n = t.chars().take_while(|c| *c == '#').count();
    let rest = t[n..]
        .strip_prefix(' ')
        .or_else(|| t[n..].is_empty().then_some(""))?;
    (1..=6)
        .contains(&n)
        .then(|| (n, rest.trim().trim_end_matches('#').trim()))
}

fn is_rule(t: &str) -> bool {
    let s: String = t.split_whitespace().collect();
    s.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|c| s.chars().all(|x| c.starts_with(x)))
}

fn marker(line: &str) -> Option<(usize, bool, &str)> {
    let s = line.trim_start();
    let indent = line.len() - s.len();
    if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| s.strip_prefix(m)) {
        return Some((indent, false, rest));
    }
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    let rest = s[digits..]
        .strip_prefix(". ")
        .or_else(|| s[digits..].strip_prefix(") "))?;
    (digits > 0 && digits < 10).then_some((indent, true, rest))
}

fn fence_of(t: &str) -> Option<&'static str> {
    ["```", "~~~"].into_iter().find(|f| t.starts_with(f))
}

fn starts_block(line: &str) -> bool {
    let t = line.trim();
    fence_of(t).is_some()
        || heading(t).is_some()
        || t.starts_with('>')
        || is_rule(t)
        || marker(line).is_some()
}

fn cells(line: &str) -> Vec<&str> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    t.strip_suffix('|')
        .unwrap_or(t)
        .split('|')
        .map(str::trim)
        .collect()
}

fn is_separator(line: &str) -> bool {
    let c = cells(line);
    line.contains('-')
        && c.iter()
            .all(|x| !x.is_empty() && x.chars().all(|ch| ch == '-' || ch == ':'))
}

fn row(tag: &str, cells: &[&str], aligns: &[&str]) -> String {
    let mut out = String::from("<tr>");
    for (i, c) in cells.iter().enumerate() {
        let style = aligns
            .get(i)
            .map(|a| format!(" style=\"text-align:{a}\""))
            .unwrap_or_default();
        out += &format!("<{tag}{style}>{}</{tag}>", inline(c));
    }
    out + "</tr>\n"
}

fn blocks(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.is_empty() {
            i += 1;
        } else if let Some(fence) = fence_of(t) {
            let lang = escape(t.trim_start_matches(fence).trim());
            let class = if lang.is_empty() {
                String::new()
            } else {
                format!(" class=\"language-{lang}\"")
            };
            let mut code = String::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with(fence) {
                code += lines[i];
                code.push('\n');
                i += 1;
            }
            i += 1;
            out += &format!("<pre><code{class}>{}</code></pre>\n", escape(&code));
        } else if let Some((n, text)) = heading(t) {
            out += &format!("<h{n}>{}</h{n}>\n", inline(text));
            i += 1;
        } else if is_rule(t) {
            out += "<hr>\n";
            i += 1;
        } else if t.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('>') {
                let l = lines[i].trim().trim_start_matches('>');
                inner.push(l.strip_prefix(' ').unwrap_or(l));
                i += 1;
            }
            out += &format!("<blockquote>\n{}</blockquote>\n", blocks(&inner));
        } else if let Some((indent, ordered, _)) = marker(line) {
            let mut items: Vec<Vec<&str>> = Vec::new();
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() {
                    let next = lines[i + 1..].iter().find(|x| !x.trim().is_empty());
                    if next
                        .is_some_and(|n| marker(n).map_or(n.starts_with(' '), |m| m.1 == ordered))
                    {
                        if let Some(last) = items.last_mut() {
                            last.push("");
                        }
                        i += 1;
                        continue;
                    }
                    break;
                }
                let lead = l.len() - l.trim_start().len();
                match marker(l) {
                    Some((ind, o, rest)) if ind < indent + 2 && o == ordered => {
                        items.push(vec![rest])
                    }
                    _ if lead > 0 && !items.is_empty() => {
                        let cut = lead.min(indent + 2);
                        items.last_mut().unwrap().push(&l[cut..]);
                    }
                    _ => break,
                }
                i += 1;
            }
            let tag = if ordered { "ol" } else { "ul" };
            out += &format!("<{tag}>\n");
            for item in items {
                let mut html = blocks(&item);
                if !item.contains(&"") {
                    if let Some((first, tail)) = html
                        .strip_prefix("<p>")
                        .and_then(|r| r.split_once("</p>\n"))
                    {
                        html = format!("{first}\n{tail}");
                    }
                }
                out += &format!("<li>{}</li>\n", html.trim_end());
            }
            out += &format!("</{tag}>\n");
        } else if t.contains('|') && lines.get(i + 1).is_some_and(|s| is_separator(s)) {
            let head = cells(t);
            let aligns: Vec<&str> = cells(lines[i + 1])
                .iter()
                .map(|c| match (c.starts_with(':'), c.ends_with(':')) {
                    (true, true) => "center",
                    (false, true) => "right",
                    _ => "left",
                })
                .collect();
            out += "<table>\n";
            out += &row("th", &head, &aligns);
            i += 2;
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                out += &row("td", &cells(lines[i]), &aligns);
                i += 1;
            }
            out += "</table>\n";
        } else {
            let start = i;
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() && !starts_block(lines[i]) {
                i += 1;
            }
            let text = lines[start..i]
                .iter()
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join("\n");
            out += &format!("<p>{}</p>\n", inline(&text));
        }
    }
    out
}

fn find(c: &[char], from: usize, pat: &[char]) -> Option<usize> {
    (from..(c.len() + 1).saturating_sub(pat.len())).find(|&k| c[k..].starts_with(pat))
}

fn safe_url(url: &str) -> String {
    let lower = url.trim().to_ascii_lowercase();
    if ["javascript:", "vbscript:", "data:"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        "#".into()
    } else {
        escape(url)
    }
}

fn link(c: &[char], i: usize) -> Option<(String, usize)> {
    let image = c[i] == '!';
    let open = i + usize::from(image);
    if c.get(open) != Some(&'[') {
        return None;
    }
    let close = find(c, open + 1, &[']'])?;
    if c.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = find(c, close + 2, &[')'])?;
    let text: String = c[open + 1..close].iter().collect();
    let target: String = c[close + 2..end].iter().collect();
    let url = safe_url(target.split_whitespace().next().unwrap_or(""));
    let html = if image {
        format!("<img src=\"{url}\" alt=\"{}\">", escape(&text))
    } else {
        format!("<a href=\"{url}\">{}</a>", inline(&text))
    };
    Some((html, end + 1 - i))
}

fn inline(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            '\\' if c.get(i + 1).is_some_and(char::is_ascii_punctuation) => {
                out += &escape(&c[i + 1].to_string());
                i += 2;
                continue;
            }
            '`' => {
                if let Some(end) = find(&c, i + 1, &['`']) {
                    let code: String = c[i + 1..end].iter().collect();
                    out += &format!("<code>{}</code>", escape(&code));
                    i = end + 1;
                    continue;
                }
            }
            '!' | '[' => {
                if let Some((html, n)) = link(&c, i) {
                    out += &html;
                    i += n;
                    continue;
                }
            }
            '*' | '_' => {
                let k = c[i..].iter().take_while(|x| **x == ch).count().min(2);
                let delim = vec![ch; k];
                let opens = c.get(i + k).is_some_and(|x| !x.is_whitespace())
                    && (ch == '*' || i == 0 || !c[i - 1].is_alphanumeric());
                if let Some(end) = find(&c, i + k + 1, &delim).filter(|_| opens) {
                    if !c[end - 1].is_whitespace() {
                        let inner: String = c[i + k..end].iter().collect();
                        let tag = if k == 2 { "strong" } else { "em" };
                        out += &format!("<{tag}>{}</{tag}>", inline(&inner));
                        i = end + k;
                        continue;
                    }
                }
                out += &delim.iter().collect::<String>();
                i += k;
                continue;
            }
            _ => {}
        }
        out += &escape(&ch.to_string());
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_markdown() {
        let html = to_html(
            "# Title\n\nsome *em* and **strong** with `a<b` and [link](http://x.y/a)\n\n- one\n- two\n  - nested\n\n1. a\n2. b\n\n> quoted\n\n```rust\nlet x = 1 < 2;\n```\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n![pic](p.png)\n\n---\n",
        );
        for want in [
            "<h1>Title</h1>",
            "<em>em</em>",
            "<strong>strong</strong>",
            "<code>a&lt;b</code>",
            "<a href=\"http://x.y/a\">link</a>",
            "<li>one</li>",
            "<li>two\n<ul>",
            "<li>nested</li>",
            "<ol>",
            "<blockquote>",
            "<p>quoted</p>",
            "<pre><code class=\"language-rust\">let x = 1 &lt; 2;\n</code></pre>",
            "<th style=\"text-align:left\">a</th>",
            "<td style=\"text-align:right\">2</td>",
            "<img src=\"p.png\" alt=\"pic\">",
            "<hr>",
        ] {
            assert!(html.contains(want), "missing {want} in {html}");
        }
    }

    #[test]
    fn escapes_html_and_bad_urls() {
        let html = to_html("<script>alert(1)</script> [x](javascript:alert(1)) snake_case_name");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("href=\"#\""));
        assert!(html.contains("snake_case_name"));
    }
}
