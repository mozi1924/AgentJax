use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantPhase {
    Commentary,
    FinalAnswer,
}

impl AssistantPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commentary => "commentary",
            Self::FinalAnswer => "final_answer",
        }
    }

    pub fn from_api_value(value: &str) -> Option<Self> {
        match value.trim() {
            "commentary" => Some(Self::Commentary),
            "final_answer" => Some(Self::FinalAnswer),
            _ => None,
        }
    }
}
