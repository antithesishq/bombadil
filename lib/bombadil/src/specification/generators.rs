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
    Range(RangeInclusive<u32>),
    Literal(String),
}

pub fn range_contains<T: PartialOrd>(
    outer: &RangeInclusive<T>,
    inner: &RangeInclusive<T>,
) -> bool {
    outer.start() <= inner.start() && inner.end() <= outer.end()
}

impl StringGenerator {
    /// Whether this generator generates every string that `other` does. It
    /// may say no when it does (e.g. a range that two entries cover between
    /// them), but not yes when it does not.
    pub fn contains(&self, other: &StringGenerator) -> bool {
        match (self, other) {
            (
                StringGenerator::Text { length: outer },
                StringGenerator::Text { length: inner },
            ) => range_contains(outer, inner),
            (
                StringGenerator::CharSet { entries: outer },
                StringGenerator::CharSet { entries: inner },
            ) => inner
                .iter()
                .all(|inner| outer.iter().any(|outer| outer.contains(inner))),
            _ => self == other,
        }
    }
}

impl CharSetEntry {
    fn contains(&self, other: &CharSetEntry) -> bool {
        match (self, other) {
            (CharSetEntry::Range(outer), CharSetEntry::Range(inner)) => {
                range_contains(outer, inner)
            }
            (_, CharSetEntry::Literal(literal)) => self.accepts(literal),
            (CharSetEntry::Literal(literal), CharSetEntry::Range(range)) => {
                range.start() == range.end()
                    && char::from_u32(*range.start())
                        .is_some_and(|char| literal.chars().eq([char]))
            }
        }
    }
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
        generators::{
            booleans, integers, just, one_of, sampled_from, text, vecs,
        },
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

    fn range_within<T: hegel::generators::Integer>(
        tc: &TestCase,
        outer: &RangeInclusive<T>,
    ) -> RangeInclusive<T> {
        let bounds =
            || integers().min_value(*outer.start()).max_value(*outer.end());
        let start = tc.draw_silent(bounds());
        let end = tc.draw_silent(bounds().min_value(start));
        start..=end
    }

    /// A generator of some of the strings that `generator` generates.
    fn narrowed(tc: &TestCase, generator: &StringGenerator) -> StringGenerator {
        match generator {
            StringGenerator::Text { length } => StringGenerator::Text {
                length: range_within(tc, length),
            },
            StringGenerator::CharSet { entries } => {
                let count =
                    tc.draw_silent(integers().min_value(1).max_value(4));
                let entries = (0..count)
                    .map(|_| {
                        match tc.draw_silent(sampled_from(entries.clone())) {
                            CharSetEntry::Range(range) => {
                                CharSetEntry::Range(range_within(tc, &range))
                            }
                            literal => literal,
                        }
                    })
                    .collect();
                StringGenerator::CharSet { entries }
            }
            other => other.clone(),
        }
    }

    #[hegel::test]
    fn string_generator_contains_its_narrowings(tc: TestCase) {
        let generator = tc.draw(string_generators().print_as_debug());
        let narrowed = narrowed(&tc, &generator);
        assert!(
            generator.contains(&narrowed),
            "{generator:?} does not contain {narrowed:?}"
        );
    }

    #[hegel::test]
    fn string_generator_accepts_what_a_contained_one_generates(tc: TestCase) {
        let containing = tc.draw(string_generators().print_as_debug());
        let contained = if tc.draw(booleans()) {
            narrowed(&tc, &containing)
        } else {
            tc.draw(string_generators().print_as_debug())
        };
        let mut rng = StdRng::seed_from_u64(tc.draw(integers()));
        if containing.contains(&contained) {
            let value = contained.generate(&mut rng);
            assert!(
                containing.accepts(&value),
                "{containing:?} contains {contained:?} but rejected \
                 {value:?}"
            );
        }
    }

    #[hegel::composite]
    fn string_generators(tc: &TestCase) -> StringGenerator {
        tc.draw(
            one_of([
                just(StringGenerator::Email).boxed(),
                just(StringGenerator::Text {
                    length: range_within(tc, &(0..=64)),
                })
                .boxed(),
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
    fn char_ranges(tc: &TestCase) -> RangeInclusive<u32> {
        fn in_range(tc: &TestCase, min: u32, max: u32) -> RangeInclusive<u32> {
            let start = tc.draw(integers().min_value(min).max_value(max));
            let end = tc.draw(integers().min_value(start).max_value(max));
            start..=end
        }

        tc.draw(one_of([
            just(in_range(tc, 0x0000, 0xD7FF)),
            just(in_range(tc, 0xE000, 0x10FFFF)),
        ]))
    }
}
