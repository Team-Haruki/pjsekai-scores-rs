//! Image decoding (PNG via `png`, JPEG via `zune-jpeg`) into premultiplied
//! tiny-skia pixmaps, and PNG/JPEG encoding of the rendered page.

use mtpng::encoder::{Encoder as MtpngEncoder, Options as MtpngOptions};
use mtpng::{ColorType as MtpngColorType, CompressionLevel, Header as MtpngHeader};
use tiny_skia::{IntSize, Pixmap, PixmapRef, PremultipliedColorU8};

/// Decodes PNG or JPEG bytes into a premultiplied RGBA pixmap, like
/// `SkImage::MakeFromEncoded` does for the formats the renderer needs.
pub(super) fn decode_image(bytes: &[u8]) -> Option<Pixmap> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png(bytes)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        decode_jpeg(bytes)
    } else {
        None
    }
}

fn decode_png(bytes: &[u8]) -> Option<Pixmap> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0_u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => expand(&buf, 3, |p| [p[0], p[1], p[2], 255]),
        png::ColorType::GrayscaleAlpha => expand(&buf, 2, |p| [p[0], p[0], p[0], p[1]]),
        png::ColorType::Grayscale => expand(&buf, 1, |p| [p[0], p[0], p[0], 255]),
        png::ColorType::Indexed => return None,
    };
    premultiplied_pixmap(rgba, info.width, info.height)
}

fn decode_jpeg(bytes: &[u8]) -> Option<Pixmap> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let pixels = decoder.decode().ok()?;
    let (width, height) = decoder.dimensions()?;
    // JPEG has no alpha, so the RGBA output is already premultiplied.
    Pixmap::from_vec(pixels, IntSize::from_wh(width as u32, height as u32)?)
}

fn expand(buf: &[u8], channels: usize, f: impl Fn(&[u8]) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() / channels * 4);
    for pixel in buf.chunks_exact(channels) {
        out.extend_from_slice(&f(pixel));
    }
    out
}

fn premultiplied_pixmap(mut rgba: Vec<u8>, width: u32, height: u32) -> Option<Pixmap> {
    for pixel in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let a = pixel[3];
        if a != 255 {
            for c in &mut pixel[..3] {
                *c = mul_div_255_round(*c, a);
            }
        }
    }
    Pixmap::from_vec(rgba, IntSize::from_wh(width, height)?)
}

/// `SkMulDiv255Round`, the rounding Skia uses when premultiplying decoded pixels.
fn mul_div_255_round(c: u8, a: u8) -> u8 {
    let prod = u32::from(c) * u32::from(a) + 128;
    ((prod + (prod >> 8)) >> 8) as u8
}

/// Unpremultiplied RGBA rows, as `Surface::read_pixels(.., AlphaType::Unpremul, ..)`
/// produces them for the Skia backend.
pub(super) fn unpremultiplied_rgba(pixmap: PixmapRef<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.pixels() {
        out.extend_from_slice(&demultiply(*pixel));
    }
    out
}

fn demultiply(pixel: PremultipliedColorU8) -> [u8; 4] {
    let a = pixel.alpha();
    match a {
        255 => [pixel.red(), pixel.green(), pixel.blue(), 255],
        0 => [0, 0, 0, 0],
        _ => {
            let un =
                |c: u8| ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
            [un(pixel.red()), un(pixel.green()), un(pixel.blue()), a]
        }
    }
}

pub(super) fn encode_png_mtpng(pixmap: PixmapRef<'_>) -> Option<Vec<u8>> {
    let pixels = unpremultiplied_rgba(pixmap);
    let mut header = MtpngHeader::new();
    header.set_size(pixmap.width(), pixmap.height()).ok()?;
    header.set_color(MtpngColorType::TruecolorAlpha, 8).ok()?;
    let mut options = MtpngOptions::new();
    options.set_compression_level(CompressionLevel::Fast).ok()?;
    let mut encoder = MtpngEncoder::new(Vec::new(), &options);
    encoder.write_header(&header).ok()?;
    encoder.write_image_rows(&pixels).ok()?;
    encoder.finish().ok()
}

/// Single-threaded `png` crate encoder; the counterpart of `PngEncoder::Skia`.
pub(super) fn encode_png_reference(pixmap: PixmapRef<'_>) -> Option<Vec<u8>> {
    let pixels = unpremultiplied_rgba(pixmap);
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, pixmap.width(), pixmap.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&pixels).ok()?;
    }
    Some(out)
}

/// Baseline JPEG with 4:2:0 chroma subsampling and optimized Huffman tables,
/// matching what Skia's libjpeg-turbo encoder writes (byte-for-byte the same size
/// as libjpeg-turbo with `optimize_coding` on the fixture). Like Skia's default
/// `AlphaOption::kIgnore`, the premultiplied colour is encoded and alpha dropped.
pub(super) fn encode_jpeg(pixmap: PixmapRef<'_>, quality: u8) -> Option<Vec<u8>> {
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

    let width = u16::try_from(pixmap.width()).ok()?;
    let height = u16::try_from(pixmap.height()).ok()?;
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, quality.clamp(1, 100));
    encoder.set_sampling_factor(SamplingFactor::F_2_2);
    encoder.set_optimized_huffman_tables(true);
    encoder
        .encode(pixmap.data(), width, height, ColorType::Rgba)
        .ok()?;
    Some(out)
}
