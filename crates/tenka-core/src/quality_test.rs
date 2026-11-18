use std::{
    cell::{LazyCell, OnceCell},
    collections::{HashMap, hash_map::Entry},
};

use image::RgbaImage;
use jpegxl_rs::ThreadsRunner;

use crate::{
    Error, Options, Output, OutputFormat, RESIZE_SCALES, Result, Setting, lossless_output,
    parallel_runner, raster, round_trip,
};

/// Measures lossy test encodings and reuses their scores within a comparison series.
pub struct QualityTest<'a> {
    original: &'a RgbaImage,
    options: Options,
    runner: Option<ThreadsRunner<'static>>,
    reference: OnceCell<raster::QualityReference<'a>>,
    probes: HashMap<(u32, u8), Option<Probe>>,
}

#[derive(Clone, Copy)]
struct Probe {
    quality: f64,
    bytes: usize,
}

impl<'a> QualityTest<'a> {
    pub(crate) fn new(original: &'a RgbaImage, options: Options) -> Self {
        Self {
            original,
            options,
            runner: parallel_runner(),
            reference: OnceCell::new(),
            probes: HashMap::new(),
        }
    }

    /// Select the tested result with the highest similarity at or below the ceiling.
    /// At 100%, encode the prepared pixels losslessly at the original resolution.
    pub fn encode(&mut self, ceiling: u8) -> Result<Output> {
        if !(1..=100).contains(&ceiling) {
            return Err(Error::QualityRange(ceiling));
        }
        let format = self.options.format;
        let (lossless, _) = format.settings();
        if ceiling == 100 {
            return lossless_output(self.original, 100.0, lossless, &self.runner);
        }
        if self.original.width().min(self.original.height()) < 8 {
            return Err(Error::QualityCeiling(ceiling));
        }

        let cap = f64::from(ceiling);
        let original_side = self.original.width().max(self.original.height());
        let sides = std::iter::once(original_side).chain(
            RESIZE_SCALES
                .into_iter()
                .filter(|_| self.options.auto_resize)
                .map(|scale| (original_side * scale / 100).max(1)),
        );
        let reference = self
            .reference
            .get_or_init(|| raster::QualityReference::new(self.original));
        let mut last_side = 0;
        for side in sides {
            if side == last_side {
                continue;
            }
            last_side = side;
            let image = LazyCell::new(|| raster::limit_size(self.original, side));
            search_strengths(|strength| {
                let result = match self.probes.entry((side, strength)) {
                    Entry::Occupied(entry) => *entry.get(),
                    Entry::Vacant(entry) => {
                        let setting = lossy_setting(format, strength);
                        let result = measure(&image, reference, setting, &self.runner)?;
                        *entry.insert(result)
                    }
                };
                Ok(result.is_some_and(|probe| probe.quality <= cap))
            })?;
        }

        let (&(side, strength), probe) = self
            .probes
            .iter()
            .filter_map(|(key, probe)| probe.as_ref().map(|probe| (key, probe)))
            .filter(|(_, probe)| probe.quality <= cap)
            .max_by(|(key, a), (other_key, b)| {
                a.quality
                    .total_cmp(&b.quality)
                    .then_with(|| b.bytes.cmp(&a.bytes))
                    .then_with(|| key.cmp(other_key))
            })
            .ok_or(Error::QualityCeiling(ceiling))?;
        let image = raster::limit_size(self.original, side);
        let setting = lossy_setting(format, strength);
        Ok(Output {
            bytes: setting.encode(&image, &self.runner)?,
            dimensions: image.dimensions(),
            quality: probe.quality,
            setting,
        })
    }
}

fn lossy_setting(format: OutputFormat, strength: u8) -> Setting {
    match format {
        OutputFormat::JpegXl => Setting::Jxl(0.05 + f32::from(strength) * (24.95 / 100.0)),
        OutputFormat::Webp => Setting::Webp(100.0 - f32::from(strength)),
    }
}

fn search_strengths(mut fits: impl FnMut(u8) -> Result<bool>) -> Result<()> {
    let mut low = 0;
    if fits(low)? {
        return Ok(());
    }
    let mut high = 100;
    if !fits(high)? {
        high = 50;
        if !fits(high)? {
            return Ok(());
        }
    }

    // Similarity usually decreases as compression increases. Keep every measured
    // candidate so selection still compares actual scores across settings and resolutions.
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        if fits(mid)? {
            high = mid;
        } else {
            low = mid;
        }
    }
    Ok(())
}

fn measure(
    image: &RgbaImage,
    reference: &raster::QualityReference<'_>,
    setting: Setting,
    runner: &Option<ThreadsRunner<'_>>,
) -> Result<Option<Probe>> {
    if image.width().min(image.height()) < 8 {
        return Ok(None);
    }
    let (bytes, decoded) = round_trip(image, setting, runner)?;
    if !raster::alpha_equal(image, &decoded) {
        return Ok(None);
    }
    Ok(reference
        .compare(&decoded)?
        .filter(|score| score.score.is_finite())
        .map(|score| Probe {
            quality: score.score.max(0.0),
            bytes: bytes.len(),
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Source;
    use image::Rgba;

    #[test]
    fn test_batches_pick_the_closest_measured_result_below_the_ceiling() {
        let source = Source(RgbaImage::from_fn(96, 64, |x, y| {
            let block = if (x / 6 + y / 5) % 2 == 0 { 50 } else { 0 };
            Rgba([(x * 2 + block) as u8, (y * 3) as u8, (x + y) as u8, 255])
        }));
        for format in OutputFormat::ALL {
            let options = Options {
                format,
                auto_resize: false,
            };
            let mut test = source.quality_test(options);
            for target in [60, 90] {
                let output = test.encode(target).unwrap();
                assert_eq!(output.setting.format(), format);
                assert!(!output.setting.is_lossless());
                assert_eq!(output.dimensions, source.dimensions());
                let actual = source.measure(output.setting).unwrap().quality.unwrap();
                assert_eq!(output.quality, actual);
                assert!(
                    actual <= f64::from(target),
                    "{format:?} at {target}: {actual}"
                );
                assert!(
                    test.probes
                        .values()
                        .flatten()
                        .all(|probe| probe.quality > f64::from(target) || probe.quality <= actual)
                );
            }
            let output = test.encode(100).unwrap();
            assert!(output.setting.is_lossless());
            assert_eq!(output.quality, 100.0);
            assert_eq!(output.preview().unwrap(), source.0);
        }
    }
}
