// Text out of an uploaded PDF or Word document, so an author can attach
// one to an AI composer as the source of truth — ported from parelabs'
// /labs/extract-document. Text only: a scanned PDF (pages that are just
// images) has nothing to extract and says so, instead of returning "".

use std::io::{Cursor, Read};

use crate::errors::AppError;

pub const MAX_CHARS: usize = 60_000;
const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

#[derive(Debug, serde::Serialize)]
pub struct Extracted {
    pub text: String,
    pub char_count: usize,
    pub truncated: bool,
    /// "pdf" or "docx".
    pub kind: &'static str,
}

fn unreadable(kind: &str) -> AppError {
    AppError::UnprocessableEntity("document_unreadable", format!("File {kind} ini tidak bisa dibaca — mungkin rusak atau terkunci kata sandi."))
}

pub fn extract(filename: &str, content_type: Option<&str>, bytes: &[u8]) -> Result<Extracted, AppError> {
    let ext = filename.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    let (raw, kind) = if ext == "pdf" || content_type == Some("application/pdf") || bytes.starts_with(b"%PDF") {
        (pdf_text(bytes)?, "pdf")
    } else if ext == "docx" || content_type == Some(DOCX_MIME) {
        (docx_text(bytes)?, "docx")
    } else if ext == "doc" {
        return Err(AppError::UnprocessableEntity("doc_unsupported", "Format .doc lama belum didukung — simpan ulang sebagai .docx atau PDF.".to_string()));
    } else {
        return Err(AppError::UnprocessableEntity("unsupported_document", "Yang bisa dibaca di sini: PDF dan DOCX.".to_string()));
    };

    let text = tidy(&raw);
    if text.trim().is_empty() {
        return Err(AppError::UnprocessableEntity(
            "no_text",
            "Tidak ada teks yang bisa dibaca — kemungkinan dokumen hasil pindai (gambar). Salin teksnya, atau lampirkan halamannya sebagai gambar.".to_string(),
        ));
    }
    let char_count = text.chars().count();
    let truncated = char_count > MAX_CHARS;
    let text = if truncated { text.chars().take(MAX_CHARS).collect() } else { text };
    Ok(Extracted { text, char_count, truncated, kind })
}

fn pdf_text(bytes: &[u8]) -> Result<String, AppError> {
    let owned = bytes.to_vec();
    // pdf-extract panics on some malformed PDFs instead of returning an
    // error; one bad upload must not take a worker thread down with it.
    match std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem(&owned)) {
        Ok(Ok(text)) => Ok(text),
        _ => Err(unreadable("PDF")),
    }
}

fn docx_text(bytes: &[u8]) -> Result<String, AppError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| unreadable("DOCX"))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|_| unreadable("DOCX"))?
        .read_to_string(&mut xml)
        .map_err(|_| unreadable("DOCX"))?;
    Ok(docx_xml_to_text(&xml))
}

/// word/document.xml → text: runs (`w:t`) joined, a newline per
/// paragraph, a tab between table cells.
pub fn docx_xml_to_text(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"w:t" {
                    in_text = true;
                }
            }
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"w:tab" => out.push('\t'),
                b"w:br" | b"w:cr" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => {
                if let Ok(s) = t.unescape() {
                    out.push_str(&s);
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"w:t" => in_text = false,
                b"w:p" => out.push('\n'),
                b"w:tc" => out.push('\t'),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Trailing spaces off every line, and at most one blank line in a row
/// — PDF extraction in particular pads text with both.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docx_paragraphs_runs_and_cells() {
        let xml = r#"<w:document><w:body>
            <w:p><w:r><w:t>Hukum </w:t></w:r><w:r><w:t xml:space="preserve">Newton &amp; gaya</w:t></w:r></w:p>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>m</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
            <w:p><w:r><w:t>F</w:t><w:tab/><w:t>= ma</w:t></w:r></w:p>
        </w:body></w:document>"#;
        let text = tidy(&docx_xml_to_text(xml));
        assert!(text.starts_with("Hukum Newton & gaya"), "{text}");
        assert!(text.contains("F\t= ma"), "{text}");
        assert!(text.contains('m') && text.contains('a'));
    }

    #[test]
    fn rejects_old_doc_and_unknown_formats() {
        assert!(matches!(extract("tugas.doc", None, b"x"), Err(AppError::UnprocessableEntity("doc_unsupported", _))));
        assert!(matches!(extract("gambar.png", None, b"x"), Err(AppError::UnprocessableEntity("unsupported_document", _))));
    }

    #[test]
    fn a_broken_pdf_is_an_error_not_a_panic() {
        assert!(matches!(extract("rusak.pdf", None, b"%PDF-1.4 not really"), Err(AppError::UnprocessableEntity(..))));
    }

    #[test]
    fn tidy_collapses_blank_runs() {
        assert_eq!(tidy("a  \n\n\n\nb\n"), "a\n\nb");
    }
}
