use std::fmt::Write;

use serde::{Deserialize, Serialize};

// TODO: make this a type level parameter of SmallString instead of a constant?
const STRING_INLINE_CHARS_COUNT_MAX: usize = 4;

/// A string stored inline for small grapheme clusters (most common), and on the
/// heap for larger clusters.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum SmallString {
    Inline {
        buffer: [char; STRING_INLINE_CHARS_COUNT_MAX],
        size: u8,
    },
    Heap(Vec<char>),
}

impl SmallString {
    pub fn null_with_size(size: usize) -> Self {
        if size <= STRING_INLINE_CHARS_COUNT_MAX {
            SmallString::Inline {
                buffer: ['\0'; STRING_INLINE_CHARS_COUNT_MAX],
                size: size as u8,
            }
        } else {
            SmallString::Heap(vec!['\0'; size])
        }
    }
}

impl std::fmt::Debug for SmallString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for SmallString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for c in self.iter() {
            f.write_char(*c)?;
        }
        Ok(())
    }
}

impl Serialize for SmallString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SmallString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let string = String::deserialize(deserializer)?;
        Ok(SmallString::from(string))
    }
}

impl From<&[char]> for SmallString {
    fn from(input: &[char]) -> Self {
        let source_size = input.len();
        if source_size <= STRING_INLINE_CHARS_COUNT_MAX {
            let mut buffer = ['\0'; STRING_INLINE_CHARS_COUNT_MAX];
            for (i, char) in input.iter().enumerate() {
                buffer[i] = *char;
            }
            SmallString::Inline {
                buffer,
                size: source_size as u8,
            }
        } else {
            SmallString::Heap(input.to_vec())
        }
    }
}

impl From<&str> for SmallString {
    fn from(input: &str) -> Self {
        Self::from(input.chars().collect::<Vec<char>>().as_slice())
    }
}

impl From<String> for SmallString {
    fn from(input: String) -> Self {
        Self::from(input.as_str())
    }
}

impl std::ops::Deref for SmallString {
    type Target = [char];
    fn deref(&self) -> &[char] {
        match self {
            SmallString::Inline { buffer, size } => &buffer[..*size as usize],
            SmallString::Heap(chars) => chars,
        }
    }
}

impl std::ops::DerefMut for SmallString {
    fn deref_mut(&mut self) -> &mut [char] {
        match self {
            SmallString::Inline { buffer, size } => {
                &mut buffer[..*size as usize]
            }
            SmallString::Heap(chars) => chars,
        }
    }
}

#[cfg(test)]
mod tests {
    use hegel::TestCase;
    use hegel::generators::text;

    use super::{STRING_INLINE_CHARS_COUNT_MAX, SmallString};

    #[hegel::test]
    fn test_string_roundtrip(tc: TestCase) {
        let input = tc.draw(text());
        let chars: Vec<char> = input.chars().collect();
        let small = SmallString::from(chars.as_slice());
        assert_eq!(&*small, &chars);
    }

    #[hegel::test]
    fn test_inline_vs_heap(tc: TestCase) {
        let input = tc.draw(text());
        let chars: Vec<char> = input.chars().collect();
        let small = SmallString::from(chars.as_slice());
        match &small {
            SmallString::Inline { size, .. } => assert!(
                chars.len() <= STRING_INLINE_CHARS_COUNT_MAX,
                "should be heap: len={}",
                *size
            ),
            SmallString::Heap(_) => {
                assert!(input.len() > STRING_INLINE_CHARS_COUNT_MAX)
            }
        }
    }
}
