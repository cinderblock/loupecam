//! Image file encoders. All return the encoded bytes.

use crate::{Image8, Image16};
use amscope_protocol::frame::{RawFrame, SampleFormat};

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("PNG: {0}")]
    Png(#[from] png::EncodingError),
    #[error("TIFF: {0}")]
    Tiff(#[from] tiff::TiffError),
    #[error("JPEG: {0}")]
    Jpeg(#[from] jpeg_encoder::EncodingError),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, EncodeError>;

/// Output file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Tiff,
    Jpeg { quality: u8 },
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Tiff => "tiff",
            Format::Jpeg { .. } => "jpg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Format::Png => "image/png",
            Format::Tiff => "image/tiff",
            Format::Jpeg { .. } => "image/jpeg",
        }
    }
}

fn png_color(channels: u8) -> Result<png::ColorType> {
    match channels {
        1 => Ok(png::ColorType::Grayscale),
        3 => Ok(png::ColorType::Rgb),
        n => Err(EncodeError::Unsupported(format!("{n} channels"))),
    }
}

pub fn png8(img: &Image8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, img.width, img.height);
    e.set_color(png_color(img.channels)?);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::Fast);
    let mut w = e.write_header()?;
    w.write_image_data(&img.data)?;
    w.finish()?;
    Ok(out)
}

pub fn png16(img: &Image16) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, img.width, img.height);
    e.set_color(png_color(img.channels)?);
    e.set_depth(png::BitDepth::Sixteen);
    e.set_compression(png::Compression::Fast);
    let mut w = e.write_header()?;
    let be: Vec<u8> = img.data.iter().flat_map(|v| v.to_be_bytes()).collect();
    w.write_image_data(&be)?;
    w.finish()?;
    Ok(out)
}

pub fn tiff8(img: &Image8) -> Result<Vec<u8>> {
    use tiff::encoder::{TiffEncoder, colortype};
    let mut cur = std::io::Cursor::new(Vec::new());
    let mut e = TiffEncoder::new(&mut cur)?;
    match img.channels {
        1 => e.write_image::<colortype::Gray8>(img.width, img.height, &img.data)?,
        3 => e.write_image::<colortype::RGB8>(img.width, img.height, &img.data)?,
        n => return Err(EncodeError::Unsupported(format!("{n} channels"))),
    }
    Ok(cur.into_inner())
}

pub fn tiff16(img: &Image16) -> Result<Vec<u8>> {
    use tiff::encoder::{TiffEncoder, colortype};
    let mut cur = std::io::Cursor::new(Vec::new());
    let mut e = TiffEncoder::new(&mut cur)?;
    match img.channels {
        1 => e.write_image::<colortype::Gray16>(img.width, img.height, &img.data)?,
        3 => e.write_image::<colortype::RGB16>(img.width, img.height, &img.data)?,
        n => return Err(EncodeError::Unsupported(format!("{n} channels"))),
    }
    Ok(cur.into_inner())
}

pub fn jpeg(img: &Image8, quality: u8) -> Result<Vec<u8>> {
    let (w, h) = (
        u16::try_from(img.width).map_err(|_| EncodeError::Unsupported("width > 65535".into()))?,
        u16::try_from(img.height).map_err(|_| EncodeError::Unsupported("height > 65535".into()))?,
    );
    let color = match img.channels {
        1 => jpeg_encoder::ColorType::Luma,
        3 => jpeg_encoder::ColorType::Rgb,
        n => return Err(EncodeError::Unsupported(format!("{n} channels"))),
    };
    let mut out = Vec::with_capacity(img.data.len() / 8);
    jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100)).encode(&img.data, w, h, color)?;
    Ok(out)
}

/// Encode an 8-bit image in `format`.
pub fn encode8(img: &Image8, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Png => png8(img),
        Format::Tiff => tiff8(img),
        Format::Jpeg { quality } => jpeg(img, quality),
    }
}

/// Encode a 16-bit image in `format` (JPEG is 8-bit only).
pub fn encode16(img: &Image16, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Png => png16(img),
        Format::Tiff => tiff16(img),
        Format::Jpeg { .. } => Err(EncodeError::Unsupported("16-bit JPEG".into())),
    }
}

/// The undeveloped mosaic as a single-channel 16-bit TIFF, samples scaled to the full
/// 16-bit range. For processing in external raw tools.
pub fn raw_tiff(frame: &RawFrame) -> Result<Vec<u8>> {
    let shift = crate::scale_shift(frame.format);
    let data: Vec<u16> = match frame.format {
        SampleFormat::U8 => frame.data.iter().map(|&v| (v as u16) << shift).collect(),
        SampleFormat::U16 { .. } => {
            frame.data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]]) << shift).collect()
        }
    };
    tiff16(&Image16 { width: frame.width, height: frame.height, channels: 1, data })
}
