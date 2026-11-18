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
const MAX_PIXELS: u64 = 36_000_000;
const MAX_SIDE: u32 = 16_383;
pub const DEFAULT_QUALITY: u8 = 90;
const BLOCK_ERROR_PER_POINT: f64 = 0.4;
const RESIZE_SCALES: [u32; 11] = [95, 90, 85, 80, 70, 60, 50, 40, 30, 20, 10];

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
    #[error("Quality {0} is outside the allowed range of 1 to 100")]
    QualityRange(u8),
    #[error("Image decoding failed")]
    Decode(#[source] image::ImageError),
    #[error("JPEG XL encoding failed")]
    JxlEncode(#[from] jpegxl_rs::EncodeError),
    #[error("JPEG XL decoding failed")]
    JxlDecode(#[from] jpegxl_rs::DecodeError),
    #[error("WebP encoding failed")]
    WebpEncode,
    #[error("Could not initialize the WebP encoder configuration")]
    WebpConfig,
    #[error("Could not decode the encoded WebP candidate")]
    WebpCandidateDecode(#[source] image::ImageError),
    #[error("The JPEG XL decoder returned an invalid pixel buffer")]
    JxlPixels,
    #[error("The quality metric received an invalid RGB sample")]
    QualitySample,
    #[error("The decoded candidate dimensions differ from the encoded dimensions")]
    CandidateDimensions,
    #[error("SSIMULACRA2 could not compare the image samples")]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    JpegXl,
    Webp,
}

impl OutputFormat {
    pub const ALL: [Self; 2] = [Self::JpegXl, Self::Webp];

    pub fn name(self) -> &'static str {
        match self {
            Self::JpegXl => "JPEG XL",
            Self::Webp => "WebP",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::JpegXl => "jxl",
            Self::Webp => "webp",
        }
    }

    fn settings(self) -> (Setting, [Setting; 5]) {
        match self {
            Self::JpegXl => (
                Setting::JxlLossless,
                [0.4, 0.75, 1.25, 2.0, 3.0].map(Setting::Jxl),
            ),
            Self::Webp => (
                Setting::WebpLossless,
                [98.0, 94.0, 82.0, 70.0, 55.0].map(Setting::Webp),
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub format: OutputFormat,
    /// Allow the search to try smaller resolutions.
    pub auto_resize: bool,
}

#[derive(Debug)]
pub struct Output {
    pub bytes: Vec<u8>,
    pub dimensions: (u32, u32),
    /// Similarity to the original image, including resolution changes.
    pub quality: f64,
    pub setting: Setting,
}

impl Output {
    pub fn preview(&self) -> Result<RgbaImage> {
        let decoded = self.setting.decode(&self.bytes, &parallel_runner())?;
        if decoded.dimensions() != self.dimensions {
            return Err(Error::CandidateDimensions);
        }
        Ok(raster::limit_size(&decoded, 480).into_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Setting {
    Jxl(f32),
    JxlLossless,
    Webp(f32),
    WebpLossless,
}

impl Setting {
    pub fn format(self) -> OutputFormat {
        match self {
            Self::Jxl(_) | Self::JxlLossless => OutputFormat::JpegXl,
            Self::Webp(_) | Self::WebpLossless => OutputFormat::Webp,
        }
    }

    pub fn is_lossless(self) -> bool {
        matches!(self, Self::JxlLossless | Self::WebpLossless)
    }

    fn encode(self, image: &RgbaImage, runner: &Option<ThreadsRunner<'_>>) -> Result<Vec<u8>> {
        match self {
            Self::Jxl(distance) if (0.0..=25.0).contains(&distance) && distance > 0.0 => {
                encode_jxl(image, Some(distance), runner)
            }
            Self::JxlLossless => encode_jxl(image, None, runner),
            Self::Webp(quality) if (0.0..=100.0).contains(&quality) => {
                encode_webp(image, Some(quality))
            }
            Self::WebpLossless => encode_webp(image, None),
            _ => Err(Error::Options),
        }
    }

    fn decode(self, bytes: &[u8], runner: &Option<ThreadsRunner<'_>>) -> Result<RgbaImage> {
        match self.format() {
            OutputFormat::JpegXl => decode_jxl(bytes, runner),
            OutputFormat::Webp => image::load_from_memory_with_format(bytes, ImageFormat::WebP)
                .map(|image| image.into_rgba8())
                .map_err(Error::WebpCandidateDecode),
        }
    }
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
        self.convert_with_quality(options, DEFAULT_QUALITY)
    }

    /// Optimize for a minimum similarity score from 1 to 100.
    /// A score of 100 preserves the dimensions and encodes the prepared pixels losslessly.
    pub fn convert_with_quality(&self, options: Options, min_quality: u8) -> Result<Output> {
        if !(1..=100).contains(&min_quality) {
            return Err(Error::QualityRange(min_quality));
        }
        let runner = parallel_runner();
        let (lossless, lossy) = options.format.settings();
        if min_quality == 100 {
            return lossless_output(&self.0, 100.0, lossless, &runner);
        }

        let min_quality = f64::from(min_quality);
        let reference = raster::QualityReference::new(&self.0);
        let mut best = smallest_at(
            &self.0,
            &reference,
            min_quality,
            100.0,
            lossless,
            &lossy,
            &runner,
        )?;

        if options.auto_resize {
            let source_side = self.0.width().max(self.0.height());
            let mut previous_dimensions = self.0.dimensions();
            for scale in RESIZE_SCALES {
                let image = raster::limit_size(&self.0, (source_side * scale / 100).max(1));
                if image.dimensions() == previous_dimensions {
                    continue;
                }
                let Some(quality) = accepted_quality(reference.compare(&image)?, min_quality)
                else {
                    break;
                };
                previous_dimensions = image.dimensions();
                let candidate = smallest_at(
                    &image,
                    &reference,
                    min_quality,
                    quality,
                    lossless,
                    &lossy,
                    &runner,
                )?;
                if candidate.bytes.len() < best.bytes.len() {
                    best = candidate;
                }
            }
        }
        Ok(best)
    }

    pub fn measure(&self, setting: Setting) -> Result<Measurement> {
        let image = &self.0;
        let runner = parallel_runner();
        let start = Instant::now();
        let bytes = setting.encode(image, &runner)?;
        let encode_time = start.elapsed();
        let decoded = setting.decode(&bytes, &runner)?;
        if decoded.dimensions() != image.dimensions() {
            return Err(Error::CandidateDimensions);
        }
        Ok(Measurement {
            setting,
            bytes: bytes.len(),
            quality: raster::QualityReference::new(image)
                .compare(&decoded)?
                .map(|value| value.score),
            encode_time,
            alpha_exact: raster::alpha_equal(image, &decoded),
            preview: raster::limit_size(&decoded, 480).into_owned(),
        })
    }
}

fn accepted_quality(similarity: Option<raster::Similarity>, min_quality: f64) -> Option<f64> {
    let max_block_error = (100.0 - min_quality) * BLOCK_ERROR_PER_POINT;
    similarity
        .filter(|value| value.score >= min_quality && value.max_block_error <= max_block_error)
        .map(|value| value.score)
}

fn lossless_output(
    image: &RgbaImage,
    quality: f64,
    setting: Setting,
    runner: &Option<ThreadsRunner<'_>>,
) -> Result<Output> {
    Ok(Output {
        bytes: setting.encode(image, runner)?,
        dimensions: image.dimensions(),
        quality,
        setting,
    })
}

fn smallest_at(
    image: &RgbaImage,
    reference: &raster::QualityReference<'_>,
    min_quality: f64,
    lossless_quality: f64,
    lossless: Setting,
    lossy: &[Setting],
    runner: &Option<ThreadsRunner<'_>>,
) -> Result<Output> {
    let mut best = lossless_output(image, lossless_quality, lossless, runner)?;
    let dimensions = best.dimensions;
    if dimensions.0.min(dimensions.1) < 8 {
        return Ok(best);
    }

    for &setting in lossy {
        let bytes = setting.encode(image, runner)?;
        let decoded = setting.decode(&bytes, runner)?;
        if decoded.dimensions() != dimensions {
            return Err(Error::CandidateDimensions);
        }
        if !raster::alpha_equal(image, &decoded) {
            break;
        }
        let Some(quality) = accepted_quality(reference.compare(&decoded)?, min_quality) else {
            break;
        };
        if bytes.len() < best.bytes.len() {
            best = Output {
                bytes,
                dimensions,
                quality,
                setting,
            };
        }
    }
    Ok(best)
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
    let mut config = webp::WebPConfig::new().map_err(|_| Error::WebpConfig)?;
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
    RgbaImage::from_raw(metadata.width, metadata.height, pixels).ok_or(Error::JxlPixels)
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
    fn conversion_uses_the_requested_codec_and_preserves_dimensions_by_default() {
        let source = sample();
        for format in OutputFormat::ALL {
            let output = source
                .convert(Options {
                    format,
                    ..Options::default()
                })
                .unwrap();
            let decoded = output.setting.decode(&output.bytes, &None).unwrap();
            assert_eq!(output.setting.format(), format);
            assert_eq!(decoded.dimensions(), source.dimensions());
            assert!(raster::alpha_equal(&source.0, &decoded));
            assert!(output.quality >= f64::from(DEFAULT_QUALITY));
        }
    }

    #[test]
    fn automatic_resize_preserves_fine_detail_and_can_shrink_flat_images() {
        let options = Options {
            format: OutputFormat::Webp,
            auto_resize: true,
        };
        let fine = Source(RgbaImage::from_fn(128, 96, |x, y| {
            let value = if (x + y) % 2 == 0 { 0 } else { 255 };
            image::Rgba([value, value, value, 255])
        }));
        let output = fine.convert(options).unwrap();
        assert_eq!(output.dimensions, fine.dimensions());
        assert!(output.quality >= f64::from(DEFAULT_QUALITY));

        let flat = Source(RgbaImage::from_fn(288, 192, |x, y| {
            image::Rgba([
                (12 + x * 130 / 288) as u8,
                (20 + y * 130 / 192) as u8,
                (65 + (x + y) * 115 / 480) as u8,
                255,
            ])
        }));
        let original_size = flat
            .convert(Options {
                auto_resize: false,
                ..options
            })
            .unwrap();
        let resized = flat.convert_with_quality(options, DEFAULT_QUALITY).unwrap();
        let strict = flat.convert_with_quality(options, 99).unwrap();
        assert!(resized.dimensions.0 < original_size.dimensions.0);
        assert!(resized.bytes.len() < original_size.bytes.len());
        assert!(resized.quality >= f64::from(DEFAULT_QUALITY));
        assert!(strict.quality >= 99.0);
        assert!(strict.dimensions.0 > resized.dimensions.0);
        assert!(strict.bytes.len() > resized.bytes.len());
    }

    #[test]
    fn full_quality_round_trips_pixels_and_disables_resizing_for_both_formats() {
        let source = sample();
        for format in OutputFormat::ALL {
            let options = Options {
                format,
                auto_resize: true,
            };
            let output = source.convert_with_quality(options, 100).unwrap();
            assert!(output.setting.is_lossless());
            assert_eq!(output.dimensions, source.dimensions());
            assert_eq!(output.quality, 100.0);
            assert_eq!(output.preview().unwrap(), source.0);
        }

        for quality in [0, 101] {
            assert!(matches!(
                source.convert_with_quality(Options::default(), quality),
                Err(Error::QualityRange(value)) if value == quality
            ));
        }
    }

    #[test]
    fn codec_and_resize_losses_share_one_original_reference() {
        let source = Source(RgbaImage::from_fn(240, 160, |x, y| {
            image::Rgba([(x * 19 + y * 11) as u8, (y * 13) as u8, (x * y) as u8, 255])
        }));
        let output = source
            .convert(Options {
                format: OutputFormat::Webp,
                auto_resize: true,
            })
            .unwrap();
        let decoded = output.setting.decode(&output.bytes, &None).unwrap();
        let checked = accepted_quality(
            raster::QualityReference::new(&source.0)
                .compare(&decoded)
                .unwrap(),
            f64::from(DEFAULT_QUALITY),
        )
        .expect("candidate must pass both similarity checks against the original");
        assert!(checked >= f64::from(DEFAULT_QUALITY));
        assert_eq!(checked, output.quality);
    }
}
