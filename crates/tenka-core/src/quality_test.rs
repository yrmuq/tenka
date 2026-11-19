use std::{
    cell::{LazyCell, OnceCell},
    cmp::Reverse,
    collections::{HashMap, hash_map::Entry},
};

use image::RgbaImage;
use jpegxl_rs::ThreadsRunner;

use crate::{
    Error, Options, Output, OutputFormat, RESIZE_SCALES, Result, Setting, lossless_output,
    output_budget, parallel_runner, raster, round_trip,
};

/// Measures test encodings and reuses their scores within a comparison series.
pub struct QualityTest<'a> {
    original: &'a RgbaImage,
    options: Options,
    runner: Option<ThreadsRunner<'static>>,
    reference: OnceCell<raster::QualityReference<'a>>,
    probes: HashMap<ProbeKey, Option<Probe>>,
    source_bytes: Option<usize>,
    lossless_bytes: Option<usize>,
}

type ProbeKey = (u32, Option<u8>);

#[derive(Clone, Copy)]
struct Probe {
    quality: f64,
    bytes: Option<usize>,
}

impl<'a> QualityTest<'a> {
    pub(crate) fn new(
        original: &'a RgbaImage,
        options: Options,
        source_bytes: Option<usize>,
    ) -> Self {
        Self {
            original,
            options,
            runner: parallel_runner(),
            reference: OnceCell::new(),
            probes: HashMap::new(),
            source_bytes,
            lossless_bytes: None,
        }
    }

    /// Select the closest measured result at or below the ceiling, using size
    /// to break ties at the displayed precision.
    /// At 100%, encode the prepared pixels losslessly at the original resolution.
    pub fn encode(&mut self, ceiling: u8) -> Result<Output> {
        if !(1..=100).contains(&ceiling) {
            return Err(Error::QualityRange(ceiling));
        }
        let format = self.options.format;
        let (lossless, _) = format.settings();
        if ceiling == 100 {
            let output = lossless_output(self.original, 100.0, lossless, &self.runner)?;
            self.lossless_bytes = Some(output.bytes.len());
            if self
                .source_bytes
                .is_some_and(|source| output.bytes.len() >= source)
            {
                return Err(Error::TestSizeLimit);
            }
            return Ok(output);
        }
        if self.original.width().min(self.original.height()) < 8 {
            return Err(Error::QualityCeiling(ceiling));
        }

        let max_bytes = match self.source_bytes {
            Some(source) => {
                let bytes = match self.lossless_bytes {
                    Some(bytes) => bytes,
                    None => {
                        let bytes = lossless.encode(self.original, &self.runner)?.len();
                        self.lossless_bytes = Some(bytes);
                        bytes
                    }
                };
                output_budget(source, bytes)
            }
            None => usize::MAX,
        };
        let cap = f64::from(ceiling);
        let original_side = self.original.width().max(self.original.height());
        let mut sides = vec![original_side];
        if self.options.auto_resize {
            sides.extend(
                RESIZE_SCALES
                    .into_iter()
                    .map(|scale| (original_side * scale / 100).max(1)),
            );
            sides.dedup();
        }
        for &side in &sides {
            self.probe_side(side, original_side, cap, max_bytes)?;
        }
        for _ in 0..2 {
            let intermediate = self.intermediate_sides(&sides, cap, max_bytes);
            if intermediate.is_empty() {
                break;
            }
            for &side in &intermediate {
                self.probe_side(side, original_side, cap, max_bytes)?;
            }
            sides.extend(intermediate);
            sides.sort_unstable();
            sides.reverse();
        }

        self.encode_best(ceiling, lossless, max_bytes)
    }

    fn probe_side(
        &mut self,
        side: u32,
        original_side: u32,
        cap: f64,
        max_bytes: usize,
    ) -> Result<()> {
        let image = LazyCell::new(|| raster::limit_size(self.original, side));
        let reference = self
            .reference
            .get_or_init(|| raster::QualityReference::new(self.original));
        if side < original_side
            && let Entry::Vacant(entry) = self.probes.entry((side, None))
        {
            entry.insert(quality(&image, reference)?.map(|quality| Probe {
                quality,
                bytes: None,
            }));
        }
        search_strengths(|strength| {
            let result = match self.probes.entry((side, Some(strength))) {
                Entry::Occupied(entry) => *entry.get(),
                Entry::Vacant(entry) => {
                    let setting = lossy_setting(self.options.format, strength);
                    let result = measure(&image, reference, setting, &self.runner)?;
                    *entry.insert(result)
                }
            };
            Ok(result.is_some_and(|probe| probe.fits(cap, max_bytes)))
        })
    }

    fn intermediate_sides(&self, sides: &[u32], cap: f64, max_bytes: usize) -> Vec<u32> {
        if sides.len() < 2 {
            return Vec::new();
        }
        let closest_side = self
            .probes
            .iter()
            .filter_map(|(&(side, _), probe)| {
                let probe = probe.as_ref()?;
                (probe.fits(cap, max_bytes) && sides.contains(&side))
                    .then_some((side, probe.quality))
            })
            .max_by(|(a_side, a), (b_side, b)| a.total_cmp(b).then(a_side.cmp(b_side)))
            .map(|(side, _)| side);
        let resize_quality = |side| {
            if side == sides[0] {
                Some(100.0)
            } else {
                self.probes.get(&(side, None))?.map(|probe| probe.quality)
            }
        };
        // Refine near the closest measured result and where resizing alone crosses the ceiling.
        sides
            .windows(2)
            .filter_map(|pair| {
                let (upper, lower) = (pair[0], pair[1]);
                let crosses_ceiling = resize_quality(upper)
                    .zip(resize_quality(lower))
                    .is_some_and(|(a, b)| (a <= cap) != (b <= cap));
                let near_best = Some(upper) == closest_side || Some(lower) == closest_side;
                if upper - lower > 1 && (crosses_ceiling || near_best) {
                    Some(u32::midpoint(upper, lower))
                } else {
                    None
                }
            })
            .collect()
    }

