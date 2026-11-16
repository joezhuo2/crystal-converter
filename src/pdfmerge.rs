use crate::convert::write_pdf;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

const COMPRESSED: &str =
    "uses compressed cross-reference or object streams, which merging does not support";
const DAMAGED: &str = "damaged or unsupported PDF structure";
const MAX_OBJECTS: u32 = 2_000_000;

struct Source {
    objs: BTreeMap<u32, Vec<u8>>,
    pages: u32,
    count: u32,
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn is_delim(b: u8) -> bool {
    b.is_ascii_whitespace() || b"()<>[]{}/%".contains(&b)
}

fn token<'a>(d: &'a [u8], pos: &mut usize) -> &'a [u8] {
    while d.get(*pos).is_some_and(u8::is_ascii_whitespace) {
        *pos += 1;
    }
    let start = *pos;
    while d.get(*pos).is_some_and(|b| !is_delim(*b)) {
        *pos += 1;
    }
    &d[start..*pos]
}

fn number(d: &[u8], pos: &mut usize) -> Option<u32> {
    std::str::from_utf8(token(d, pos)).ok()?.parse().ok()
}

fn int_after(d: &[u8], key: &[u8]) -> Option<u32> {
    let mut p = find(d, key)? + key.len();
    number(d, &mut p)
}

fn ref_after(d: &[u8], key: &[u8]) -> Option<u32> {
    let mut p = find(d, key)? + key.len();
    let n = number(d, &mut p)?;
    number(d, &mut p)?;
    (token(d, &mut p) == b"R").then_some(n)
}

fn parse(d: &[u8]) -> Result<Source, String> {
    if !d.starts_with(b"%PDF") {
        return Err("not a PDF file".into());
    }
    let start = d
        .windows(9)
        .rposition(|w| w == b"startxref")
        .ok_or(DAMAGED)?;
    let mut p = start + 9;
    let mut next = Some(number(d, &mut p).ok_or(DAMAGED)? as usize);
    let mut offsets: BTreeMap<u32, usize> = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut visited = HashSet::new();
    let mut root = None;
    while let Some(off) = next.take() {
        if !visited.insert(off) {
            break;
        }
        if !d.get(off..).is_some_and(|x| x.starts_with(b"xref")) {
            return Err(COMPRESSED.into());
        }
        let mut p = off + 4;
        loop {
            let t = token(d, &mut p);
            if t == b"trailer" {
                break;
            }
            let first: u32 = std::str::from_utf8(t)
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or(DAMAGED)?;
            let count = number(d, &mut p).ok_or(DAMAGED)?;
            for k in 0..count {
                let o = number(d, &mut p).ok_or(DAMAGED)?;
                number(d, &mut p).ok_or(DAMAGED)?;
                let flag = token(d, &mut p);
                let num = first.checked_add(k).ok_or(DAMAGED)?;
                if seen.insert(num) && flag == b"n" && o > 0 {
                    offsets.insert(num, o as usize);
                }
            }
        }
        let end = find(&d[p..], b"startxref").map_or(d.len(), |x| p + x);
        let trailer = &d[p..end];
        if find(trailer, b"/Encrypt").is_some() {
            return Err("is encrypted, which merging does not support".into());
        }
        if find(trailer, b"/XRefStm").is_some() {
            return Err(COMPRESSED.into());
        }
        if root.is_none() {
            root = Some(ref_after(trailer, b"/Root").ok_or(DAMAGED)?);
        }
        next = int_after(trailer, b"/Prev").map(|v| v as usize);
    }
    let mut bounds: Vec<usize> = offsets.values().copied().chain(visited).collect();
    bounds.sort_unstable();
    let mut objs = BTreeMap::new();
    for (&num, &off) in &offsets {
        let bound = bounds.iter().copied().find(|b| *b > off).unwrap_or(d.len());
        let slice = d.get(off..bound).ok_or(DAMAGED)?;
        let mut p = 0;
        if number(slice, &mut p) != Some(num) || number(slice, &mut p).is_none() {
            return Err(DAMAGED.into());
        }
        if token(slice, &mut p) != b"obj" {
            return Err(DAMAGED.into());
        }
        let rest = &slice[p..];
        let end = rest
            .windows(6)
            .rposition(|w| w == b"endobj")
            .ok_or(DAMAGED)?;
        objs.insert(num, rest[..end].trim_ascii().to_vec());
    }
    let catalog = objs.get(&root.ok_or(DAMAGED)?).ok_or(DAMAGED)?;
    let pages = ref_after(catalog, b"/Pages").ok_or(DAMAGED)?;
    let count = objs
        .get(&pages)
        .and_then(|b| int_after(b, b"/Count"))
        .ok_or(DAMAGED)?;
    Ok(Source { objs, pages, count })
}

fn stream_start(b: &[u8]) -> usize {
    let mut from = 0;
    while let Some(i) = find(&b[from..], b"stream").map(|i| from + i) {
        if b[..i].trim_ascii_end().ends_with(b">>") {
            return i;
        }
        from = i + 6;
    }
    b.len()
}

