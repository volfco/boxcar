use anyhow::Context;
use serde::{Deserialize, Serialize};

pub fn encode<T: Serialize>(msg: T) -> anyhow::Result<Vec<u8>> {
    bincode::serialize(&msg).context("")
}

pub fn decode<'a, T: Deserialize<'a>>(msg: &'a [u8]) -> anyhow::Result<T> {
    bincode::deserialize(msg).context("")
}
