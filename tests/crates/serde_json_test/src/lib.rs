use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Player {
    name: String,
    level: u32,
    score: f64,
    tags: Vec<String>,
    inventory: BTreeMap<String, u16>,
    guild: Option<String>,
}

pub fn run() {
    let mut inv = BTreeMap::new();
    inv.insert("sword".to_string(), 1);
    inv.insert("potion".to_string(), 12);
    let p = Player { name: "Ana".into(), level: 42, score: 1234.5, tags: vec!["pro".into(), "vip".into()], inventory: inv, guild: None };
    let s = serde_json::to_string(&p).unwrap();
    println!("{s}");
    let back: Player = serde_json::from_str(&s).unwrap();
    println!("roundtrip {}", back == p);
    let v: serde_json::Value = serde_json::from_str(r#"{"a":[1,2,{"b":null}],"c":"é\n","d":-1.5e3,"e":true}"#).unwrap();
    println!("{} {} {}", v["a"][2], v["c"], v["d"].as_f64().unwrap());
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    let err = serde_json::from_str::<Player>(r#"{"name": 5}"#).unwrap_err();
    println!("err: {err}");
}
