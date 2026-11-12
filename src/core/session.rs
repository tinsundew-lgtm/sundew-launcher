//! Explicit user-entered session preferences; no invented play-time telemetry.
use serde::{Serialize,Deserialize};
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(default)]
pub struct SessionNotes {
    pub recap:String, pub next_goal:String, pub screenshot:Option<String>,
    pub min_minutes:u32, pub startup_minutes:u32, pub mood:String,
}
impl Default for SessionNotes {fn default()->Self {Self{recap:String::new(),next_goal:String::new(),screenshot:None,min_minutes:20,startup_minutes:2,mood:"Любое".into()}}}
pub fn fits(notes:&SessionNotes,minutes:u32,mood:&str)->bool {
    notes.min_minutes.saturating_add(notes.startup_minutes)<=minutes && (mood=="Любое" || notes.mood=="Любое" || notes.mood==mood)
}
#[cfg(test)]mod tests {use super::*;#[test]fn recommendation_respects_budget_and_mood(){let n=SessionNotes{min_minutes:30,startup_minutes:5,mood:"С друзьями".into(),..Default::default()};assert!(!fits(&n,34,"Любое"));assert!(fits(&n,35,"С друзьями"));assert!(!fits(&n,60,"Расслабиться"));}#[test]fn older_notes_get_defaults(){let n:SessionNotes=serde_json::from_str("{}").unwrap();assert_eq!(n.min_minutes,20);}}