    fn encode_best(&mut self, ceiling: u8, lossless: Setting, max_bytes: usize) -> Result<Output> {
        let cap = f64::from(ceiling);
        let mut candidates: Vec<_> = self
            .probes
            .iter_mut()
            .filter_map(|(&key, probe)| {
                let probe = probe.as_mut()?;
                (probe.quality <= cap).then_some((key, probe))
            })
            .collect();
        if candidates.is_empty() {
            return Err(Error::QualityCeiling(ceiling));
        }
        candidates.sort_unstable_by_key(|(_, probe)| Reverse(displayed_tenths(probe.quality)));
        let mut best: Option<(ProbeKey, usize, f64)> = None;
        for (key, probe) in candidates {
            if best.is_some_and(|(_, _, score)| {
                displayed_tenths(probe.quality) < displayed_tenths(score)
            }) {
                break;
            }
            // Resize-only candidates need encoding only when their score competes for first place.
            let bytes = match probe.bytes {
                Some(bytes) => bytes,
                None => {
                    let image = raster::limit_size(self.original, key.0);
                    let bytes = lossless.encode(&image, &self.runner)?.len();
                    probe.bytes = Some(bytes);
                    bytes
                }
            };
            if bytes <= max_bytes
                && best.is_none_or(|(other_key, other_bytes, other_quality)| {
                    bytes
                        .cmp(&other_bytes)
                        .then_with(|| other_quality.total_cmp(&probe.quality))
                        .then_with(|| other_key.cmp(&key))
                        .is_lt()
                })
            {
                best = Some((key, bytes, probe.quality));
            }
        }
        let ((side, strength), _, quality) = best.ok_or(Error::TestSizeLimit)?;
        let image = raster::limit_size(self.original, side);
        let setting = strength.map_or(lossless, |value| lossy_setting(self.options.format, value));
        let bytes = setting.encode(&image, &self.runner)?;
        if bytes.len() > max_bytes {
            return Err(Error::TestSizeLimit);
        }
        Ok(Output {
            bytes,
            dimensions: image.dimensions(),
            quality,
            setting,
        })
    }
}

impl Probe {
    fn fits(self, cap: f64, max_bytes: usize) -> bool {
        self.quality <= cap && self.bytes.is_none_or(|bytes| bytes <= max_bytes)
    }
}

fn displayed_tenths(quality: f64) -> u16 {
    (quality * 10.0).round_ties_even() as u16
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
    Ok(quality(&decoded, reference)?.map(|quality| Probe {
        quality,
        bytes: Some(bytes.len()),
    }))
}

fn quality(image: &RgbaImage, reference: &raster::QualityReference<'_>) -> Result<Option<f64>> {
    Ok(reference
        .compare(image)?
        .filter(|score| score.score.is_finite())
        .map(|score| score.score.max(0.0)))
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
                assert!(test.probes.values().flatten().all(|probe| {
                    probe.quality > f64::from(target)
                        || displayed_tenths(probe.quality) <= displayed_tenths(actual)
                }));
            }
            let output = test.encode(100).unwrap();
            assert!(output.setting.is_lossless());
            assert_eq!(output.quality, 100.0);
            assert_eq!(output.preview().unwrap(), source.0);
        }
    }

    #[test]
    fn resize_only_can_win_on_size_at_the_same_displayed_quality() {
        let source = Source(RgbaImage::from_fn(96, 64, |x, y| {
            let edge = if (x / 3 + y / 5) % 2 == 0 { 32 } else { 224 };
            Rgba([edge, x as u8, y as u8, 255])
        }));
        let format = OutputFormat::Webp;
        let (lossless, _) = format.settings();
        let mut test = source.quality_test(Options {
            format,
            auto_resize: true,
        });
        let width = 96;
        let side = 48;
        let resized = raster::limit_size(&source.0, side);
        let reference = raster::QualityReference::new(&source.0);
        let score = quality(&resized, &reference).unwrap().unwrap();
        let ceiling = score.ceil().max(1.0) as u8;
        assert!(ceiling < 100);
        test.probe_side(side, width, f64::from(ceiling), usize::MAX)
            .unwrap();
        assert_eq!(test.probes[&(side, None)].unwrap().quality, score);
        test.probes.retain(|key, _| *key == (side, None));
        let bytes = lossless.encode(&resized, &test.runner).unwrap().len();
        test.probes.insert(
            (width, Some(12)),
            Some(Probe {
                quality: score,
                bytes: Some(bytes + 1),
            }),
        );
        let output = test.encode_best(ceiling, lossless, usize::MAX).unwrap();
        assert_eq!(output.setting, lossless);
        assert_eq!(output.dimensions, (48, 32));
        assert_eq!(output.quality, score);
        assert_eq!(output.preview().unwrap(), *resized);
        let higher_ceiling = ceiling + 1;
        let probe = test.probes.get_mut(&(width, Some(12))).unwrap();
        probe.as_mut().unwrap().quality = f64::from(higher_ceiling);
        let bounded = test.encode_best(higher_ceiling, lossless, bytes).unwrap();
        assert_eq!(bounded.quality, score);
        assert_eq!(bounded.bytes, output.bytes);
        assert!(matches!(
            test.encode_best(higher_ceiling, lossless, bytes - 1),
            Err(Error::TestSizeLimit)
        ));
    }
}
