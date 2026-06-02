use std::path::Path;

use base64::Engine;

use super::super::constants::PDF_MAX_PAGES_PER_READ;
use super::super::output::{PartsOutput, PartsOutputFile, PdfOutput};

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("Failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse PDF: {0}")]
    Parse(String),
    #[error("Invalid page range: {0}")]
    InvalidPageRange(String),
    #[error("Page count exceeds maximum of {0}")]
    TooManyPages(u32),
    #[error("PDF has {0} pages, but requested page {1}")]
    PageNotFound(u32, u32),
}

fn parse_page_range(pages: &str, max_pages: u32) -> Result<Vec<u32>, PdfError> {
    let mut result = Vec::new();

    for part in pages.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if part.contains('-') {
            let sides: Vec<&str> = part.split('-').collect();
            if sides.len() != 2 {
                return Err(PdfError::InvalidPageRange(format!(
                    "Invalid range: {}",
                    part
                )));
            }
            let start: u32 = sides[0]
                .trim()
                .parse()
                .map_err(|_| PdfError::InvalidPageRange(format!("Invalid page: {}", sides[0])))?;
            let end: u32 = sides[1]
                .trim()
                .parse()
                .map_err(|_| PdfError::InvalidPageRange(format!("Invalid page: {}", sides[1])))?;
            if start == 0 || end == 0 {
                return Err(PdfError::InvalidPageRange("Page must be non-zero".into()));
            }
            if start > end {
                return Err(PdfError::InvalidPageRange(format!("{} > {}", start, end)));
            }
            for page in start..=end {
                if !result.contains(&page) {
                    result.push(page);
                }
            }
        } else {
            let page: u32 = part
                .parse()
                .map_err(|_| PdfError::InvalidPageRange(format!("Invalid page: {}", part)))?;
            if page == 0 {
                return Err(PdfError::InvalidPageRange("Page must be non-zero".into()));
            }
            if !result.contains(&page) {
                result.push(page);
            }
        }
    }

    result.sort();
    result.dedup();

    if result.len() as u32 > max_pages {
        return Err(PdfError::TooManyPages(max_pages));
    }

    Ok(result)
}

#[allow(dead_code)]
pub async fn read_pdf<P: AsRef<Path>>(file_path: P) -> Result<PdfOutput, PdfError> {
    let file_path = file_path.as_ref();
    let file_path_str = file_path.to_string_lossy().to_string();

    let bytes = tokio::fs::read(file_path).await?;
    let original_size = bytes.len() as u64;
    let base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(PdfOutput {
        file_path: file_path_str,
        base64,
        original_size,
    })
}

#[allow(dead_code)]
pub fn read_pdf_from_bytes(file_path: &str, bytes: &[u8]) -> PdfOutput {
    let original_size = bytes.len() as u64;
    let base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    PdfOutput {
        file_path: file_path.to_string(),
        base64,
        original_size,
    }
}

pub(crate) fn plan_pdf_parts_from_bytes(
    file_path: &str,
    bytes: &[u8],
    pages: &str,
    output_dir: &str,
) -> Result<(PartsOutput, Vec<(String, Vec<u8>)>), PdfError> {
    let output_dir_path = Path::new(output_dir);
    let page_numbers = parse_page_range(pages, PDF_MAX_PAGES_PER_READ)?;

    if page_numbers.is_empty() {
        return Err(PdfError::InvalidPageRange("No pages specified".into()));
    }

    let original_size = bytes.len() as u64;
    let doc = lopdf::Document::load_mem(bytes)
        .map_err(|e| PdfError::Parse(format!("PDF parse error: {}", e)))?;
    let page_count = doc.get_pages().len() as u32;

    for &page in &page_numbers {
        if page > page_count {
            return Err(PdfError::PageNotFound(page_count, page));
        }
    }

    let mut files = Vec::new();
    for page_num in &page_numbers {
        let output_path = output_dir_path.join(format!("page_{}.pdf", page_num));
        let mut page_doc = doc.clone();
        let all_pages = page_doc.get_pages();

        let pages_to_delete: Vec<u32> = all_pages
            .keys()
            .filter(|&&p| p != *page_num)
            .cloned()
            .collect();

        if !pages_to_delete.is_empty() {
            page_doc.delete_pages(&pages_to_delete);
        }

        page_doc.prune_objects();
        page_doc.renumber_objects();

        let mut buffer = Vec::new();
        page_doc
            .save_to(&mut buffer)
            .map_err(|e| PdfError::Parse(format!("PDF save error: {}", e)))?;

        files.push((output_path.to_string_lossy().to_string(), buffer));
    }

    Ok((
        PartsOutput {
            file: PartsOutputFile {
                file_path: file_path.to_string(),
                original_size,
                count: page_numbers.len() as u32,
                output_dir: output_dir.to_string(),
            },
        },
        files,
    ))
}

#[allow(dead_code)]
pub async fn extract_pdf_pages<P: AsRef<Path>>(
    file_path: P,
    pages: &str,
    output_dir: &str,
) -> Result<PartsOutput, PdfError> {
    let file_path = file_path.as_ref();
    let file_path_str = file_path.to_string_lossy().to_string();
    let output_dir = Path::new(output_dir);

    let page_numbers = parse_page_range(pages, PDF_MAX_PAGES_PER_READ)?;

    if page_numbers.is_empty() {
        return Err(PdfError::InvalidPageRange("No pages specified".into()));
    }

    let bytes = tokio::fs::read(file_path).await?;
    let original_size = bytes.len() as u64;

    let doc = lopdf::Document::load_mem(&bytes)
        .map_err(|e| PdfError::Parse(format!("PDF parse error: {}", e)))?;

    let page_count = doc.get_pages().len() as u32;

    for &page in &page_numbers {
        if page > page_count {
            return Err(PdfError::PageNotFound(page_count, page));
        }
    }

    tokio::fs::create_dir_all(output_dir).await?;

    for page_num in &page_numbers {
        let output_path = output_dir.join(format!("page_{}.pdf", page_num));
        let mut page_doc = doc.clone();
        let all_pages = page_doc.get_pages();

        let pages_to_delete: Vec<u32> = all_pages
            .keys()
            .filter(|&&p| p != *page_num)
            .cloned()
            .collect();

        if !pages_to_delete.is_empty() {
            page_doc.delete_pages(&pages_to_delete);
        }

        page_doc.prune_objects();
        page_doc.renumber_objects();

        let mut buffer = Vec::new();
        page_doc
            .save_to(&mut buffer)
            .map_err(|e| PdfError::Parse(format!("PDF save error: {}", e)))?;

        tokio::fs::write(&output_path, &buffer).await?;
    }

    Ok(PartsOutput {
        file: PartsOutputFile {
            file_path: file_path_str,
            original_size,
            count: page_numbers.len() as u32,
            output_dir: output_dir.to_string_lossy().to_string(),
        },
    })
}

#[allow(dead_code)]
pub fn extract_pdf_pages_from_bytes(
    file_path: &str,
    bytes: &[u8],
    pages: &str,
    output_dir: &str,
) -> Result<PartsOutput, PdfError> {
    let output_dir_path = Path::new(output_dir);
    let (output, files) = plan_pdf_parts_from_bytes(file_path, bytes, pages, output_dir)?;
    std::fs::create_dir_all(output_dir_path).map_err(PdfError::Io)?;
    for (output_path, buffer) in files {
        std::fs::write(output_path, buffer).map_err(PdfError::Io)?;
    }
    Ok(output)
}
