//! Still-image conversion and codec measurements without filesystem or GUI dependencies.

mod jxl_header;
mod raster;

use std::{
    io::{BufRead, Seek},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use image::{DynamicImage, ImageFormat, RgbaImage};
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
const MAX_JPEG_XL_PIXELS: u64 = 80_000_000;
const MAX_SIDE: u32 = 16_383;
pub const DEFAULT_QUALITY: u8 = 90;
const CHEAP_MIN_QUALITY: u8 = 88;
const BLOCK_ERROR_PER_POINT: f64 = 0.4;
const RESIZE_SCALES: [u32; 11] = [95, 90, 85, 80, 70, 60, 50, 40, 30, 20, 10];

#[derive(Debug, Error)]
pub enum Error {
    #[error("Conversion cancelled")]
    Cancelled,
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
    #[error("Reversible JPEG to JPEG XL transcoding failed")]
    JxlTranscode(#[source] jpegxl_rs::EncodeError),
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

    fn lossless_setting(self) -> Setting {
        match self {
            Self::JpegXl => Setting::JxlLossless,
            Self::Webp => Setting::WebpLossless,
        }
    }

    fn compression_settings(self, detailed: bool) -> Vec<Setting> {
        let (values, more, setting): (_, _, fn(f32) -> Setting) = match self {
            Self::JpegXl => ([0.4, 0.75, 1.25, 2.0, 3.0], [4.0, 5.0], Setting::Jxl),
            Self::Webp => ([98.0, 94.0, 82.0, 70.0, 55.0], [45.0, 35.0], Setting::Webp),
        };
        let mut settings = Vec::with_capacity(if detailed { 13 } else { 5 });
        if detailed && self == Self::JpegXl {
            settings.extend([Setting::Jxl(0.15), Setting::Jxl(0.25)]);
        }
        for (index, value) in values.into_iter().enumerate() {
            if detailed && index > 0 {
                settings.push(setting(f32::midpoint(values[index - 1], value)));
            }
            settings.push(setting(value));
        }
        if detailed {
            settings.extend(more.map(setting));
        }
        settings
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptimizationMode {
    #[default]
    Best,
    Balance,
    Cheap,
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
    JxlReversible,
    Webp(f32),
    WebpLossless,
}

impl Setting {
    pub fn format(self) -> OutputFormat {
        match self {
            Self::Jxl(_) | Self::JxlLossless | Self::JxlReversible => OutputFormat::JpegXl,
            Self::Webp(_) | Self::WebpLossless => OutputFormat::Webp,
        }
    }

    pub fn is_lossless(self) -> bool {
        matches!(
            self,
            Self::JxlLossless | Self::JxlReversible | Self::WebpLossless
        )
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
        if self == Self::JxlReversible {
            let normalize = jxl_header::needs_color_conversion(bytes)?;
            return decode_jxl(bytes, runner, normalize);
        }
        match self.format() {
            OutputFormat::JpegXl => decode_jxl(bytes, runner, false),
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

/// Decode a still image for display, including JPEG XL results.
pub fn decode_for_comparison(bytes: &[u8]) -> Result<RgbaImage> {
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(Error::Limits);
    }
    if jpegxl_rs::utils::check_valid_signature(bytes) == Some(true) {
        let needs_color_conversion = jxl_header::needs_color_conversion(bytes)?;
        decode_jxl(bytes, &parallel_runner(), needs_color_conversion)
    } else {
        raster::decode(bytes)
    }
}

/// Optimize encoded input, including reversible JPEG-to-JPEG XL transcoding.
/// Returns the original display dimensions and the selected output, if any.
pub fn optimize_for_conversion(
    bytes: &[u8],
    options: Options,
    mode: OptimizationMode,
    cancelled: &AtomicBool,
) -> Result<((u32, u32), Option<Output>)> {
    check_cancelled(Some(cancelled))?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(Error::Limits);
    }
    let (source, reversible) = if options.format == OutputFormat::JpegXl
        && image::guess_format(bytes).is_ok_and(|format| format == ImageFormat::Jpeg)
    {
        let dimensions = raster::reversible_jpeg_dimensions(bytes)?;
        let encoded = encode_reversible_jpeg(bytes, dimensions);
        check_cancelled(Some(cancelled))?;
        let large = raster::check_dimensions(dimensions.0, dimensions.1).is_err();
        if large && mode == OptimizationMode::Best {
            return Ok((dimensions, Some(encoded?)));
        }
        let source = if large {
            match raster::decode_large_jpeg(bytes) {
                Ok(image) => Source(image),
                Err(error) => {
                    check_cancelled(Some(cancelled))?;
                    return encoded
                        .map(|output| (dimensions, Some(output)))
                        .or(Err(error));
                }
            }
        } else {
            Source::decode(bytes)?
        };
        (source, encoded.ok())
    } else {
        (Source::decode(bytes)?, None)
    };

    let output = source.optimize_inner(
        options,
        bytes.len(),
        mode,
        options.format == OutputFormat::JpegXl,
        Some(cancelled),
        reversible,
    )?;
    Ok((source.dimensions(), output))
}

pub struct Source(RgbaImage);

impl Source {
    /// Read the dimensions of a supported input without decoding its pixels.
    pub fn probe_dimensions(mut reader: impl BufRead + Seek) -> Option<(u32, u32)> {
        let format = imagesize::reader_type(&mut reader).ok()?;
        let size = format.reader_size(&mut reader).ok()?;
        let width = u32::try_from(size.width).ok()?;
        let height = u32::try_from(size.height).ok()?;
        let max_pixels = if format == imagesize::ImageType::Jpeg {
            MAX_JPEG_XL_PIXELS
        } else {
            MAX_PIXELS
        };
        raster::check_dimension_limits(width, height, max_pixels).ok()?;
        Some((width, height))
    }

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

    /// Find the most similar tested output that reduces the original file size.
    pub fn optimize(&self, options: Options, source_bytes: usize) -> Result<Option<Output>> {
        self.optimize_with_mode(options, source_bytes, OptimizationMode::Best)
    }

    /// Choose a smaller output using the requested quality and size trade-off.
    pub fn optimize_with_mode(
        &self,
        options: Options,
        source_bytes: usize,
        mode: OptimizationMode,
    ) -> Result<Option<Output>> {
        self.optimize_inner(options, source_bytes, mode, false, None, None)
    }

    /// Prefer an output smaller than the source. For JPEG XL, fall back to the smallest
    /// tested output that meets the mode's quality checks. Cancellation is cooperative.
    pub fn optimize_for_conversion(
        &self,
        options: Options,
        source_bytes: usize,
        mode: OptimizationMode,
        cancelled: &AtomicBool,
    ) -> Result<Option<Output>> {
        self.optimize_inner(
            options,
            source_bytes,
            mode,
            options.format == OutputFormat::JpegXl,
            Some(cancelled),
            None,
        )
    }

    fn optimize_inner(
        &self,
        options: Options,
        source_bytes: usize,
        mode: OptimizationMode,
        jpeg_xl_fallback: bool,
        cancelled: Option<&AtomicBool>,
        reversible: Option<Output>,
    ) -> Result<Option<Output>> {
        let detailed = mode != OptimizationMode::Best;
        let min_quality = match mode {
            OptimizationMode::Cheap => CHEAP_MIN_QUALITY,
            OptimizationMode::Best | OptimizationMode::Balance => DEFAULT_QUALITY,
        };
        let mut search = Search {
            min_quality: f64::from(min_quality),
            max_bytes: usize::MAX,
            detailed,
            exhaustive: false,
            cancelled,
        };
        search.check_cancelled()?;
        let runner = parallel_runner();
        let (width, height) = self.dimensions();
        let baseline = match reversible {
            Some(jpeg) if raster::check_dimensions(width, height).is_err() => jpeg,
            reversible => {
                let lossless = options.format.lossless_setting();
                let pixel_lossless = encode_output(&self.0, 100.0, lossless, &runner);
                match (pixel_lossless, reversible) {
                    (Ok(pixels), Some(jpeg)) if jpeg.bytes.len() <= pixels.bytes.len() => jpeg,
                    (Err(_), Some(jpeg)) => jpeg,
                    (pixels, _) => pixels?,
                }
            }
        };
        search.check_cancelled()?;
        search.max_bytes = output_budget(source_bytes, baseline.bytes.len());
        let fits = baseline.bytes.len() <= search.max_bytes;
        let smaller = if detailed {
            let mut candidates = Vec::new();
            if fits {
                candidates.push(Candidate::from(&baseline));
            }
            self.visit_candidates(options, search, &runner, |output| {
                candidates.push(Candidate::from(&output));
            })?;
            if let Some(candidate) = economical_candidate(&candidates, mode) {
                if candidate.setting == baseline.setting
                    && candidate.dimensions == self.dimensions()
                {
                    return Ok(Some(baseline));
                }
                search.check_cancelled()?;
                let (width, height) = candidate.dimensions;
                let image = raster::limit_size(&self.0, width.max(height));
                let output = encode_output(&image, candidate.quality, candidate.setting, &runner)?;
                (output.bytes.len() <= search.max_bytes).then_some(output)
            } else {
                None
            }
        } else {
            if fits {
                return Ok(Some(baseline));
            }
            let mut best: Option<Output> = None;
            self.visit_candidates(options, search, &runner, |candidate| {
                if best.as_ref().is_none_or(|current| {
                    candidate
                        .quality
                        .total_cmp(&current.quality)
                        .then_with(|| current.bytes.len().cmp(&candidate.bytes.len()))
                        .is_gt()
                }) {
                    best = Some(candidate);
                }
            })?;
            best
        };
        if smaller.is_some() || !jpeg_xl_fallback {
            return Ok(smaller);
        }

        let mut smallest = baseline;
        search.max_bytes = smallest.bytes.len();
        search.detailed = true;
        search.exhaustive = true;
        self.visit_candidates(options, search, &runner, |candidate| {
            if candidate.bytes.len() < smallest.bytes.len()
                || candidate.bytes.len() == smallest.bytes.len()
                    && candidate.quality > smallest.quality
            {
                smallest = candidate;
            }
        })?;
        Ok(Some(smallest))
    }

    /// Optimize for a minimum similarity score from 1 to 100.
    /// A score of 100 preserves the dimensions and encodes the prepared pixels losslessly.
    pub fn convert_with_quality(&self, options: Options, min_quality: u8) -> Result<Output> {
        if !(1..=100).contains(&min_quality) {
            return Err(Error::QualityRange(min_quality));
        }
        let runner = parallel_runner();
        let lossless = options.format.lossless_setting();
        let mut best = encode_output(&self.0, 100.0, lossless, &runner)?;
        if min_quality == 100 {
            return Ok(best);
        }

        self.visit_candidates(
            options,
            Search {
                min_quality: f64::from(min_quality),
                max_bytes: usize::MAX,
                detailed: false,
                exhaustive: false,
                cancelled: None,
            },
            &runner,
            |candidate| {
                if candidate.bytes.len() < best.bytes.len() {
                    best = candidate;
                }
            },
        )?;
        Ok(best)
    }

    fn visit_candidates(
        &self,
        options: Options,
        search: Search<'_>,
        runner: &Option<ThreadsRunner<'_>>,
        mut visit: impl FnMut(Output),
    ) -> Result<()> {
        search.check_cancelled()?;
        let lossless = options.format.lossless_setting();
        let lossy = options.format.compression_settings(search.detailed);
        let reference = raster::QualityReference::new(&self.0);
        probe_compression(&self.0, &reference, search, &lossy, runner, &mut visit)?;

        if options.auto_resize {
            let source_side = self.0.width().max(self.0.height());
            let mut previous_dimensions = self.0.dimensions();
            for scale in RESIZE_SCALES {
                search.check_cancelled()?;
                let image = raster::limit_size(&self.0, (source_side * scale / 100).max(1));
                if image.dimensions() == previous_dimensions {
                    continue;
                }
                let Some(quality) =
                    accepted_quality(reference.compare(&image)?, search.min_quality)
                else {
                    break;
                };
                previous_dimensions = image.dimensions();
                let candidate = encode_output(&image, quality, lossless, runner)?;
                if candidate.bytes.len() <= search.max_bytes {
                    visit(candidate);
                }
                probe_compression(&image, &reference, search, &lossy, runner, &mut visit)?;
            }
        }
        search.check_cancelled()
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

#[derive(Clone, Copy)]
struct Candidate {
    setting: Setting,
    dimensions: (u32, u32),
    quality: f64,
    bytes: usize,
}

impl From<&Output> for Candidate {
    fn from(output: &Output) -> Self {
        Self {
            setting: output.setting,
            dimensions: output.dimensions,
            quality: output.quality,
            bytes: output.bytes.len(),
        }
    }
}

fn economical_candidate(candidates: &[Candidate], mode: OptimizationMode) -> Option<Candidate> {
    let cheap = candidates.iter().min_by(|a, b| {
        a.bytes
            .cmp(&b.bytes)
            .then_with(|| b.quality.total_cmp(&a.quality))
    })?;
    if mode == OptimizationMode::Cheap {
        return Some(*cheap);
    }
    let highest = candidates.iter().max_by(|a, b| {
        a.quality
            .total_cmp(&b.quality)
            .then_with(|| b.bytes.cmp(&a.bytes))
    })?;
    if highest.quality <= cheap.quality || highest.bytes <= cheap.bytes {
        return Some(*cheap);
    }

    // Normalize the available trade-off per image, with equal weight on quality
    // and proportional size savings.
    let quality_span = highest.quality - cheap.quality;
    let size_span = (highest.bytes as f64 / cheap.bytes as f64).ln();
    let distance = |candidate: &Candidate| {
        let quality = (highest.quality - candidate.quality) / quality_span;
        let size = (candidate.bytes as f64 / cheap.bytes as f64).ln() / size_span;
        quality * quality + size * size
    };
    candidates
        .iter()
        .filter(|candidate| candidate.bytes <= highest.bytes && candidate.quality >= cheap.quality)
        .min_by(|a, b| {
            distance(a)
                .total_cmp(&distance(b))
                .then_with(|| b.quality.total_cmp(&a.quality))
                .then_with(|| a.bytes.cmp(&b.bytes))
        })
        .copied()
}

fn accepted_quality(similarity: Option<raster::Similarity>, min_quality: f64) -> Option<f64> {
    let max_block_error = (100.0 - min_quality) * BLOCK_ERROR_PER_POINT;
    similarity
        .filter(|value| value.score >= min_quality && value.max_block_error <= max_block_error)
        .map(|value| value.score)
}

fn encode_output(
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

fn output_budget(source_bytes: usize, lossless_bytes: usize) -> usize {
    source_bytes.saturating_sub(1).min(lossless_bytes)
}

#[derive(Clone, Copy)]
struct Search<'a> {
    min_quality: f64,
    max_bytes: usize,
    detailed: bool,
    exhaustive: bool,
    cancelled: Option<&'a AtomicBool>,
}

impl Search<'_> {
    fn check_cancelled(self) -> Result<()> {
        check_cancelled(self.cancelled)
    }
}

fn check_cancelled(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn probe_compression(
    image: &RgbaImage,
    reference: &raster::QualityReference<'_>,
    search: Search<'_>,
    lossy: &[Setting],
    runner: &Option<ThreadsRunner<'_>>,
    visit: &mut impl FnMut(Output),
) -> Result<()> {
    let dimensions = image.dimensions();
    if dimensions.0.min(dimensions.1) < 8 {
        return Ok(());
    }

    for &setting in lossy {
        search.check_cancelled()?;
        let bytes = setting.encode(image, runner)?;
        search.check_cancelled()?;
        if bytes.len() > search.max_bytes {
            continue;
        }
        let decoded = decode_candidate(&bytes, dimensions, setting, runner)?;
        search.check_cancelled()?;
        let quality = if raster::alpha_equal(image, &decoded) {
            accepted_quality(reference.compare(&decoded)?, search.min_quality)
        } else {
            None
        };
        if let Some(quality) = quality {
            visit(Output {
                bytes,
                dimensions,
                quality,
                setting,
            });
        } else if !search.exhaustive {
            break;
        }
    }
    Ok(())
}

fn decode_candidate(
    bytes: &[u8],
    dimensions: (u32, u32),
    setting: Setting,
    runner: &Option<ThreadsRunner<'_>>,
) -> Result<RgbaImage> {
    let decoded = setting.decode(bytes, runner)?;
    if decoded.dimensions() != dimensions {
        return Err(Error::CandidateDimensions);
    }
    Ok(decoded)
}

fn parallel_runner() -> Option<ThreadsRunner<'static>> {
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    ThreadsRunner::new(None, Some(workers))
}

fn encode_reversible_jpeg(bytes: &[u8], dimensions: (u32, u32)) -> Result<Output> {
    let runner = parallel_runner();
    let encoded = encoder_builder()
        .use_container(true)
        .uses_original_profile(true)
        .maybe_parallel_runner(runner.as_ref().map(|runner| runner as &dyn ParallelRunner))
        .build()
        .and_then(|mut encoder| encoder.encode_jpeg(bytes))
        .map_err(Error::JxlTranscode)?;
    Ok(Output {
        bytes: encoded,
        dimensions,
        quality: 100.0,
        setting: Setting::JxlReversible,
    })
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
    builder
        .build()?
        .encode_frame::<u8>(&frame, image.width(), image.height())
        .map_err(Error::JxlEncode)
}

fn encode_webp(image: &RgbaImage, quality: Option<f32>) -> Result<Vec<u8>> {
    use libwebp_sys::{
        WEBP_MAX_DIMENSION, WebPConfig, WebPEncode, WebPMemoryWrite, WebPMemoryWriter,
        WebPMemoryWriterClear, WebPMemoryWriterInit, WebPPicture, WebPPictureFree,
        WebPPictureImportRGBA,
    };
    use std::{mem::MaybeUninit, slice};

    let (width, height) = image.dimensions();
    if !(1..=WEBP_MAX_DIMENSION).contains(&width) || !(1..=WEBP_MAX_DIMENSION).contains(&height) {
        return Err(Error::WebpEncode);
    }
    let mut config = WebPConfig::new().map_err(|()| Error::WebpConfig)?;
    config.lossless = i32::from(quality.is_none());
    config.quality = quality.unwrap_or(75.0);
    config.exact = 1;
    let mut picture = WebPPicture::new().map_err(|()| Error::WebpEncode)?;
    picture.use_argb = 1;
    picture.width = width as i32;
    picture.height = height as i32;
    let mut writer = MaybeUninit::<WebPMemoryWriter>::uninit();

    // SAFETY: The RGBA image supplies width * height pixels with a checked stride. The
    // initialized writer lives through the encode, and both C allocations are released
    // after copying the result, including when import or encoding fails.
    unsafe {
        WebPMemoryWriterInit(writer.as_mut_ptr());
        picture.writer = Some(WebPMemoryWrite);
        picture.custom_ptr = writer.as_mut_ptr().cast();
        let encoded =
            WebPPictureImportRGBA(&mut picture, image.as_raw().as_ptr(), picture.width * 4) != 0
                && WebPEncode(&config, &mut picture) != 0;
        let mut writer = writer.assume_init();
        let result = if encoded && !writer.mem.is_null() && writer.size != 0 {
            Ok(slice::from_raw_parts(writer.mem, writer.size).to_vec())
        } else {
            Err(Error::WebpEncode)
        };
        WebPPictureFree(&mut picture);
        WebPMemoryWriterClear(&mut writer);
        result
    }
}

fn decode_jxl(
    bytes: &[u8],
    runner: &Option<ThreadsRunner<'_>>,
    normalize_profile: bool,
) -> Result<RgbaImage> {
    let builder = decoder_builder()
        .pixel_format(PixelFormat {
            num_channels: 4,
            ..Default::default()
        })
        .icc_profile(normalize_profile)
        .maybe_parallel_runner(runner.as_ref().map(|runner| runner as &dyn ParallelRunner));
    let (metadata, pixels) = builder.build()?.decode_with::<u8>(bytes)?;
    let decoded =
        RgbaImage::from_raw(metadata.width, metadata.height, pixels).ok_or(Error::JxlPixels)?;
    match metadata.icc_profile {
        Some(icc) => raster::to_srgb(DynamicImage::ImageRgba8(decoded), &icc),
        None => Ok(decoded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_are_available_from_a_png_header_without_image_data() {
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        header.extend_from_slice(&640_u32.to_be_bytes());
        header.extend_from_slice(&480_u32.to_be_bytes());
        assert_eq!(
            Source::probe_dimensions(std::io::Cursor::new(&header)),
            Some((640, 480))
        );
        assert!(Source::decode(&header).is_err());
    }

    fn sample() -> Source {
        Source(RgbaImage::from_fn(48, 32, |x, y| {
            image::Rgba([(x * 5) as u8, (y * 7) as u8, 40, (x * 3 + y) as u8])
        }))
    }

    #[test]
    fn balance_trades_similarity_for_space_and_cheap_picks_the_smallest() {
        let candidates =
            [(100.0, 1024), (92.0, 512), (80.0, 256), (81.0, 768)].map(|(quality, bytes)| {
                Candidate {
                    setting: Setting::Jxl(1.0),
                    dimensions: (96, 64),
                    quality,
                    bytes,
                }
            });
        let balance = economical_candidate(&candidates, OptimizationMode::Balance).unwrap();
        let cheap = economical_candidate(&candidates, OptimizationMode::Cheap).unwrap();
        assert_eq!((balance.quality, balance.bytes), (92.0, 512));
        assert_eq!((cheap.quality, cheap.bytes), (80.0, 256));
    }

    #[test]
    fn cheap_keeps_subtle_texture_while_saving_space() {
        let source = Source(RgbaImage::from_fn(256, 192, |x, y| {
            let grain = (((x * 13) ^ (y * 37) ^ (x * y)) % 6) as u8;
            image::Rgba([
                28 + (x / 4) as u8 + grain,
                45 + (y / 4) as u8 + grain,
                75 + ((x + y) / 5) as u8 + grain,
                255,
            ])
        }));
        let options = Options::default();
        let lossless = source.convert_with_quality(options, 100).unwrap();
        let output = source
            .optimize_with_mode(options, lossless.bytes.len() + 1, OptimizationMode::Cheap)
            .unwrap()
            .unwrap();

        assert!(output.quality >= f64::from(CHEAP_MIN_QUALITY));
        assert!(output.bytes.len() < lossless.bytes.len());
        assert_eq!(output.dimensions, source.dimensions());
        let local_error = raster::QualityReference::new(&source.0)
            .compare(&output.preview().unwrap())
            .unwrap()
            .unwrap()
            .max_block_error;
        assert!(local_error <= 4.8);
    }

    #[test]
    fn optimization_keeps_lossless_when_it_saves_space() {
        let source = sample();
        for format in OutputFormat::ALL {
            let options = Options {
                format,
                auto_resize: true,
            };
            let baseline = source.convert_with_quality(options, 100).unwrap();
            let input_bytes = baseline.bytes.len() + 1;
            let optimized = source.optimize(options, input_bytes).unwrap().unwrap();
            assert_eq!(optimized.quality, 100.0);
            assert_eq!(optimized.dimensions, source.dimensions());
            assert!(optimized.setting.is_lossless());
            assert_eq!(optimized.bytes.len(), baseline.bytes.len());
            assert!(source.optimize(options, 1).unwrap().is_none());
        }
    }

    #[test]
    fn jpeg_conversion_can_preserve_the_original_jpeg_and_orientation() {
        use image::{ImageEncoder, Rgb, RgbImage, codecs::jpeg::JpegEncoder};
        use jpegxl_rs::decode::Data;

        let pixels = RgbImage::from_fn(128, 96, |x, y| {
            Rgb([(x * 19 + y * 11) as u8, (y * 13) as u8, (x * y) as u8])
        });
        let mut input = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut input, 95);
        // Little-endian EXIF with orientation 6 (rotate 90 degrees clockwise).
        encoder
            .set_exif_metadata(
                b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec(),
            )
            .unwrap();
        encoder.encode_image(&pixels).unwrap();

        let (dimensions, output) = optimize_for_conversion(
            &input,
            Options::default(),
            OptimizationMode::Best,
            &AtomicBool::new(false),
        )
        .unwrap();
        let output = output.unwrap();
        assert_eq!(dimensions, (96, 128));
        assert_eq!(output.dimensions, dimensions);
        assert_eq!(output.quality, 100.0);
        assert!(output.bytes.len() < input.len());
        assert_eq!(output.preview().unwrap().dimensions(), dimensions);
        let (_, reconstructed) = decoder_builder()
            .build()
            .unwrap()
            .reconstruct(&output.bytes)
            .unwrap();
        assert!(matches!(reconstructed, Data::Jpeg(bytes) if bytes == input));
    }

    #[test]
    fn jpeg_xl_conversion_falls_back_to_the_smallest_acceptable_result() {
        let source = sample();
        let options = Options::default();
        let cancelled = AtomicBool::new(false);
        let fallback = source
            .optimize_for_conversion(options, 1, OptimizationMode::Best, &cancelled)
            .unwrap()
            .unwrap();
        let reference = raster::QualityReference::new(&source.0);
        let runner = parallel_runner();
        let lossless = Setting::JxlLossless.encode(&source.0, &runner).unwrap();
        let mut smallest = lossless.len();
        for setting in options.format.compression_settings(true) {
            let bytes = setting.encode(&source.0, &runner).unwrap();
            let decoded = setting.decode(&bytes, &runner).unwrap();
            if raster::alpha_equal(&source.0, &decoded)
                && accepted_quality(
                    reference.compare(&decoded).unwrap(),
                    f64::from(DEFAULT_QUALITY),
                )
                .is_some()
            {
                smallest = smallest.min(bytes.len());
            }
        }
        assert_eq!(fallback.bytes.len(), smallest);
        assert_eq!(fallback.setting.format(), OutputFormat::JpegXl);
        assert_eq!(fallback.dimensions, source.dimensions());
        assert!(fallback.quality >= f64::from(DEFAULT_QUALITY));
        assert!(
            source
                .optimize_for_conversion(
                    Options {
                        format: OutputFormat::Webp,
                        ..options
                    },
                    1,
                    OptimizationMode::Best,
                    &cancelled,
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cancelled_conversion_reports_interruption() {
        let result = sample().optimize_for_conversion(
            Options::default(),
            1,
            OptimizationMode::Best,
            &AtomicBool::new(true),
        );
        assert!(matches!(result, Err(Error::Cancelled)));
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
    fn comparison_decodes_lossless_jpeg_xl_results_at_their_original_resolution() {
        let source = sample();
        let encoded = Setting::JxlLossless.encode(&source.0, &None).unwrap();
        assert!(matches!(Source::decode(&encoded), Err(Error::Format)));
        let decoded = decode_for_comparison(&encoded).unwrap();
        assert_eq!(decoded.dimensions(), source.dimensions());
        assert!(decoded.as_raw() == source.0.as_raw());
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
