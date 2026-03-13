use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct HelloResponse {
    pub message: String,
}
