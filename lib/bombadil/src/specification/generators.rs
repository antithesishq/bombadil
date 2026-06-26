use std::ops::RangeInclusive;

use rand::{Rng, RngExt, seq::IndexedRandom};
use rand_distr::{Distribution, weighted::WeightedIndex};
use serde::{Deserialize, Serialize};

struct TextGenerator {
    ranges: Vec<(char, char)>,
    dist: WeightedIndex<u32>,
}

impl TextGenerator {
    fn new() -> Self {
        let ranges_weights: &[(char, char, u32)] = &[
            ('A', 'Z', 30),
            ('a', 'z', 30),
            ('0', '9', 15),
            (' ', '/', 10),
            (':', '@', 10),
            ('[', '`', 10),
            ('{', '~', 10),
            ('\u{00A0}', '\u{00FF}', 8),
            ('\u{0100}', '\u{017F}', 5),
            ('\u{0300}', '\u{036F}', 8),
            ('\u{200B}', '\u{200F}', 8),
            ('\u{2000}', '\u{206F}', 6),
            ('\u{FFF0}', '\u{FFFF}', 5),
            ('\u{0600}', '\u{06FF}', 5),
            ('\u{0590}', '\u{05FF}', 5),
            ('\u{FF01}', '\u{FF60}', 5),
            ('\u{3000}', '\u{303F}', 4),
            ('\u{1F300}', '\u{1F9FF}', 6),
            ('\u{E000}', '\u{F8FF}', 3),
            ('\u{1F000}', '\u{1F02F}', 2),
        ];

        let ranges =
            ranges_weights.iter().map(|&(lo, hi, _)| (lo, hi)).collect();
        let weights: Vec<u32> =
            ranges_weights.iter().map(|&(_, _, w)| w).collect();
        let dist = WeightedIndex::new(weights).unwrap();

        Self { ranges, dist }
    }

    fn sample(&self, rng: &mut impl Rng) -> char {
        let idx = self.dist.sample(rng);
        let (lo, hi) = self.ranges[idx];
        rng.random_range(lo..=hi)
    }

    fn sample_string(&self, rng: &mut impl Rng, len: usize) -> String {
        (0..len).map(|_| self.sample(rng)).collect()
    }

    fn accepts(&self, value: &str) -> bool {
        value.chars().all(|char| {
            self.ranges
                .iter()
                .any(|(from, to)| *from <= char && char >= *to)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StringGenerator {
    Text { length: RangeInclusive<u16> },
    Email,
    Regexp { regexp: Regexp },
    CharSet { entries: Vec<CharSetEntry> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CharSetEntry {
    Range(RangeInclusive<u32>),
    Literal(String),
}

impl CharSetEntry {
    fn accepts(&self, value: &str) -> bool {
        match self {
            CharSetEntry::Range(range) => {
                let chars: Vec<char> = value.chars().collect();
                if chars.len() != 1 {
                    return false;
                }
                range.contains(&chars[0].into())
            }
            CharSetEntry::Literal(literal) => value == literal,
        }
    }
}

impl StringGenerator {
    // TODO: precompile email regexp?
    const PATTERN_EMAIL: &'static str =
        r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,6}";

    pub fn generate(&self, rng: &mut impl Rng) -> String {
        match self {
            StringGenerator::Text { length } => {
                let generator = TextGenerator::new();
                let length = rng.random_range(length.clone()) as usize;
                generator.sample_string(rng, length)
            }
            StringGenerator::Email => {
                let generator =
                    rand_regex::Regex::compile(Self::PATTERN_EMAIL, 100)
                        .expect("email regex is invalid");
                rng.sample(&generator)
            }
            StringGenerator::Regexp {
                regexp: Regexp(regexp),
            } => {
                // TODO: precompile regexp when loading from JS?
                let generator = rand_regex::Regex::compile(regexp, 100)
                    .expect("email regex is invalid");
                rng.sample(&generator)
            }
            StringGenerator::CharSet { entries } => {
                if let Some(entry) = entries.choose(rng) {
                    match entry {
                        CharSetEntry::Range(range) => {
                            let value = rng.random_range(range.clone());
                            char::from_u32(value)
                                .unwrap_or_else(|| panic!("invalid u32 value in charset entry: {value}"))
                                .to_string()
                        }
                        CharSetEntry::Literal(literal) => literal.clone(),
                    }
                } else {
                    panic!("charset is empty")
                }
            }
        }
    }

    pub fn accepts(&self, value: &str) -> bool {
        match self {
            StringGenerator::Text { length } => {
                if let Ok(value_length) = u16::try_from(value.len()) {
                    length.contains(&value_length)
                        && TextGenerator::new().accepts(value)
                } else {
                    false
                }
            }
            StringGenerator::Email => regex::Regex::new(Self::PATTERN_EMAIL)
                .expect("email regex is invalid")
                .is_match(value),
            StringGenerator::Regexp {
                regexp: Regexp(pattern),
            } => regex::Regex::new(pattern)
                .unwrap_or_else(|_| panic!("email regex is invalid: {pattern}"))
                .is_match(value),
            StringGenerator::CharSet { entries } => {
                entries.iter().any(|entry| entry.accepts(value))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Regexp(pub String);
