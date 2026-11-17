use std::{borrow::Cow, io::Cursor};

use image::{
    DynamicImage, ImageDecoder, ImageFormat, Limits, Rgba, Rgba32FImage, RgbaImage,
    codecs::{jpeg::JpegDecoder, png::PngDecoder, webp::WebPDecoder},
    imageops::{FilterType, crop_imm, resize},
};
use moxcms::{ColorProfile, DataColorSpace, Layout};
use ssimulacra2::{ColorPrimaries, Rgb, TransferCharacteristic, compute_frame_ssimulacra2};

use crate::{Error, MAX_PIXELS, MAX_SIDE, Result};

pub fn decode(bytes: &[u8]) -> Result<RgbaImage> {
    let reader = Cursor::new(bytes);
    match image::guess_format(bytes) {
        Ok(ImageFormat::Jpeg) => decode_still(JpegDecoder::new(reader)?),
        Ok(ImageFormat::Png) => {
            let decoder = PngDecoder::with_limits(reader, limits())?;
            if decoder.is_apng()? {
                return Err(Error::Animation);
            }
            decode_still(decoder)
        }
        Ok(ImageFormat::WebP) => {
            let decoder = WebPDecoder::new(reader)?;
            if decoder.has_animation() {
                return Err(Error::Animation);
            }
            decode_still(decoder)
        }
        _ => Err(Error::Format),
    }
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_PIXELS * 12);
    limits
}

fn decode_still(mut decoder: impl ImageDecoder) -> Result<RgbaImage> {
    decoder.set_limits(limits())?;
    let (width, height) = decoder.dimensions();
    if width.max(height) > MAX_SIDE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(Error::Limits);
    }
    let color = decoder.color_type();
    if color.bits_per_pixel() / u16::from(color.channel_count()) != 8 {
        return Err(Error::BitDepth);
    }
    let orientation = decoder.orientation()?;
    let profile = decoder.icc_profile()?;
    let mut decoded = DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    match profile {
        Some(icc) => to_srgb(decoded, &icc),
        None => Ok(decoded.into_rgba8()),
    }
}

fn to_srgb(image: DynamicImage, icc: &[u8]) -> Result<RgbaImage> {
    let source = ColorProfile::new_from_slice(icc).map_err(|_| Error::Color)?;
    let target = ColorProfile::new_srgb();
    let (layout, width, height, data) = match source.color_space {
        DataColorSpace::Rgb => {
            let rgba = image.into_rgba8();
            (Layout::Rgba, rgba.width(), rgba.height(), rgba.into_raw())
        }
        DataColorSpace::Gray => {
            let la = image.into_luma_alpha8();
            (Layout::GrayAlpha, la.width(), la.height(), la.into_raw())
        }
        _ => return Err(Error::Color),
    };
    let transform = source
        .create_transform_8bit(layout, &target, Layout::Rgba, Default::default())
        .map_err(|_| Error::Color)?;
    let mut pixels = vec![0; width as usize * height as usize * 4];
    transform
        .transform(&data, &mut pixels)
        .map_err(|_| Error::Color)?;
    RgbaImage::from_raw(width, height, pixels).ok_or(Error::Color)
}

pub fn limit_size(image: &RgbaImage, longest: u32) -> Cow<'_, RgbaImage> {
    let (width, height) = image.dimensions();
    let side = width.max(height);
    if side <= longest {
        return Cow::Borrowed(image);
    }

    let width_out = ((width * longest + side / 2) / side).max(1);
    let height_out = ((height * longest + side / 2) / side).max(1);
    let mut premultiplied = Rgba32FImage::new(width, height);
    let linear: [f32; 256] = std::array::from_fn(|v| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    for (Rgba([r, g, b, a]), destination) in image.pixels().zip(premultiplied.pixels_mut()) {
        let alpha = f32::from(*a) / 255.0;
        *destination = Rgba([
            linear[usize::from(*r)] * alpha,
            linear[usize::from(*g)] * alpha,
            linear[usize::from(*b)] * alpha,
            alpha,
        ]);
    }
    let reduced = resize(&premultiplied, width_out, height_out, FilterType::Lanczos3);
    Cow::Owned(RgbaImage::from_fn(width_out, height_out, |x, y| {
        let [r, g, b, a] = reduced.get_pixel(x, y).0;
        let alpha = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
        if alpha == 0 {
            return Rgba([0, 0, 0, 0]);
        }
        let gamma = |value: f32| {
            let linear = (value / a).clamp(0.0, 1.0);
            let srgb = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            (srgb * 255.0).round() as u8
        };
        Rgba([gamma(r), gamma(g), gamma(b), alpha])
    }))
}

pub fn alpha_equal(source: &RgbaImage, output: &RgbaImage) -> bool {
    source.dimensions() == output.dimensions()
        && source
            .pixels()
            .zip(output.pixels())
            .all(|(left, right)| left[3] == right[3])
}

pub fn quality(source: &RgbaImage, output: &RgbaImage) -> Result<Option<f64>> {
    let (width, height) = source.dimensions();
    if source.dimensions() != output.dimensions() {
        return Err(Error::Measurement);
    }
    if source == output {
        return Ok(Some(100.0));
    }
    if width.min(height) < 8 {
        return Ok(None);
    }
    if width.max(height) <= 1024 {
        return score(source, output).map(Some);
    }

    // A full-frame sample plus a native-resolution crop keeps large-image memory bounded.
    let view_source = limit_size(source, 1024);
    let view_output = limit_size(output, 1024);
    let crop_width = width.min(512);
    let crop_height = height.min(512);
    let x = (width - crop_width) / 2;
    let y = (height - crop_height) / 2;
    let source_crop = crop_imm(source, x, y, crop_width, crop_height).to_image();
    let output_crop = crop_imm(output, x, y, crop_width, crop_height).to_image();
    let details = score(&source_crop, &output_crop)?;
    if view_source.width().min(view_source.height()) < 8 {
        return Ok(Some(details));
    }
    Ok(Some(details.min(score(&view_source, &view_output)?)))
}

fn score(source: &RgbaImage, output: &RgbaImage) -> Result<f64> {
    let rgb = |image: &RgbaImage, background: f32| {
        let pixels = image
            .pixels()
            .map(|&Rgba([r, g, b, a])| {
                let alpha = f32::from(a) / 255.0;
                [r, g, b].map(|c| (f32::from(c) / 255.0) * alpha + background * (1.0 - alpha))
            })
            .collect();
        Rgb::new(
            pixels,
            image.width() as usize,
            image.height() as usize,
            TransferCharacteristic::SRGB,
            ColorPrimaries::BT709,
        )
        .map_err(|_| Error::Measurement)
    };
    let mut rating = compute_frame_ssimulacra2(rgb(source, 0.0)?, rgb(output, 0.0)?)
        .map_err(|_| Error::Measurement)?;
    if source.pixels().chain(output.pixels()).any(|p| p[3] != 255) {
        let light = compute_frame_ssimulacra2(rgb(source, 1.0)?, rgb(output, 1.0)?)
            .map_err(|_| Error::Measurement)?;
        rating = rating.min(light);
    }
    Ok(rating)
}
