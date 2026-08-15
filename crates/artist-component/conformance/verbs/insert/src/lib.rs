wit_bindgen::generate!({ path: ".", world: "tool-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
include!("../../../typed-guest/src/lib.rs");
