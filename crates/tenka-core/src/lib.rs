//! Still-image conversion and codec measurements without filesystem or GUI dependencies.

mod raster;

use std::time::{Duration, Instant};

use image::{ImageFormat, RgbaImage};
use jpegxl_rs::{
    ThreadsRunner,
    decode::PixelFormat,
    decoder_builder,
    encode::{EncoderFrame, EncoderSpeed},
    encoder_builder,
    parallel::ParallelRunner,
};
use thiserror::Error;

pub const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PIXELS: u64 = 24_000_000;
const MAX_SIDE: u32 = 16_383;
const JXL_DISTANCES: [f32; 4] = [3.0, 2.0, 1.25, 0.75];

#[derive(Debug, Error)]
pub enum Error {
    #[error("Only JPEG, PNG and WebP input is supported")]
    Format,
    #[error("Animated images are unsupported")]
    Animation,
    #[error("Only 8-bit input is supported")]
    BitDepth,
    #[error("The image exceeds the input limits")]
    Limits,
    #[error("The source color profile cannot be converted to sRGB")]
    Color,
    #[error("Invalid conversion settings")]
    Options,
    #[error("Image decoding failed")]
    Decode(#[source] image::ImageError),
    #[error("JPEG XL encoding failed")]
    JxlEncode(#[from] jpegxl_rs::EncodeError),
    #[error("JPEG XL decoding failed")]
    JxlDecode(#[from] jpegxl_rs::DecodeError),
    #[error("WebP encoding failed")]
    WebpEncode,
    #[error("Quality measurement failed")]
    Measurement,
}

impl From<image::ImageError> for Error {
    fn from(error: image::ImageError) -> Self {
        match error {
            image::ImageError::Limits(_) => Self::Limits,
            _ => Self::Decode(error),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Minimum estimated SSIMULACRA2 score for lossy candidates.
    pub min_score: f64,
    /// Longest permitted edge in pixels. Images are never enlarged.
    pub max_side: Option<u32>,
    /// Target size of the encoded file, in bytes.
    pub target_bytes: Option<usize>,
    /// Try smaller dimensions when no candidate reaches the size target.
    pub resize_for_target: bool,
    /// Encode the prepared pixels without loss.
    pub lossless: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            min_score: 80.0,
            max_side: None,
            target_bytes: None,
            resize_for_target: false,
            lossless: false,
        }
    }
}

#[derive(Debug)]
pub struct Output {
    pub bytes: Vec<u8>,
    pub dimensions: (u32, u32),
    pub quality: f64,
    /// JPEG XL distance used for lossy output.
    pub distance: Option<f32>,
    pub meets_target: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Setting {
    Jxl(f32),
    JxlLossless,
    Webp(f32),
    WebpLossless,
}

pub const BENCHMARK_SETTINGS: [Setting; 10] = [
    Setting::Jxl(3.0),
    Setting::Jxl(2.0),
    Setting::Jxl(1.25),
    Setting::Jxl(0.75),
    Setting::JxlLossless,
    Setting::Webp(55.0),
    Setting::Webp(70.0),
    Setting::Webp(82.0),
    Setting::Webp(94.0),
    Setting::WebpLossless,
];

pub struct Measurement {
    pub setting: Setting,
    pub bytes: usize,
    pub quality: Option<f64>,
    pub encode_time: Duration,
    pub alpha_exact: bool,
    pub preview: RgbaImage,
}

pub struct Source(RgbaImage);

impl Source {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_INPUT_BYTES {
            return Err(Error::Limits);
        }
        raster::decode(bytes).map(Self)
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.0.dimensions()
    }

    pub fn preview(&self) -> RgbaImage {
        raster::limit_size(&self.0, 480).into_owned()
    }

    pub fn convert(&self, options: Options) -> Result<Output> {
        if !(-100.0..=100.0).contains(&options.min_score)
            || options.max_side == Some(0)
            || options.target_bytes == Some(0)
        {
            return Err(Error::Options);
        }

        let source_side = self.0.width().max(self.0.height());
        let start = source_side.min(options.max_side.unwrap_or(source_side));
        let mut sizes = vec![start];
        if options.resize_for_target && options.target_bytes.is_some() {
            let floor = 256.min(start);
            for scale in [85, 70, 55, 40, 25, 15, 8, 0] {
                let side = (start * scale / 100).max(floor);
                if !sizes.contains(&side) {
                    sizes.push(side);
                }
            }
        }

        let runner = parallel_runner();
        let mut fallback: Option<Output> = None;
        for side in sizes {
            let image = raster::limit_size(&self.0, side);
            let mut smallest: Option<Output> = None;
            if !options.lossless && image.width().min(image.height()) >= 8 {
                for distance in JXL_DISTANCES {
                    let bytes = encode_jxl(&image, Some(distance), &runner)?;
                    if smallest
                        .as_ref()
                        .is_some_and(|best| best.bytes.len() <= bytes.len())
                    {
                        continue;
                    }
                    let decoded = decode_jxl(&bytes, &runner)?;
                    if !raster::alpha_equal(&image, &decoded) {
                        continue;
                    }
                    let quality = raster::quality(&image, &decoded)?.ok_or(Error::Measurement)?;
                    if quality >= options.min_score {
                        smallest = Some(output(
                            &image,
                            bytes,
                            quality,
                            Some(distance),
                            options.target_bytes,
                        ));
                    }
                }
            }

            let lossless = encode_jxl(&image, None, &runner)?;
            if smallest
                .as_ref()
                .is_none_or(|best| lossless.len() < best.bytes.len())
            {
                smallest = Some(output(&image, lossless, 100.0, None, options.target_bytes));
            }

            let best = smallest.ok_or(Error::Measurement)?;
            if best.meets_target {
                return Ok(best);
            }
            if fallback
                .as_ref()
                .is_none_or(|other| best.bytes.len() < other.bytes.len())
            {
                fallback = Some(best);
            }
        }
        fallback.ok_or(Error::Measurement)
    }

    pub fn measure(&self, setting: Setting) -> Result<Measurement> {
        let image = &self.0;
        let runner = parallel_runner();
        let start = Instant::now();
        let bytes = match setting {
            Setting::Jxl(distance) if (0.0..=25.0).contains(&distance) && distance > 0.0 => {
                encode_jxl(image, Some(distance), &runner)?
            }
            Setting::JxlLossless => encode_jxl(image, None, &runner)?,
            Setting::Webp(quality) if (0.0..=100.0).contains(&quality) => {
                encode_webp(image, Some(quality))?
            }
            Setting::WebpLossless => encode_webp(image, None)?,
            _ => return Err(Error::Options),
        };
        let encode_time = start.elapsed();
        let decoded = match setting {
            Setting::Jxl(_) | Setting::JxlLossless => decode_jxl(&bytes, &runner)?,
            Setting::Webp(_) | Setting::WebpLossless => {
                image::load_from_memory_with_format(&bytes, ImageFormat::WebP)?.into_rgba8()
            }
        };
        Ok(Measurement {
            setting,
            bytes: bytes.len(),
            quality: raster::quality(image, &decoded)?,
            encode_time,
            alpha_exact: raster::alpha_equal(image, &decoded),
            preview: raster::limit_size(&decoded, 480).into_owned(),
        })
    }
}

fn output(
    image: &RgbaImage,
    bytes: Vec<u8>,
    quality: f64,
    distance: Option<f32>,
    target: Option<usize>,
) -> Output {
    Output {
        dimensions: image.dimensions(),
        meets_target: target.is_none_or(|limit| bytes.len() <= limit),
        bytes,
        quality,
        distance,
    }
}

fn parallel_runner() -> Option<ThreadsRunner<'static>> {
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    ThreadsRunner::new(None, Some(workers))
}

fn encode_jxl(
    image: &RgbaImage,
    distance: Option<f32>,
    runner: &Option<ThreadsRunner<'_>>,
) -> Result<Vec<u8>> {
    let lossless = distance.is_none();
    let builder = encoder_builder()
        .has_alpha(true)
        .lossless(lossless)
        .uses_original_profile(lossless)
        .quality(distance.unwrap_or(0.0))
        .speed(if lossless {
            EncoderSpeed::Wombat
        } else {
            EncoderSpeed::Hare
        })
        .maybe_parallel_runner(runner.as_ref().map(|runner| runner as &dyn ParallelRunner));
    let frame = EncoderFrame::new(image.as_raw()).num_channels(4);
    let encoded = builder
        .build()?
        .encode_frame::<u8, u8>(&frame, image.width(), image.height())?;
    Ok(encoded.data)
}

fn encode_webp(image: &RgbaImage, quality: Option<f32>) -> Result<Vec<u8>> {
    let mut config = webp::WebPConfig::new().map_err(|_| Error::WebpEncode)?;
    config.lossless = i32::from(quality.is_none());
    config.quality = quality.unwrap_or(75.0);
    config.exact = 1;
    let bytes = webp::Encoder::from_rgba(image.as_raw(), image.width(), image.height())
        .encode_advanced(&config)
        .map_err(|_| Error::WebpEncode)?;
    Ok(bytes.to_vec())
}

fn decode_jxl(bytes: &[u8], runner: &Option<ThreadsRunner<'_>>) -> Result<RgbaImage> {
    let builder = decoder_builder()
        .pixel_format(PixelFormat {
            num_channels: 4,
            ..Default::default()
        })
        .maybe_parallel_runner(runner.as_ref().map(|runner| runner as &dyn ParallelRunner));
    let (metadata, pixels) = builder.build()?.decode_with::<u8>(bytes)?;
    RgbaImage::from_raw(metadata.width, metadata.height, pixels).ok_or(Error::Measurement)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Source {
        Source(RgbaImage::from_fn(48, 32, |x, y| {
            image::Rgba([(x * 5) as u8, (y * 7) as u8, 40, (x * 3 + y) as u8])
        }))
    }

    #[test]
    fn lossless_jxl_round_trips_transparency_and_pixels() {
        let source = sample();
        let output = source
            .convert(Options {
                lossless: true,
                ..Options::default()
            })
            .unwrap();
        assert_eq!(decode_jxl(&output.bytes, &None).unwrap(), source.0);
    }

    #[test]
    fn measured_lossless_codecs_report_exact_pixels_and_transparency() {
        let source = sample();
        for setting in [Setting::JxlLossless, Setting::WebpLossless] {
            let measured = source.measure(setting).unwrap();
            assert!(measured.alpha_exact);
            assert_eq!(measured.quality, Some(100.0));
            assert_eq!(measured.preview, source.0);
        }
    }

    #[test]
    fn auto_reports_an_unreachable_limit_and_keeps_the_allowed_dimensions() {
        let output = sample()
            .convert(Options {
                max_side: Some(24),
                target_bytes: Some(1),
                ..Options::default()
            })
            .unwrap();
        assert_eq!(output.dimensions, (24, 16));
        assert!(!output.meets_target);
        assert!(output.quality >= Options::default().min_score);
    }

    #[test]
    fn target_can_reduce_resolution_after_preserving_the_larger_image_first() {
        let source = Source(RgbaImage::from_fn(600, 400, |x, y| {
            image::Rgba([(x * 19 + y * 11) as u8, (y * 13) as u8, (x * y) as u8, 255])
        }));
        let baseline = source
            .convert(Options {
                lossless: true,
                ..Options::default()
            })
            .unwrap();
        let resized = source
            .convert(Options {
                target_bytes: Some(baseline.bytes.len() - 1),
                resize_for_target: true,
                lossless: true,
                ..Options::default()
            })
            .unwrap();
        assert!(resized.meets_target);
        assert!(resized.dimensions.0 < baseline.dimensions.0);
    }
}
