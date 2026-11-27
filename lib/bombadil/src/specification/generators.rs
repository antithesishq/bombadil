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
                .any(|(from, to)| *from <= char && char <= *to)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum StringGenerator {
    Text { length: RangeInclusive<u16> },
    Email,
    Regexp { regexp: Regexp },
    CharSet { entries: Vec<CharSetEntry> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CharSetEntry {
    Range(CharRange),
    Literal(String),
}

/// The characters in an inclusive range of code points. The range may span
/// the surrogates, which are not characters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "RangeInclusive<u32>", into = "RangeInclusive<u32>")]
pub struct CharRange {
    first: char,
    last: char,
}

impl CharRange {
    const SURROGATE_FIRST: u32 = 0xD800;
    const SURROGATE_COUNT: u32 = 0x800;

    pub fn first(&self) -> char {
        self.first
    }

    pub fn last(&self) -> char {
        self.last
    }

    pub fn contains(&self, char: char) -> bool {
        self.first <= char && char <= self.last
    }

    /// Every character in the range is equally likely.
    pub fn sample(&self, rng: &mut impl Rng) -> char {
        let (first, last) = (u32::from(self.first), u32::from(self.last));
        // Both ends are characters, so the range has all surrogates or none.
        let skipped =
            if first < Self::SURROGATE_FIRST && last >= Self::SURROGATE_FIRST {
                Self::SURROGATE_COUNT
            } else {
                0
            };
        let mut value = rng.random_range(first..=last - skipped);
        if skipped > 0 && value >= Self::SURROGATE_FIRST {
            value += skipped;
        }
        char::from_u32(value).expect("a character between two characters")
    }
}

impl TryFrom<RangeInclusive<u32>> for CharRange {
    type Error = anyhow::Error;

    fn try_from(range: RangeInclusive<u32>) -> anyhow::Result<Self> {
        let (start, end) = range.into_inner();
        anyhow::ensure!(
            start <= end,
            "charset range {start:#x} to {end:#x} is empty"
        );
        anyhow::ensure!(
            end <= u32::from(char::MAX),
            "charset range {start:#x} to {end:#x} ends above the last code point, 0x10ffff"
        );
        // An end among the surrogates moves inward to a character.
        let first = char::from_u32(start).unwrap_or('\u{E000}');
        let last = char::from_u32(end).unwrap_or('\u{D7FF}');
        anyhow::ensure!(
            first <= last,
            "charset range {start:#x} to {end:#x} has only surrogates (0xd800 to 0xdfff), which are not characters"
        );
        Ok(Self { first, last })
    }
}

impl From<CharRange> for RangeInclusive<u32> {
    fn from(range: CharRange) -> Self {
        u32::from(range.first)..=u32::from(range.last)
    }
}

// As the code point range hashes, which keeps action hashes as they were.
impl std::hash::Hash for CharRange {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        RangeInclusive::from(*self).hash(state);
    }
}

impl CharSetEntry {
    fn accepts(&self, value: &str) -> bool {
        match self {
            CharSetEntry::Range(range) => {
                let mut chars = value.chars();
                match (chars.next(), chars.next()) {
                    (Some(char), None) => range.contains(char),
                    _ => false,
                }
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
                            range.sample(rng).to_string()
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
                if let Ok(value_length) = u16::try_from(value.chars().count()) {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Regexp(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::{
        Generator, TestCase,
        generators::{integers, just, one_of, sampled_from, text, vecs},
    };
    use rand::{SeedableRng, rngs::StdRng};

    #[hegel::test]
    fn string_generator_accepts_generated(tc: TestCase) {
        let generator = tc.draw(string_generators().print_as_debug());
        let mut rng = StdRng::seed_from_u64(tc.draw(integers()));
        let value = generator.generate(&mut rng);
        assert!(
            generator.accepts(&value),
            "generator must accept all values it generates, but rejected {value:?}"
        );
    }

    #[hegel::composite]
    fn string_generators(tc: &TestCase) -> StringGenerator {
        tc.draw(
            one_of([
                just(StringGenerator::Email).boxed(),
                vecs(charset_entries())
                    .min_size(1)
                    .map(|entries| StringGenerator::CharSet { entries })
                    .boxed(),
            ])
            .print_as_debug(),
        )
    }

    #[hegel::composite]
    fn charset_entries(tc: &TestCase) -> CharSetEntry {
        tc.draw(
            one_of([
                char_ranges().map(CharSetEntry::Range).boxed(),
                text().map(CharSetEntry::Literal).boxed(),
            ])
            .print_as_debug(),
        )
    }

    #[hegel::composite]
    fn code_points(tc: &TestCase) -> u32 {
        let edge = tc
            .draw(sampled_from(vec![0, 0xD800, 0xE000, 0x11_0000, 0x12_0000]));
        match tc.draw(integers::<u8>().max_value(2)) {
            0 => tc.draw(integers().max_value(0x12_0000)),
            1 => edge + tc.draw(integers::<u32>().max_value(4)),
            _ => edge.saturating_sub(tc.draw(integers::<u32>().max_value(4))),
        }
    }

    fn is_character(code_point: u32) -> bool {
        char::from_u32(code_point).is_some()
    }

    #[hegel::composite]
    fn char_ranges(tc: &TestCase) -> CharRange {
        let start = tc.draw(code_points().filter(|&start| is_character(start)));
        let end = tc.draw(code_points().filter(|&end| is_character(end)));
        CharRange::try_from(start.min(end)..=start.max(end)).unwrap()
    }

    #[hegel::test(test_cases = 1000)]
    fn char_range_has_the_characters_of_its_code_points(tc: TestCase) {
        let (start, end) = (tc.draw(code_points()), tc.draw(code_points()));
        let characters = start <= end
            && end <= 0x10_FFFF
            && (start < 0xD800 || end > 0xDFFF);
        let range = CharRange::try_from(start..=end);
        assert_eq!(range.is_ok(), characters, "{range:?}");
        let Ok(range) = range else {
            return;
        };
        assert!(start <= u32::from(range.first()));
        assert!(u32::from(range.last()) <= end);
        assert!(
            (start..u32::from(range.first())).all(|cut| !is_character(cut))
        );
        assert!(
            (u32::from(range.last()) + 1..=end).all(|cut| !is_character(cut))
        );

        let mut rng = StdRng::seed_from_u64(tc.draw(integers()));
        let sampled = range.sample(&mut rng);
        assert!(range.contains(sampled), "{sampled:?} outside {range:?}");
        assert!(
            CharSetEntry::Range(range).accepts(&sampled.to_string()),
            "{sampled:?} outside {range:?}"
        );
    }

    #[hegel::test(test_cases = 1000)]
    fn char_range_serialises_and_hashes_as_its_code_points(tc: TestCase) {
        use std::hash::{Hash, Hasher};

        let range = tc.draw(char_ranges().print_as_debug());
        let code_points = RangeInclusive::from(range);

        let json = serde_json::to_value(range).unwrap();
        assert_eq!(json, serde_json::to_value(&code_points).unwrap());
        assert_eq!(serde_json::from_value::<CharRange>(json).unwrap(), range);

        let hash = |value: &dyn Fn(&mut stdx::hash::StableHasher)| {
            let mut hasher = stdx::hash::StableHasher::default();
            value(&mut hasher);
            hasher.finish()
        };
        assert_eq!(
            hash(&|hasher| range.hash(hasher)),
            hash(&|hasher| code_points.hash(hasher))
        );
    }

    #[hegel::test(test_cases = 200)]
    fn char_range_samples_every_character_across_surrogates(tc: TestCase) {
        let start =
            0xD800 - tc.draw(integers::<u32>().min_value(1).max_value(8));
        let end = 0xDFFF + tc.draw(integers::<u32>().min_value(1).max_value(8));
        let range = CharRange::try_from(start..=end).unwrap();
        let mut rng = StdRng::seed_from_u64(tc.draw(integers()));
        // At most 16 characters, so 2000 samples do not miss one by chance.
        let sampled: std::collections::BTreeSet<char> =
            (0..2000).map(|_| range.sample(&mut rng)).collect();
        let characters: std::collections::BTreeSet<char> =
            (start..=end).filter_map(char::from_u32).collect();
        assert_eq!(sampled, characters);
    }

    #[test]
    fn char_range_errors() {
        let error = |start: u32, end: u32| {
            CharRange::try_from(start..=end).unwrap_err().to_string()
        };
        insta::assert_snapshot!(
            [error(0xD800, 0xDFFF), error(0xDC00, 0xDC00), error(0, 0x11_0000)].join("\n"),
            @"
        charset range 0xd800 to 0xdfff has only surrogates (0xd800 to 0xdfff), which are not characters
        charset range 0xdc00 to 0xdc00 has only surrogates (0xd800 to 0xdfff), which are not characters
        charset range 0x0 to 0x110000 ends above the last code point, 0x10ffff
        "
        );
    }
}
