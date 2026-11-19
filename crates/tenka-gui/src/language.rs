use std::{collections::HashMap, env};

const ENGLISH: &str = include_str!("../../../languages/English.ini");
const RUSSIAN: &str = include_str!("../../../languages/Russian.ini");

pub struct Catalog(HashMap<u16, &'static str>);

impl Catalog {
    pub fn new(choice: &str) -> Self {
        let mut english: HashMap<u16, _> = entries(ENGLISH)
            .map(|(id, original, _)| (id, original))
            .collect();
        if choice == "ru" || choice == "system" && system_language() == "ru" {
            for (id, original, translation) in entries(RUSSIAN) {
                // The catalog has no interpolated entries.
                if !translation.is_empty()
                    && !translation.contains(['{', '}'])
                    && english.get(&id) == Some(&original)
                {
                    english.insert(id, translation);
                }
            }
        }
        Self(english)
    }

    pub fn text(&self, id: u16) -> &'static str {
        self.0.get(&id).copied().unwrap_or("")
    }

    pub(crate) fn file_size(&self, bytes: usize) -> String {
        if bytes < 1024 {
            return format!("{bytes} {}", self.text(117));
        }
        let kib = bytes as f64 / 1024.0;
        // Display thousands of the previous KiB counter in the compact table.
        let (value, unit) = if kib >= 1000.0 {
            (kib / 1000.0, 119)
        } else {
            (kib, 118)
        };
        let value = format!("{value:.1}").replace('.', self.text(120));
        format!("{value} {}", self.text(unit))
    }

    pub(crate) fn duration(&self, seconds: u64) -> String {
        if seconds < 60 {
            return format!("{} {}", seconds.max(1), self.text(126));
        }
        let minutes = seconds.div_ceil(60);
        if minutes < 60 {
            return format!("{minutes} {}", self.text(127));
        }
        let hours = minutes / 60;
        let minutes = minutes % 60;
        if minutes == 0 {
            return format!("{hours} {}", self.text(128));
        }
        format!("{hours} {} {minutes} {}", self.text(128), self.text(127))
    }
}

fn entries(catalog: &'static str) -> impl Iterator<Item = (u16, &'static str, &'static str)> {
    catalog
        .trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| {
            let (id, body) = line.split_once('=')?;
            let (original, translation) = body.split_once('=')?;
            Some((id.trim().parse().ok()?, original.trim(), translation.trim()))
        })
}

fn system_language() -> &'static str {
    for key in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = env::var(key) {
            for locale in value.split(':') {
                match locale.split(['_', '-', '.', '@']).next() {
                    Some("ru") => return "ru",
                    Some("en" | "C" | "POSIX") => return "en",
                    _ => {}
                }
            }
        }
    }
    "en"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_entries_match_their_english_keys() {
        let english: HashMap<_, _> = entries(ENGLISH)
            .map(|(id, original, _)| (id, original))
            .collect();
        let russian: HashMap<_, _> = entries(RUSSIAN)
            .map(|(id, original, _)| (id, original))
            .collect();
        assert_eq!(english, russian);
        assert_eq!(Catalog::new("ru").text(22), "Конвертировать");
    }

    #[test]
    fn file_sizes_use_the_catalog_units_and_decimal_separator() {
        let ru = Catalog::new("ru");
        let en = Catalog::new("en");
        assert_eq!(ru.file_size(18_907_265), "18,5 МБ");
        assert_eq!(ru.file_size(1_462_628), "1,4 МБ");
        assert_eq!(en.file_size(1_462_628), "1.4 MB");
        assert_eq!(en.file_size(1_536), "1.5 KB");
        assert_eq!(en.file_size(512), "512 B");
    }
}
