//! Image file reader implementation.
//!
//! Supports PNG, JPG, JPEG, GIF, and WebP formats.
//! Reads image once, encodes as base64, and optionally compresses if token budget exceeded.

use std::io::Cursor;
use std::path::Path;

use base64::Engine;
use image::GenericImageView;

use super::super::constants::{DEFAULT_IMAGE_MAX_TOKENS, IMAGE_BYTES_PER_TOKEN_ESTIMATE};
use super::super::output::{ImageDimensions, ImageOutput};

/// Media type mapping for supported image extensions.
fn media_type_for_extension(extension: &str) -> Option<&'static str> {
    match extension.to_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" => Some("image/jpeg"),
        "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Check if the extension is a supported image format.
#[allow(dead_code)]
pub fn is_supported_image_extension(extension: &str) -> bool {
    media_type_for_extension(extension).is_some()
}

/// Estimate token count for base64-encoded image data.
fn estimate_tokens_for_base64(base64_len: usize) -> usize {
    // Base64 expands data by ~4/3, but we count characters
    // Each token roughly represents 4 characters of base64
    (base64_len as f64 / IMAGE_BYTES_PER_TOKEN_ESTIMATE).ceil() as usize
}

/// Read an image file and encode as base64, with optional compression if token budget exceeded.
///
/// # Arguments
/// * `file_path` - Path to the image file
/// * `max_tokens` - Maximum tokens allowed (if None, uses DEFAULT_MAX_TOKENS)
///
/// # Returns
/// * `Ok(ImageOutput)` - The image data with base64 encoding and metadata
/// * `Err(std::io::Error)` - If file reading fails
pub fn read_image_file<P: AsRef<Path>>(
    file_path: P,
    max_tokens: Option<usize>,
) -> std::io::Result<ImageOutput> {
    let file_path = file_path.as_ref();
    let extension = file_path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing file extension")
        })?;

    let media_type = media_type_for_extension(extension).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Unsupported image extension: {}", extension),
        )
    })?;

    // Read file bytes - single read
    let image_bytes = std::fs::read(file_path)?;
    let original_size = image_bytes.len() as u64;

    // Decode image to get dimensions and format info
    let img = image::load_from_memory(&image_bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to decode image: {}", e),
        )
    })?;

    let (width, height) = img.dimensions();

    // Base64 encode the original bytes
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

    let max_tokens = max_tokens.unwrap_or(DEFAULT_IMAGE_MAX_TOKENS);
    let estimated_tokens = estimate_tokens_for_base64(base64_data.len());

    // If token budget exceeded, compress the image
    let final_base64 = if estimated_tokens > max_tokens {
        compress_image(&image_bytes, media_type, max_tokens)?
    } else {
        base64_data
    };

    Ok(ImageOutput {
        base64: final_base64,
        media_type: media_type.to_string(),
        original_size,
        dimensions: Some(ImageDimensions {
            original_width: width,
            original_height: height,
            display_width: None,
            display_height: None,
        }),
    })
}

/// Compress an image to fit within token budget.
///
/// Uses image crate to resize the image to a smaller size.
fn compress_image(
    image_bytes: &[u8],
    media_type: &str,
    max_tokens: usize,
) -> std::io::Result<String> {
    let img = image::load_from_memory(image_bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to decode image for compression: {}", e),
        )
    })?;

    let (width, height) = img.dimensions();

    // Calculate target dimensions to fit within token budget
    // Target tokens = max_tokens, each token ~4 chars of base64
    // So target base64 chars = max_tokens * 3 (rough estimate)
    // For JPEG, quality reduction is more effective than resizing
    let target_chars = max_tokens * 3;

    // Calculate scale factor needed
    let current_chars = estimate_tokens_for_base64(
        base64::engine::general_purpose::STANDARD
            .encode(image_bytes)
            .len(),
    );

    if current_chars <= target_chars {
        return Ok(base64::engine::general_purpose::STANDARD.encode(image_bytes));
    }

    // Scale down the image
    let scale = (target_chars as f64 / current_chars as f64).sqrt();
    let new_width = ((width as f64) * scale) as u32;
    let new_height = ((height as f64) * scale) as u32;

    // Ensure minimum dimensions
    let new_width = new_width.max(1);
    let new_height = new_height.max(1);

    let resized = img.resize_exact(new_width, new_height, image::imageops::FilterType::Lanczos3);

    // Encode back to the original format
    let mut buffer = Cursor::new(Vec::new());
    let format = image_format_from_media_type(media_type)?;

    resized.write_to(&mut buffer, format).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to encode compressed image: {}", e),
        )
    })?;

    Ok(base64::engine::general_purpose::STANDARD.encode(buffer.into_inner()))
}

/// Convert media type string to image::ImageFormat.
fn image_format_from_media_type(media_type: &str) -> std::io::Result<image::ImageFormat> {
    match media_type {
        "image/png" => Ok(image::ImageFormat::Png),
        "image/jpeg" => Ok(image::ImageFormat::Jpeg),
        "image/gif" => Ok(image::ImageFormat::Gif),
        "image/webp" => Ok(image::ImageFormat::WebP),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Unsupported media type for compression: {}", media_type),
        )),
    }
}
