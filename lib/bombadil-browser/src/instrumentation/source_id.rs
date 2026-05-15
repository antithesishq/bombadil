use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Copy, Clone)]
pub struct SourceId(pub u64);

impl SourceId {
    pub fn hash<I: Hash>(input: I) -> Self {
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        SourceId(hasher.finish())
    }

    pub fn add<I: Hash>(&self, input: I) -> Self {
        // TODO: is this hash-chaining bad for distribution?
        Self::hash((self.0, input))
    }
}
