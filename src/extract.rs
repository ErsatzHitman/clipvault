//! Best-effort text extraction from stored files so their contents become searchable.
//! Everything here is untrusted-input parsing: it is size-capped, panic-contained and time-boxed,
//! and it only ever runs on the dedicated extraction thread, never on the UI or clipboard threads.
use crate::{
    model::Entry,
    storage::{Store, StoredFile},
};
use anyhow::{Context, Result, bail};
use std::{
    io::Read,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::mpsc,
    time::Duration,
};

pub const MAX_CHARS: usize = 200_000;
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);

pub fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase()
}
/// Formats whose text needs a real parser (done asynchronously after capture).
pub fn needs_extraction(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "pdf" | "docx" | "pptx" | "xlsx")
}
pub fn is_plain_text(name: &str) -> bool {
    matches!(
        ext_of(name).as_str(),
        "txt"
            | "md"
            | "rs"
            | "json"
            | "csv"
            | "log"
            | "toml"
            | "yaml"
            | "yml"
            | "html"
            | "css"
            | "js"
            | "ts"
            | "py"
    )
}
pub fn is_image_name(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "png" | "jpg" | "jpeg" | "webp")
}
/// Decodes UTF-8 or BOM-marked UTF-16; anything else is not treated as text.
pub fn decode_text(data: &[u8]) -> Option<String> {
    if let Some(rest) = data.strip_prefix(&[0xFF, 0xFE]) {
        return Some(utf16(rest, u16::from_le_bytes));
    }
    if let Some(rest) = data.strip_prefix(&[0xFE, 0xFF]) {
        return Some(utf16(rest, u16::from_be_bytes));
    }
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    std::str::from_utf8(data).ok().map(String::from)
}
fn utf16(rest: &[u8], f: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|b| f(*b)).collect();
    String::from_utf16_lossy(&units)
}
fn clean(s: &str) -> String {
    let mut out = String::new();
    let (mut gap, mut n) = (false, 0usize);
    for ch in s.chars().filter(|c| *c != '\0') {
        if ch.is_whitespace() {
            gap = !out.is_empty();
        } else {
            if gap {
                out.push(' ');
                gap = false;
            }
            out.push(ch);
            n += 1;
            if n >= MAX_CHARS {
                break;
            }
        }
    }
    out
}
/// Extracts text synchronously. Parsers may panic on hostile input; that becomes an `Err`.
pub fn extract_text(path: &Path) -> Result<String> {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        bail!("Unsupported file");
    }
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut data)?;
    let raw = match ext_of(name).as_str() {
        "pdf" => catch_unwind(AssertUnwindSafe(|| {
            pdf_extract::extract_text_from_mem(&data)
        }))
        .map_err(|_| anyhow::anyhow!("PDF parser panicked"))?
        .map_err(|e| anyhow::anyhow!("PDF text extraction failed: {e}"))?,
        "docx" | "pptx" | "xlsx" => office_text(&data, &ext_of(name))?,
        _ if is_plain_text(name) => decode_text(&data).context("Not text")?,
        _ => bail!("Unsupported file type"),
    };
    Ok(clean(&raw))
}
/// Runs `extract_text` on a helper thread with a deadline. A thread stuck in a parser is abandoned.
pub fn extract_with_timeout(path: &Path, timeout: Duration) -> Result<String> {
    let (tx, rx) = mpsc::channel();
    let p = path.to_owned();
    std::thread::Builder::new()
        .name("clipvault-extract-worker".into())
        .spawn(move || {
            let _ = tx.send(catch_unwind(AssertUnwindSafe(|| extract_text(&p))));
        })?;
    match rx.recv_timeout(timeout) {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => bail!("Extractor panicked"),
        Err(_) => bail!("Text extraction timed out"),
    }
}
/// Text for every managed file in an entry, formatted as `[name]\ncontent` blocks.
pub fn extract_entry(store: &Store, e: &Entry) -> String {
    let mut out = String::new();
    let Some(manifest) = e
        .blob
        .as_deref()
        .and_then(|b| store.blob_path(b).ok())
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Vec<StoredFile>>(&b).ok())
    else {
        return out;
    };
    for f in manifest.iter().filter(|f| needs_extraction(&f.name)) {
        if out.chars().count() >= MAX_CHARS * 2 {
            break;
        }
        let Ok(path) = store.blob_path(&f.stored) else {
            continue;
        };
        if let Ok(t) = extract_with_timeout(&path, TIMEOUT)
            && !t.is_empty()
        {
            out.push_str(&format!("\n[{}]\n{t}", f.name));
        }
    }
    out
}

