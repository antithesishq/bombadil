use std::collections::BTreeMap;
use std::fmt::Debug;
use std::marker::PhantomData;

use bombadil_ltl::formula::{Domain, State};
use bombadil_schema::Time;
use serde::{Deserialize, Serialize};
use serde_json as json;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub index: usize,
    pub name: Option<String>,
    pub value: json::Value,
    pub time: Time,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct UniqueSnapshots(BTreeMap<(usize, Time), Snapshot>);

impl std::ops::Deref for UniqueSnapshots {
    type Target = BTreeMap<(usize, Time), Snapshot>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromIterator<((usize, Time), Snapshot)> for UniqueSnapshots {
    fn from_iter<I: IntoIterator<Item = ((usize, Time), Snapshot)>>(
        iter: I,
    ) -> Self {
        UniqueSnapshots(iter.into_iter().collect())
    }
}

impl State for UniqueSnapshots {
    fn merge(&self, other: &Self) -> Self {
        let mut merged = self.0.clone();
        merged.extend(
            other
                .0
                .iter()
                .map(|(index, snapshot)| (*index, snapshot.clone())),
        );
        UniqueSnapshots(merged)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BombadilDomain<F>(PhantomData<F>);

impl<F: Clone + Debug + PartialEq> Domain for BombadilDomain<F> {
    type Function = F;
    type Time = Time;
    type Duration = std::time::Duration;
    type State = UniqueSnapshots;
}
