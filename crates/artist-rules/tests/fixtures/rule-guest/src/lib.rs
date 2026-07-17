//! Fixture guest for the wasm-rule host tests, and a working starter
//! template for programmable stream rules.
//!
//! This rule demonstrates what regexes can't do: *stateful* matching. Its
//! native prefilter (in the sibling `.toml` manifest) matches cheap
//! patterns; the guest then counts hits in host KV and only fires on the
//! third strike.

wit_bindgen::generate!({
    path: "../../../wit/rule-plugin.wit",
    world: "rule-plugin",
});

struct Plugin;

impl Guest for Plugin {
    fn meta() -> String {
        "third-strike".to_owned()
    }

    fn on_event(ev: Event) -> Verdict {
        // Test hooks for the host's sandbox limits.
        if ev.text.contains("INFINITE_LOOP") {
            #[allow(clippy::empty_loop)]
            loop {}
        }
        if ev.text.contains("MEMORY_BOMB") {
            let mut bombs: Vec<Vec<u8>> = Vec::new();
            loop {
                bombs.push(vec![0u8; 16 * 1024 * 1024]);
                std::hint::black_box(&bombs);
            }
        }

        let hits = host::kv_get("hits")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        host::kv_set("hits", &hits.to_string());
        host::log(&format!("strike {hits} on {:?} (turn {})", ev.target, ev.turn));
        if hits >= 3 {
            Verdict::Fire(Firing {
                reminder: format!(
                    "Third strike on \"{}\" — stop and take a different approach.",
                    ev.text
                ),
                persistence: "session".to_owned(),
            })
        } else {
            Verdict::Pass
        }
    }
}

export!(Plugin);
