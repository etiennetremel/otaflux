use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistryTagList {
    pub name: String,
    pub tags: Vec<String>,
}
