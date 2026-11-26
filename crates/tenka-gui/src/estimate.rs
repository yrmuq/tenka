use std::{collections::VecDeque, time::Duration};

#[derive(Clone, Copy, Default)]
pub struct InputSize {
    pub bytes: Option<u64>,
    pub pixels: Option<u64>,
}

#[derive(Clone, Copy)]
pub struct Timing {
    pub bytes: u64,
    pub pixels: u64,
    pub read: Duration,
    pub processing: Duration,
}

#[derive(Default)]
pub struct Estimate {
    recent: VecDeque<Timing>,
}

impl Estimate {
    pub fn record(&mut self, timing: Timing) {
        if self.recent.len() == 10 {
            self.recent.pop_front();
        }
        self.recent.push_back(timing);
    }

    pub fn seconds(&self, remaining: impl Iterator<Item = InputSize>) -> Option<u64> {
        if self.recent.is_empty() {
            return None;
        }

        let mut bytes = 0;
        let mut pixels = 0;
        let mut read = Duration::ZERO;
        let mut processing = Duration::ZERO;
        for timing in &self.recent {
            bytes += timing.bytes;
            pixels += timing.pixels;
            read += timing.read;
            processing += timing.processing;
        }

        let samples = self.recent.len() as f64;
        let read = read.as_secs_f64();
        let processing = processing.as_secs_f64();
        let read_per_byte = read / bytes as f64;
        let processing_per_pixel = processing / pixels as f64;
        let seconds = remaining
            .map(|input| {
                let read = input
                    .bytes
                    .map_or(read / samples, |size| read_per_byte * size as f64);
                let processing = input.pixels.map_or(processing / samples, |size| {
                    processing_per_pixel * size as f64
                });
                read + processing
            })
            .reduce(|a, b| a + b)?;
        Some(seconds.ceil() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_scales_reads_by_bytes_and_processing_by_pixels() {
        let mut estimate = Estimate::default();
        estimate.record(Timing {
            bytes: 100,
            pixels: 10,
            read: Duration::from_secs(2),
            processing: Duration::from_secs(8),
        });
        let inputs = [
            InputSize {
                bytes: Some(300),
                pixels: Some(20),
            },
            InputSize {
                bytes: None,
                pixels: Some(5),
            },
            InputSize {
                bytes: Some(50),
                pixels: None,
            },
        ];
        assert_eq!(estimate.seconds(inputs.into_iter()), Some(37));
        assert_eq!(estimate.seconds(std::iter::empty()), None);
    }

    #[test]
    fn estimate_uses_up_to_ten_completed_files_from_this_batch() {
        let mut estimate = Estimate::default();
        let sample = |seconds| Timing {
            bytes: 100,
            pixels: 10,
            read: Duration::ZERO,
            processing: Duration::from_secs(seconds),
        };
        let remaining = || std::iter::repeat_n(InputSize::default(), 11);
        assert_eq!(estimate.seconds(remaining()), None);
        estimate.record(sample(40));
        assert_eq!(estimate.seconds(remaining()), Some(440));
        for _ in 0..10 {
            estimate.record(sample(2));
        }
        assert_eq!(estimate.seconds(remaining()), Some(22));
    }
}
