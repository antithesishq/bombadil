use serde::{Deserialize, Serialize};

// TODO: make this a type level parameter of SmallString instead of a constant?
const STRING_INLINE_SIZE_MAX: usize = 4;

/// A string stored inline for small grapheme clusters (most common), and on the
/// heap for larger clusters.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum SmallString {
    Inline {
        buffer: [u8; STRING_INLINE_SIZE_MAX],
        size: u8,
    },
    Heap(Box<str>),
}

impl SmallString {
    pub fn as_str(&self) -> &str {
        match self {
            SmallString::Inline { buffer, size } => {
                std::str::from_utf8(&buffer[..*size as usize]).unwrap()
            }
            SmallString::Heap(string) => string,
        }
    }
}

impl std::fmt::Debug for SmallString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

impl Serialize for SmallString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SmallString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(SmallString::from(<&str>::deserialize(deserializer)?))
    }
}

impl From<&str> for SmallString {
    fn from(input: &str) -> Self {
        let source = input.as_bytes();
        let source_size = source.len();
        if source_size <= STRING_INLINE_SIZE_MAX {
            let mut buffer = [0; STRING_INLINE_SIZE_MAX];
            buffer[..source_size].copy_from_slice(source);
            SmallString::Inline {
                buffer,
                size: source_size as u8,
            }
        } else {
            SmallString::Heap(input.into())
        }
    }
}

impl From<String> for SmallString {
    fn from(input: String) -> Self {
        Self::from(input.as_str())
    }
}

impl std::ops::Deref for SmallString {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use proptest::proptest;

    use super::{STRING_INLINE_SIZE_MAX, SmallString};

    proptest! {
        #[test]
        fn test_string_roundtrip(input in ".*") {
            let small = SmallString::from(input.as_str());
            assert_eq!(small.as_str(), input.as_str());
        }

        #[test]
        fn test_inline_vs_heap(input in ".*") {
            let small = SmallString::from(input.as_str());
            match &small {
                SmallString::Inline { size, .. } => assert!(input.len() <= STRING_INLINE_SIZE_MAX, "should be heap: len={}", *size),
                SmallString::Heap(_) => assert!(input.len() > STRING_INLINE_SIZE_MAX),
            }
        }
    }
}
