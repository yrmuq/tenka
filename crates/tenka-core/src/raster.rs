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
    resize_to(image, (width_out, height_out))
}

fn resize_to(image: &RgbaImage, (width, height): (u32, u32)) -> Cow<'_, RgbaImage> {
    if image.dimensions() == (width, height) {
        return Cow::Borrowed(image);
    }

    let linear = linear_table();
    let premultiplied = Rgba32FImage::from_fn(image.width(), image.height(), |x, y| {
        Rgba(to_linear(*image.get_pixel(x, y), &linear))
    });
    let resized = resize(&premultiplied, width, height, FilterType::Lanczos3);
    Cow::Owned(RgbaImage::from_fn(width, height, |x, y| {
        from_linear(resized.get_pixel(x, y).0)
    }))
}

fn linear_table() -> [f32; 256] {
    std::array::from_fn(|v| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    })
}

fn to_linear(Rgba([r, g, b, a]): Rgba<u8>, linear: &[f32; 256]) -> [f32; 4] {
    let alpha = f32::from(a) / 255.0;
    [
        linear[usize::from(r)] * alpha,
        linear[usize::from(g)] * alpha,
        linear[usize::from(b)] * alpha,
        alpha,
    ]
}

fn from_linear([r, g, b, a]: [f32; 4]) -> Rgba<u8> {
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
}

pub fn alpha_equal(source: &RgbaImage, output: &RgbaImage) -> bool {
    source.dimensions() == output.dimensions()
        && source
            .pixels()
            .zip(output.pixels())
            .all(|(left, right)| left[3] == right[3])
}

pub struct Similarity {
    pub score: f64,
    pub max_block_error: f64,
}

impl Similarity {
    fn sample(source: &RgbaImage, candidate: &RgbaImage) -> Result<Self> {
        Ok(Self {
            score: score(source, candidate)?,
            max_block_error: block_error(source, candidate),
        })
    }

    fn merge(&mut self, other: Self) {
        self.score = self.score.min(other.score);
        self.max_block_error = self.max_block_error.max(other.max_block_error);
    }
}

pub struct QualityReference<'a> {
    original: &'a RgbaImage,
    overview: Cow<'a, RgbaImage>,
}

impl<'a> QualityReference<'a> {
    pub fn new(original: &'a RgbaImage) -> Self {
        Self {
            original,
            overview: limit_size(original, 1024),
        }
    }

    pub fn compare(&self, candidate: &RgbaImage) -> Result<Option<Similarity>> {
        if self.original == candidate {
            return Ok(Some(Similarity {
                score: 100.0,
                max_block_error: 0.0,
            }));
        }
        let (width, height) = self.original.dimensions();
        if width.min(height) < 8 || candidate.width().min(candidate.height()) < 8 {
            return Ok(None);
        }

        let mut lowest = None;
        if self.overview.width().min(self.overview.height()) >= 8 {
            let candidate_overview = resize_to(candidate, self.overview.dimensions());
            lowest = Some(Similarity::sample(&self.overview, &candidate_overview)?);
        }

        if width.max(height) > 1024 {
            // Native-scale samples catch detail lost when both overviews are downscaled.
            let sample_size = (width.min(192), height.min(192));
            for (x, y) in [(1, 1), (3, 1), (2, 2), (1, 3), (3, 3)] {
                let left = (width - sample_size.0) * x / 4;
                let top = (height - sample_size.1) * y / 4;
                let source =
                    crop_imm(self.original, left, top, sample_size.0, sample_size.1).to_image();
                let encoded = projected_crop(candidate, (width, height), (left, top), sample_size);
                let sample = Similarity::sample(&source, &encoded)?;
                match &mut lowest {
                    Some(similarity) => similarity.merge(sample),
                    None => lowest = Some(sample),
                }
            }
        }
        Ok(lowest)
    }
}

fn projected_crop(
    candidate: &RgbaImage,
    reference_size: (u32, u32),
    (left, top): (u32, u32),
    (width, height): (u32, u32),
) -> RgbaImage {
    if candidate.dimensions() == reference_size {
        return crop_imm(candidate, left, top, width, height).to_image();
    }

    // Reconstruct only the crop, keeping allocations independent of the full resolution.
    let (cols, rows) = candidate.dimensions();
    let x_scale = cols as f32 / reference_size.0 as f32;
    let y_scale = rows as f32 / reference_size.1 as f32;
    let pixel_position = |px: u32, scale: f32, limit: u32| {
        let position = ((px as f32 + 0.5) * scale - 0.5).clamp(0.0, (limit - 1) as f32);
        let first = position.floor() as u32;
        (first, (first + 1).min(limit - 1), position - first as f32)
    };
    let linear = linear_table();
    RgbaImage::from_fn(width, height, |x, y| {
        let (x0, x1, tx) = pixel_position(left + x, x_scale, cols);
        let (y0, y1, ty) = pixel_position(top + y, y_scale, rows);
        let (a, b, c, d) = (
            to_linear(*candidate.get_pixel(x0, y0), &linear),
            to_linear(*candidate.get_pixel(x1, y0), &linear),
            to_linear(*candidate.get_pixel(x0, y1), &linear),
            to_linear(*candidate.get_pixel(x1, y1), &linear),
        );
        from_linear(std::array::from_fn(|i| {
            let upper = a[i] + (b[i] - a[i]) * tx;
            let lower = c[i] + (d[i] - c[i]) * tx;
            upper + (lower - upper) * ty
        }))
    })
}

fn block_error(source: &RgbaImage, candidate: &RgbaImage) -> f64 {
    // Local differences expose fine contrast that a structure score can miss.
    const BLOCK: u32 = 32;
    let (width, height) = source.dimensions();
    let mut worst: f64 = 0.0;
    for top in (0..height).step_by(BLOCK as usize) {
        for left in (0..width).step_by(BLOCK as usize) {
            let right = (left + BLOCK).min(width);
            let bottom = (top + BLOCK).min(height);
            let mut error = 0_u32;
            for y in top..bottom {
                for x in left..right {
                    let a = source.get_pixel(x, y).0.map(u32::from);
                    let b = candidate.get_pixel(x, y).0.map(u32::from);
                    let mut peak = 0;
                    for channel in 0..3 {
                        let dark_a = a[channel] * a[3];
                        let dark_b = b[channel] * b[3];
                        let light_a = dark_a + (255 - a[3]) * 255;
                        let light_b = dark_b + (255 - b[3]) * 255;
                        peak = peak
                            .max(dark_a.abs_diff(dark_b))
                            .max(light_a.abs_diff(light_b));
                    }
                    error += peak;
                }
            }
            let count = (right - left) * (bottom - top);
            worst = worst.max(f64::from(error) / (255.0 * f64::from(count)));
        }
    }
    worst
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
        .map_err(|_| Error::QualitySample)
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
