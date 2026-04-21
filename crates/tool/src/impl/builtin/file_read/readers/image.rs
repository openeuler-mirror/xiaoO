use std::io::Cursor;
use std::path::Path;

use base64::Engine;
use image::GenericImageView;

use super::super::constants::{DEFAULT_IMAGE_MAX_TOKENS, IMAGE_BYTES_PER_TOKEN_ESTIMATE};
use super::super::output::{ImageDimensions, ImageOutput};

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

#[allow(dead_code)]
pub fn is_supported_image_extension(extension: &str) -> bool {
    media_type_for_extension(extension).is_some()
}

fn estimate_tokens_for_base64(base64_len: usize) -> usize {
    (base64_len as f64 / IMAGE_BYTES_PER_TOKEN_ESTIMATE).ceil() as usize
}

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

    let image_bytes = std::fs::read(file_path)?;
    let original_size = image_bytes.len() as u64;

    let img = image::load_from_memory(&image_bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to decode image: {}", e),
        )
    })?;

    let (width, height) = img.dimensions();

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

    let max_tokens = max_tokens.unwrap_or(DEFAULT_IMAGE_MAX_TOKENS);
    let estimated_tokens = estimate_tokens_for_base64(base64_data.len());

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

#[allow(dead_code)]
pub fn read_image_from_bytes(
    extension: &str,
    bytes: &[u8],
    max_tokens: Option<usize>,
) -> std::io::Result<ImageOutput> {
    let media_type = media_type_for_extension(extension).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Unsupported image extension: {}", extension),
        )
    })?;

    let original_size = bytes.len() as u64;

    let img = image::load_from_memory(bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to decode image: {}", e),
        )
    })?;

    let (width, height) = img.dimensions();

    let base64_data = base64::engine::general_purpose::STANDARD.encode(bytes);

    let max_tokens = max_tokens.unwrap_or(DEFAULT_IMAGE_MAX_TOKENS);
    let estimated_tokens = estimate_tokens_for_base64(base64_data.len());

    let final_base64 = if estimated_tokens > max_tokens {
        compress_image(bytes, media_type, max_tokens)?
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

    let target_chars = max_tokens * 3;

    let current_chars = estimate_tokens_for_base64(
        base64::engine::general_purpose::STANDARD
            .encode(image_bytes)
            .len(),
    );

    if current_chars <= target_chars {
        return Ok(base64::engine::general_purpose::STANDARD.encode(image_bytes));
    }

    let scale = (target_chars as f64 / current_chars as f64).sqrt();
    let new_width = ((width as f64) * scale) as u32;
    let new_height = ((height as f64) * scale) as u32;

    let new_width = new_width.max(1);
    let new_height = new_height.max(1);

    let resized = img.resize_exact(new_width, new_height, image::imageops::FilterType::Lanczos3);

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