// --- Office Open XML (docx/pptx/xlsx): a minimal, bounded ZIP reader plus an XML text scanner ---
fn u16le(d: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?) as usize)
}
fn u32le(d: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?) as usize)
}
struct ZipEntry {
    name: String,
    method: usize,
    csize: usize,
    offset: usize,
}
fn zip_entries(d: &[u8]) -> Result<Vec<ZipEntry>> {
    let start = d.len().saturating_sub(22 + 65535);
    let eocd = (start..d.len().saturating_sub(21))
        .rev()
        .find(|&i| d[i..].starts_with(b"PK\x05\x06"))
        .context("Not a zip file")?;
    let count = u16le(d, eocd + 10).context("Bad zip")?;
    let mut pos = u32le(d, eocd + 16).context("Bad zip")?;
    let mut out = Vec::new();
    for _ in 0..count.min(5000) {
        if d.get(pos..pos + 4) != Some(b"PK\x01\x02") {
            break;
        }
        let (n, x, c) = (
            u16le(d, pos + 28).context("Bad zip")?,
            u16le(d, pos + 30).context("Bad zip")?,
            u16le(d, pos + 32).context("Bad zip")?,
        );
        let name = d.get(pos + 46..pos + 46 + n).context("Bad zip")?;
        out.push(ZipEntry {
            name: String::from_utf8_lossy(name).into_owned(),
            method: u16le(d, pos + 10).context("Bad zip")?,
            csize: u32le(d, pos + 20).context("Bad zip")?,
            offset: u32le(d, pos + 42).context("Bad zip")?,
        });
        pos += 46 + n + x + c;
    }
    Ok(out)
}
fn zip_read(d: &[u8], e: &ZipEntry) -> Result<Vec<u8>> {
    if d.get(e.offset..e.offset + 4) != Some(b"PK\x03\x04") {
        bail!("Bad zip entry");
    }
    let start = e.offset
        + 30
        + u16le(d, e.offset + 26).context("Bad zip")?
        + u16le(d, e.offset + 28).context("Bad zip")?;
    let raw = d
        .get(start..start.saturating_add(e.csize))
        .context("Bad zip")?;
    let mut out = Vec::new();
    match e.method {
        0 => out.extend_from_slice(&raw[..raw.len().min(MAX_ENTRY_BYTES as usize)]),
        8 => {
            flate2::read::DeflateDecoder::new(raw)
                .take(MAX_ENTRY_BYTES)
                .read_to_end(&mut out)?;
        }
        _ => bail!("Unsupported zip compression"),
    }
    Ok(out)
}
fn office_text(d: &[u8], ext: &str) -> Result<String> {
    let mut entries = zip_entries(d)?;
    entries.retain(|e| match ext {
        "docx" => e.name == "word/document.xml",
        "pptx" => e.name.starts_with("ppt/slides/slide") && e.name.ends_with(".xml"),
        _ => e.name == "xl/sharedStrings.xml",
    });
    // Natural slide order: slide2 before slide10.
    entries.sort_by_key(|e| (e.name.len(), e.name.clone()));
    let mut out = String::new();
    for e in &entries {
        out.push_str(&xml_text(&String::from_utf8_lossy(&zip_read(d, e)?)));
        out.push('\n');
        if out.len() > MAX_CHARS * 4 {
            break;
        }
    }
    if out.trim().is_empty() {
        bail!("No text found");
    }
    Ok(out)
}
/// Collects the text of `<w:t>`, `<a:t>` and `<t>` elements, breaking lines at paragraph ends.
pub fn xml_text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    let mut in_text = false;
    while let Some(i) = rest.find('<') {
        if in_text {
            out.push_str(&unescape(&rest[..i]));
        }
        in_text = false;
        rest = &rest[i + 1..];
        let Some(j) = rest.find('>') else { break };
        let tag = &rest[..j];
        rest = &rest[j + 1..];
        let (closing, body) = tag.strip_prefix('/').map_or((false, tag), |t| (true, t));
        let name = body
            .split([' ', '\t', '\r', '\n', '/'])
            .next()
            .unwrap_or("");
        if closing {
            if matches!(name, "w:p" | "a:p" | "si" | "row") {
                out.push('\n');
            } else if matches!(name, "w:t" | "a:t" | "t") {
                out.push(' ');
            }
        } else if matches!(name, "w:t" | "a:t" | "t") && !tag.ends_with('/') {
            in_text = true;
        }
    }
    out
}
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    /// Smallest valid single-page PDF with one line of Helvetica text.
    pub(crate) fn tiny_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for o in offsets {
            pdf.extend(format!("{o:010} 00000 n \n").bytes());
        }
        pdf.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .bytes(),
        );
        pdf
    }
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for b in data {
            c ^= u32::from(*b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut cd) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let mut enc =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(data).unwrap();
            let comp = enc.finish().unwrap();
            let off = out.len() as u32;
            let mut common = Vec::new();
            common.extend(20u16.to_le_bytes());
            common.extend(0u16.to_le_bytes());
            common.extend(8u16.to_le_bytes());
            common.extend([0u8; 4]);
            common.extend(crc(data).to_le_bytes());
            common.extend((comp.len() as u32).to_le_bytes());
            common.extend((data.len() as u32).to_le_bytes());
            common.extend((name.len() as u16).to_le_bytes());
            common.extend(0u16.to_le_bytes());
            out.extend(b"PK\x03\x04");
            out.extend(&common);
            out.extend(name.bytes());
            out.extend(&comp);
            cd.extend(b"PK\x01\x02");
            cd.extend(20u16.to_le_bytes());
            cd.extend(&common[..common.len() - 2]);
            cd.extend([0u8; 2 + 2 + 2 + 2 + 4]);
            cd.extend(off.to_le_bytes());
            cd.extend(name.bytes());
        }
        let cd_off = out.len() as u32;
        out.extend(&cd);
        out.extend(b"PK\x05\x06\0\0\0\0");
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((cd.len() as u32).to_le_bytes());
        out.extend(cd_off.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }
    #[test]
    fn pdf_text_is_extracted() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("invoice.pdf");
        std::fs::write(&p, tiny_pdf("Quarterly invoice for Acme Corporation")).unwrap();
        let t = extract_with_timeout(&p, Duration::from_secs(10)).unwrap();
        assert!(t.contains("Acme"), "{t}");
    }
    #[test]
    fn corrupt_pdf_is_an_error_not_a_panic() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bad.pdf");
        std::fs::write(&p, b"%PDF-1.4 garbage \x00\xff truncated").unwrap();
        assert!(extract_with_timeout(&p, Duration::from_secs(10)).is_err());
        assert!(extract_text(&d.path().join("missing.pdf")).is_err());
    }
    #[test]
    fn docx_pptx_xlsx_text_is_extracted() {
        let d = tempfile::tempdir().unwrap();
        let docx = zip(&[("word/document.xml", b"<w:document><w:p><w:r><w:t xml:space=\"preserve\">Budget &amp; plan</w:t></w:r></w:p></w:document>")]);
        let p = d.path().join("a.docx");
        std::fs::write(&p, docx).unwrap();
        assert_eq!(extract_text(&p).unwrap(), "Budget & plan");
        let pptx = zip(&[
            ("ppt/slides/slide10.xml", b"<a:p><a:t>last</a:t></a:p>"),
            ("ppt/slides/slide2.xml", b"<a:p><a:t>second</a:t></a:p>"),
        ]);
        let p = d.path().join("a.pptx");
        std::fs::write(&p, pptx).unwrap();
        assert_eq!(extract_text(&p).unwrap(), "second last");
        let xlsx = zip(&[(
            "xl/sharedStrings.xml",
            b"<sst><si><t>Revenue</t></si><si><t>Costs</t></si></sst>",
        )]);
        let p = d.path().join("a.xlsx");
        std::fs::write(&p, xlsx).unwrap();
        assert_eq!(extract_text(&p).unwrap(), "Revenue Costs");
        std::fs::write(d.path().join("b.docx"), b"not a zip").unwrap();
        assert!(extract_text(&d.path().join("b.docx")).is_err());
    }
    #[test]
    fn xml_scanner_decodes_entities_and_ignores_markup() {
        assert_eq!(
            clean(&xml_text("<x><tag>ignored</tag><t>a &lt; b</t><w:br/></x>")),
            "a < b"
        );
    }
    #[test]
    fn utf16_text_is_decoded() {
        let mut b = vec![0xFF, 0xFE];
        b.extend("héllo".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_text(&b).unwrap(), "héllo");
        assert!(decode_text(&[0xC3, 0x28]).is_none());
    }
}
