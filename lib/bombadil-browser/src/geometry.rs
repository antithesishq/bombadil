use std::ops::RangeInclusive;

use rand::distr::uniform::SampleUniform;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Point<N = f64> {
    pub x: N,
    pub y: N,
}

impl Point {
    pub fn distance(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

impl<N: PartialOrd + SampleUniform + Clone> Point<RangeInclusive<N>> {
    pub fn accepts(&self, other: &Point<N>) -> bool {
        self.x.contains(&other.x) && self.y.contains(&other.y)
    }

    pub fn generate<Rng: rand::TryRng + rand::RngExt>(
        &self,
        rng: &mut Rng,
    ) -> Point<N> {
        Point {
            x: rng.random_range(self.x.clone()),
            y: rng.random_range(self.y.clone()),
        }
    }
}