fn reference(d: &[u8], i: usize) -> Option<(u32, usize)> {
    let mut p = i;
    let n = number(d, &mut p)?;
    if !d.get(p).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    number(d, &mut p)?;
    if !d.get(p).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    let mut q = p;
    if token(d, &mut q) != b"R" {
        return None;
    }
    Some((n, q))
}

fn renumber(body: &[u8], base: u32) -> Vec<u8> {
    let (head, tail) = body.split_at(stream_start(body));
    let mut out = Vec::with_capacity(body.len() + 16);
    let mut i = 0;
    while i < head.len() {
        let c = head[i];
        match c {
            b'(' => {
                let mut depth = 0;
                while i < head.len() {
                    let b = head[i];
                    out.push(b);
                    i += 1;
                    match b {
                        b'\\' if i < head.len() => {
                            out.push(head[i]);
                            i += 1;
                        }
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            b'%' => {
                while i < head.len() && head[i] != b'\n' && head[i] != b'\r' {
                    out.push(head[i]);
                    i += 1;
                }
            }
            b'0'..=b'9' if i == 0 || is_delim(head[i - 1]) => {
                if let Some((n, end)) = reference(head, i) {
                    out.extend_from_slice(format!("{} 0 R", n + base).as_bytes());
                    i = end;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out.extend_from_slice(tail);
    out
}

pub fn merge_bytes(pdfs: &[&[u8]]) -> Result<Vec<u8>, String> {
    let mut objs: Vec<Vec<u8>> = vec![Vec::new(), Vec::new()];
    let mut kids = Vec::new();
    let mut total = 0;
    let mut base = 2u32;
    for (n, data) in pdfs.iter().enumerate() {
        let s = parse(data).map_err(|e| format!("PDF {} {e}", n + 1))?;
        let max = s.objs.keys().next_back().copied().unwrap_or(0);
        if base + max > MAX_OBJECTS {
            return Err("too many objects to merge".into());
        }
        objs.resize(objs.len() + max as usize, b"null".to_vec());
        for (num, body) in &s.objs {
            let mut b = renumber(body, base);
            if *num == s.pages {
                if !b.starts_with(b"<<") {
                    return Err(format!("PDF {} {DAMAGED}", n + 1));
                }
                b.splice(2..2, b" /Parent 2 0 R".iter().copied());
            }
            objs[(base + num - 1) as usize] = b;
        }
        kids.push(format!("{} 0 R", base + s.pages));
        total += s.count;
        base += max;
    }
    objs[0] = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
    objs[1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {total} >>",
        kids.join(" ")
    )
    .into_bytes();
    Ok(write_pdf(&objs))
}

pub fn merge(files: &[PathBuf], out: &Path) -> Result<(), String> {
    if files.len() < 2 {
        return Err("drop at least two PDFs to merge".into());
    }
    let data: Vec<Vec<u8>> = files
        .iter()
        .map(|f| fs::read(f).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    fs::write(out, merge_bytes(&refs)?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::pdf_from_jpegs;

    fn jpeg(w: u8) -> Vec<u8> {
        vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0, 16, 0, w, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1,
            0xFF, 0xD9,
        ]
    }

    #[test]
    fn merges_in_order_and_stays_parseable() {
        let a = pdf_from_jpegs(&[jpeg(32), jpeg(33)]).unwrap();
        let b = pdf_from_jpegs(&[jpeg(34)]).unwrap();
        let merged = merge_bytes(&[&a, &b]).unwrap();
        let s = parse(&merged).unwrap();
        assert_eq!(s.count, 3);
        let text = String::from_utf8_lossy(&merged);
        let widths: Vec<usize> = text
            .match_indices("/Width ")
            .map(|(i, _)| {
                text[i + 7..]
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect();
        assert_eq!(widths, vec![32, 33, 34]);
        assert_eq!(text.matches("/Parent 2 0 R").count(), 2);
        let again = merge_bytes(&[&merged, &b]).unwrap();
        assert_eq!(parse(&again).unwrap().count, 4);
    }

    #[test]
    fn refuses_unsupported_pdfs() {
        let ok = pdf_from_jpegs(&[jpeg(32)]).unwrap();
        let stream_xref =
            b"%PDF-1.5\n1 0 obj\n<< /Type /XRef >>\nstream\nendstream\nendobj\nstartxref\n9\n%%EOF";
        assert!(merge_bytes(&[&ok, stream_xref])
            .unwrap_err()
            .contains("compressed"));
        let mut encrypted = ok.clone();
        let at = find(&encrypted, b"/Root 1 0 R").unwrap() + 11;
        encrypted.splice(at..at, b" /Encrypt 9 0 R".iter().copied());
        assert!(merge_bytes(&[&ok, &encrypted])
            .unwrap_err()
            .contains("encrypted"));
        assert!(merge_bytes(&[&ok, b"hello"]).is_err());
    }
}
