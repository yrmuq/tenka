use std::{collections::HashMap, env};

const ENGLISH: &str = include_str!("../../../languages/English.ini");
const RUSSIAN: &str = include_str!("../../../languages/Russian.ini");

pub const LANGUAGES: [&str; 3] = ["system", "en", "ru"];

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
}
