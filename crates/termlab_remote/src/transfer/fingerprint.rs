use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceFingerprint {
    #[ts(as = "f64")]
    pub size: u64,
    pub modified_token: Option<String>,
}
