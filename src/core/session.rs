//! User-owned journal. Older recommendation fields are ignored on load.
use serde::{Serialize,Deserialize};
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
#[serde(default)]
pub struct SessionNotes {
    pub recap:String, pub next_goal:String, pub screenshot:Option<String>,
}
#[cfg(test)] mod tests {use super::*;#[test]fn legacy_notes_keep_text(){let n:SessionNotes=serde_json::from_str(r#"{"recap":"hello","min_minutes":30,"mood":"Любое"}"#).unwrap();assert_eq!(n.recap,"hello");}}
