//! Artist's palette, expanded into something a whole UI can be built from.
//!
//! # Why remap rather than instruct
//!
//! A model writing a canvas reaches for `bg-blue-500` reflexively. Telling it
//! not to is a losing battle across the range of things it might write, so the
//! utilities are redefined instead: `blue` *is* artist's blue. Nothing to obey,
//! so nothing to drift from.
//!
//! # Why a ramp rather than a flat substitution
//!
//! The six colours in the TUI are pastels — light tints meant to sit on a dark
//! terminal. Dropping `#C6E2E7` in wherever Tailwind says `blue` would make
//! every button and every piece of body text illegible: that colour on white is
//! about 1.3:1, against the 4.5:1 that readable text needs.
//!
//! So each pastel anchors a full 50–950 ramp. It keeps its hue throughout; the
//! light steps stay pastel for surfaces, and the dark steps take on lightness
//! and a little saturation so text and controls read. `bg-blue-100` is
//! recognisably the terminal's blue, and `text-blue-700` is recognisably the
//! same hue, dark enough to be read.

/// Artist's six, mirroring `artist-cli/src/theme.rs`. Kept as literals rather
/// than shared through a dependency: the TUI crate depends on ratatui's colour
/// type, and a canvas has no business pulling that in.
pub const PINK: u32 = 0xFF_CC_E1;
pub const WHITE: u32 = 0xF2_F1_ED;
pub const MINT: u32 = 0xCD_E5_D9;
pub const YELLOW: u32 = 0xF2_EB_CC;
pub const BLUE: u32 = 0xC6_E2_E7;
pub const BLUSH: u32 = 0xF7_DD_E8;

/// A pastel red, for the one thing the terminal palette has no answer for.
///
/// Danger has to read as danger — reusing pink would make a destructive button
/// look like an accent — so this is the only anchor not taken from the TUI.
pub const RED: u32 = 0xF7_CD_CD;

/// The steps a Tailwind colour family has.
pub const STEPS: [u16; 11] = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];

/// Where the anchor sits in its own ramp. A pastel is a surface colour, and
/// 200 is where surface colours are reached for.
const ANCHOR_STEP: usize = 2;

/// Which Tailwind families each anchor claims.
///
/// Every default family is covered deliberately: an unmapped one would fall
/// through to stock Tailwind and be the single off-brand thing on the page.
pub const FAMILIES: &[(&str, u32)] = &[
    ("slate", WHITE),
    ("gray", WHITE),
    ("zinc", WHITE),
    ("neutral", WHITE),
    ("stone", WHITE),
    ("red", RED),
    ("orange", YELLOW),
    ("amber", YELLOW),
    ("yellow", YELLOW),
    ("lime", MINT),
    ("green", MINT),
    ("emerald", MINT),
    ("teal", MINT),
    ("cyan", BLUE),
    ("sky", BLUE),
    ("blue", BLUE),
    ("indigo", BLUE),
    ("violet", BLUSH),
    ("purple", BLUSH),
    ("fuchsia", PINK),
    ("pink", PINK),
    ("rose", PINK),
];

/// The surface the dark end of a ramp is read against.
const LIGHT_SURFACE: u32 = 0xFF_FF_FF;

/// Contrast each text-weight step must reach on a light surface.
///
/// These are solved for rather than assumed, because a fixed lightness curve
/// cannot hold contrast across hues: WCAG weights green at 0.72 and blue at
/// 0.07, so the same HSL lightness makes green far brighter in the only measure
/// that matters here. Hand-tuning the curve fixed one family and broke another;
/// solving fixes all of them and keeps holding if a colour ever changes.
/// Contrast on white for the steps that carry text and controls.
const DARK_TARGETS: [f64; 6] = [2.7, 3.6, 4.6, 7.0, 10.0, 14.0];

/// Expand one anchor into its eleven steps.
///
/// Every step is solved to a contrast target rather than a lightness, and the
/// targets are laid out geometrically from the anchor's own contrast down to
/// the fixed text weights. Generating part of the ramp by lightness and part by
/// contrast left a seam wherever the two happened to meet: pink's 400 and 500
/// came out one RGB point apart while every other family was fine.
pub fn ramp(anchor: u32) -> [u32; 11] {
    let (hue, saturation, _) = to_hsl(anchor);
    let mut out = [0u32; 11];
    for (index, target) in contrast_targets(anchor).into_iter().enumerate() {
        // The anchor is used verbatim so a surface is exactly the terminal's
        // colour, not a re-derivation that rounds away from it.
        out[index] = match index {
            // The anchor is used verbatim so a surface is exactly the
            // terminal's colour, not a re-derivation that rounds away from it.
            ANCHOR_STEP => anchor,
            // Above it, mix toward white by fixed fractions. Contrast-solving
            // here cannot work: an off-white anchor is so close to white that
            // two targets round to the same 8-bit colour, and the ramp stalls.
            0 => mix(anchor, LIGHT_SURFACE, 0.72),
            1 => mix(anchor, LIGHT_SURFACE, 0.42),
            _ => darkest_readable(hue, saturation, target),
        };
    }
    out
}

/// Blend two colours, `amount` of the way from `from` to `to`.
fn mix(from: u32, to: u32, amount: f64) -> u32 {
    let channel = |shift: u32| {
        let a = (from >> shift & 0xFF) as f64;
        let b = (to >> shift & 0xFF) as f64;
        (a + (b - a) * amount).round().clamp(0.0, 255.0) as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

/// The contrast-on-white each step aims for.
fn contrast_targets(anchor: u32) -> [f64; 11] {
    let at_anchor = contrast(anchor, LIGHT_SURFACE);
    let mut targets = [1.0; 11];

    // Above the anchor: approach white proportionally, so a near-white family
    // stays subtle instead of being forced into visible grey banding.
    targets[ANCHOR_STEP] = at_anchor;

    // Between the anchor and the first text weight, step geometrically: equal
    // ratios read as equal jumps far better than equal differences do.
    let first_dark = DARK_TARGETS[0];
    let span = (first_dark / at_anchor).max(1.0);
    targets[3] = at_anchor * span.powf(1.0 / 3.0);
    targets[4] = at_anchor * span.powf(2.0 / 3.0);

    targets[5..].copy_from_slice(&DARK_TARGETS);
    targets
}

/// The lightest shade of `hue` that still reaches `target` contrast on white.
///
/// Lightest rather than merely sufficient: an over-dark step would read as
/// near-black and lose the hue entirely, which defeats having a palette.
fn darkest_readable(hue: f64, saturation: f64, target: f64) -> u32 {
    let (mut low, mut high) = (0.02_f64, 0.95_f64);
    let at = |lightness: f64| from_hsl(hue, saturated_for(saturation, lightness), lightness);
    // Contrast against white falls as lightness rises, so this is monotonic.
    for _ in 0..40 {
        let middle = (low + high) / 2.0;
        if contrast(at(middle), LIGHT_SURFACE) >= target {
            low = middle;
        } else {
            high = middle;
        }
    }
    at(low)
}

/// How saturated a step should be.
///
/// A pastel is barely saturated. Darkening it without compensating gives mud —
/// every dark step across every family converging on the same grey-brown — so
/// saturation rises as lightness falls, and is capped short of fluorescent.
///
/// The compensation is scaled by how much colour the anchor had to begin with.
/// Artist's off-white is only faintly warm, and boosting it like a real hue
/// turned the whole neutral family olive: greys have to stay grey.
fn saturated_for(base: f64, lightness: f64) -> f64 {
    let darkness = ((0.62 - lightness) / 0.46).clamp(0.0, 1.0);
    // Below ~0.12 saturation an anchor is a neutral, and boosting it does not
    // read as "more colourful", it reads as dirty: artist's off-white came out
    // khaki, which is the wrong colour for the body text and borders that the
    // grey families actually supply. Neutrals desaturate as they darken; real
    // hues gain saturation so they do not turn to mud.
    let chromatic = ((base - 0.12) / 0.20).clamp(0.0, 1.0);
    let target = if chromatic < 0.35 {
        // A neutral is driven almost to grey rather than merely held back.
        // Artist's off-white sits at hue 48°, where even a sixth of saturation
        // reads as tan — and these families supply body text and borders, which
        // must not look dirty.
        base.min(0.06)
    } else {
        (base * (0.55 + 1.95 * chromatic)).min(0.62)
    };
    // A real hue only needs compensating once it darkens; a neutral needs
    // correcting the whole way down, because the warmth is already visible at
    // the mid steps where borders and muted text live.
    let blend = if chromatic < 0.35 { 1.0 } else { darkness };
    (base + (target - base) * blend).clamp(0.0, 0.85)
}

/// WCAG relative luminance.
pub fn luminance(color: u32) -> f64 {
    let channel = |value: u32| {
        let value = value as f64 / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color >> 16 & 0xFF)
        + 0.7152 * channel(color >> 8 & 0xFF)
        + 0.0722 * channel(color & 0xFF)
}

/// WCAG contrast ratio between two colours, 1.0 to 21.0.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    let (lighter, darker) = if a > b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

fn to_hsl(color: u32) -> (f64, f64, f64) {
    let red = (color >> 16 & 0xFF) as f64 / 255.0;
    let green = (color >> 8 & 0xFF) as f64 / 255.0;
    let blue = (color & 0xFF) as f64 / 255.0;

    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    let delta = max - min;

    if delta.abs() < f64::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == red {
        60.0 * (((green - blue) / delta) % 6.0)
    } else if max == green {
        60.0 * ((blue - red) / delta + 2.0)
    } else {
        60.0 * ((red - green) / delta + 4.0)
    };
    ((hue + 360.0) % 360.0, saturation, lightness)
}

fn from_hsl(hue: f64, saturation: f64, lightness: f64) -> u32 {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let secondary = chroma * (1.0 - (((hue / 60.0) % 2.0) - 1.0).abs());
    let match_value = lightness - chroma / 2.0;

    let (red, green, blue) = match hue as u32 / 60 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let byte = |value: f64| ((value + match_value) * 255.0).round().clamp(0.0, 255.0) as u32;
    byte(red) << 16 | byte(green) << 8 | byte(blue)
}

pub fn hex(color: u32) -> String {
    format!("#{color:06x}")
}

/// The `@theme` block Tailwind reads its palette from.
///
/// Injected into every canvas, so a utility class the model writes without
/// thinking resolves to artist's colours.
pub fn theme_css() -> String {
    let mut out = String::from("@theme {\n");
    for (family, anchor) in FAMILIES {
        for (step, color) in STEPS.iter().zip(ramp(*anchor)) {
            out.push_str(&format!("  --color-{family}-{step}: {};\n", hex(color)));
        }
    }
    // Semantic aliases the kit and templates use, so a canvas can say what it
    // means rather than picking a hue.
    out.push_str(&format!(
        "  --color-accent: {};\n  --color-danger: {};\n  --color-warning: {};\n  --color-success: {};\n",
        hex(ramp(BLUE)[7]),
        hex(ramp(RED)[7]),
        hex(ramp(YELLOW)[7]),
        hex(ramp(MINT)[7]),
    ));
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(anchor: u32, step: u16) -> u32 {
        let index = STEPS.iter().position(|s| *s == step).expect("known step");
        ramp(anchor)[index]
    }

    const WHITE_SURFACE: u32 = 0xFF_FF_FF;
    const DARK_SURFACE: u32 = 0x0B_0B_0E;

    fn anchors() -> [(&'static str, u32); 7] {
        [
            ("pink", PINK),
            ("white", WHITE),
            ("mint", MINT),
            ("yellow", YELLOW),
            ("blue", BLUE),
            ("blush", BLUSH),
            ("red", RED),
        ]
    }

    /// The whole reason for the ramp. Text weights have to clear AA on the
    /// surface they sit on, or the theme is pretty and unreadable.
    #[test]
    fn text_weights_are_readable_on_a_light_surface() {
        for (name, anchor) in anchors() {
            for weight in [700, 800, 900, 950] {
                let color = step(anchor, weight);
                let ratio = contrast(color, WHITE_SURFACE);
                assert!(
                    ratio >= 4.5,
                    "{name}-{weight} ({}) is {ratio:.2}:1 on white, needs 4.5",
                    hex(color)
                );
            }
        }
    }

    /// Dark mode inverts which end of the ramp carries text.
    #[test]
    fn light_weights_are_readable_on_a_dark_surface() {
        for (name, anchor) in anchors() {
            for weight in [100, 200, 300] {
                let color = step(anchor, weight);
                let ratio = contrast(color, DARK_SURFACE);
                assert!(
                    ratio >= 4.5,
                    "{name}-{weight} ({}) is {ratio:.2}:1 on dark, needs 4.5",
                    hex(color)
                );
            }
        }
    }

    /// The commonest pairing a model writes: tinted surface, dark text of the
    /// same family.
    #[test]
    fn the_common_tinted_pairing_is_readable() {
        for (name, anchor) in anchors() {
            let surface = step(anchor, 100);
            let text = step(anchor, 900);
            let ratio = contrast(text, surface);
            assert!(
                ratio >= 4.5,
                "{name}-900 on {name}-100 is {ratio:.2}:1, needs 4.5"
            );
        }
    }

    /// Adjacent steps have to be *visibly* apart, not merely ordered. Solving
    /// 600 for contrast while leaving 500 on the lightness curve once put pink
    /// 500 and 600 within one RGB point of each other, which would have made
    /// every `hover:bg-*-600` a no-op.
    #[test]
    fn adjacent_steps_are_visibly_different() {
        for (name, anchor) in anchors() {
            let ramp = ramp(anchor);
            // Checked from the anchor down. Above it the steps are genuinely
            // near-white — an off-white family cannot separate 50 from 100 and
            // should not try, or it bands.
            for (index, pair) in ramp.windows(2).enumerate().skip(ANCHOR_STEP) {
                let ratio = contrast(pair[0], pair[1]);
                assert!(
                    ratio >= 1.12,
                    "{name} {} and {} are indistinguishable ({ratio:.3}:1)",
                    STEPS[index],
                    STEPS[index + 1]
                );
            }
        }
    }

    /// A ramp has to actually descend, or `-600` and `-700` are the same colour
    /// and every hover state disappears.
    #[test]
    fn each_ramp_is_monotonically_darker() {
        for (name, anchor) in anchors() {
            let ramp = ramp(anchor);
            for pair in ramp.windows(2) {
                assert!(
                    luminance(pair[0]) > luminance(pair[1]),
                    "{name} ramp does not descend: {} then {}",
                    hex(pair[0]),
                    hex(pair[1])
                );
            }
        }
    }

    /// Families must stay distinguishable. If saturation compensation pushed
    /// every dark step toward the same grey, the palette would collapse.
    #[test]
    fn families_remain_distinct_at_text_weight() {
        let families = [("mint", MINT), ("blue", BLUE), ("pink", PINK), ("yellow", YELLOW)];
        for (index, (left_name, left)) in families.iter().enumerate() {
            for (right_name, right) in &families[index + 1..] {
                let left = step(*left, 700);
                let right = step(*right, 700);
                let difference = (0..3)
                    .map(|shift| {
                        let channel = |c: u32| (c >> (shift * 8) & 0xFF) as i32;
                        (channel(left) - channel(right)).abs()
                    })
                    .sum::<i32>();
                assert!(
                    difference > 30,
                    "{left_name}-700 and {right_name}-700 are nearly identical: {} vs {}",
                    hex(left),
                    hex(right)
                );
            }
        }
    }

    /// A pastel surface should still look like the terminal's colour.
    #[test]
    fn the_light_end_stays_recognisably_the_anchor() {
        for (name, anchor) in anchors() {
            let surface = step(anchor, 200);
            assert!(
                contrast(surface, anchor) < 1.3,
                "{name}-200 ({}) drifted from its anchor ({})",
                hex(surface),
                hex(anchor)
            );
        }
    }

    /// Every default Tailwind family must be claimed; one left unmapped would
    /// be the single stock-blue element on an otherwise themed page.
    #[test]
    fn every_default_family_is_remapped() {
        let expected = [
            "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber", "yellow",
            "lime", "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet",
            "purple", "fuchsia", "pink", "rose",
        ];
        for family in expected {
            assert!(
                FAMILIES.iter().any(|(name, _)| *name == family),
                "{family} would fall through to stock Tailwind"
            );
        }
    }

    /// Danger must not read as the accent, or a destructive button looks safe.
    #[test]
    fn danger_is_distinguishable_from_the_accent() {
        assert!(
            contrast(step(RED, 700), step(BLUE, 700)) > 1.4
                || (step(RED, 700) >> 16) as i32 - (step(BLUE, 700) >> 16) as i32 > 40,
            "red-700 and blue-700 are too close to tell apart"
        );
    }

    #[test]
    fn the_theme_block_declares_every_step() {
        let css = theme_css();
        assert!(css.starts_with("@theme {"));
        assert!(css.contains("--color-blue-700:"));
        assert!(css.contains("--color-accent:"));
        for (family, _) in FAMILIES {
            for step in STEPS {
                assert!(
                    css.contains(&format!("--color-{family}-{step}:")),
                    "{family}-{step} missing from the theme block"
                );
            }
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// Not an assertion — a way to look at the ramps.
    /// `cargo test -p artist-canvas -- --ignored --nocapture show_ramps`
    #[test]
    #[ignore]
    fn show_ramps() {
        for (name, anchor) in [
            ("pink", PINK), ("blush", BLUSH), ("mint", MINT),
            ("yellow", YELLOW), ("blue", BLUE), ("white", WHITE), ("red", RED),
        ] {
            print!("{name:>7} ");
            for (step, color) in STEPS.iter().zip(ramp(anchor)) {
                let (r, g, b) = (color >> 16 & 0xFF, color >> 8 & 0xFF, color & 0xFF);
                let fg = if luminance(color) > 0.4 { "30" } else { "97" };
                print!("\x1b[48;2;{r};{g};{b}m\x1b[{fg}m {step:>3} \x1b[0m");
            }
            println!("  anchor {}", hex(anchor));
        }
        println!("\ncontrast on white: 700 =");
        for (name, anchor) in [("mint", MINT), ("blue", BLUE), ("yellow", YELLOW), ("white", WHITE)] {
            let c = ramp(anchor)[7];
            println!("  {name:>7}-700 {} {:.2}:1", hex(c), contrast(c, 0xFFFFFF));
        }
    }
}

/// The kit's semantic tokens, derived from the same ramps.
///
/// `@artist/ui` shipped with an invented palette — a stock blue accent and
/// zinc greys — which meant a canvas looked like neither artist nor itself.
/// Generating them here keeps one source of truth for both the utilities a
/// model types and the components it composes.
pub fn tokens_css() -> String {
    let blue = ramp(BLUE);
    let neutral = ramp(WHITE);
    let mint = ramp(MINT);
    let yellow = ramp(YELLOW);
    let red = ramp(RED);

    format!(
        ":root {{\n\
         \x20 --a-bg: #ffffff;\n\
         \x20 --a-fg: {fg};\n\
         \x20 --a-muted: {muted};\n\
         \x20 --a-subtle: {subtle};\n\
         \x20 --a-border: {border};\n\
         \x20 --a-accent: {accent};\n\
         \x20 --a-accent-fg: #ffffff;\n\
         \x20 --a-danger: {danger};\n\
         \x20 --a-ok: {ok};\n\
         \x20 --a-warn: {warn};\n\
         \x20 --a-chart-1: {chart1};\n\
         \x20 --a-chart-2: {chart2};\n\
         \x20 --a-chart-3: {chart3};\n\
         \x20 --a-chart-4: {chart4};\n\
         \x20 --a-chart-5: {chart5};\n\
         \x20 --a-radius: 8px;\n\
         \x20 --a-sp: 4px;\n\
         \x20 --a-font: ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif;\n\
         \x20 --a-mono: ui-monospace, SFMono-Regular, Menlo, \"Cascadia Code\", monospace;\n\
         }}\n\
         @media (prefers-color-scheme: dark) {{\n\
         \x20 :root {{\n\
         \x20   --a-bg: {dark_bg};\n\
         \x20   --a-fg: {dark_fg};\n\
         \x20   --a-muted: {dark_muted};\n\
         \x20   --a-subtle: {dark_subtle};\n\
         \x20   --a-border: {dark_border};\n\
         \x20   --a-accent: {dark_accent};\n\
         \x20   --a-accent-fg: {dark_accent_fg};\n\
         \x20   --a-danger: {dark_danger};\n\
         \x20   --a-ok: {dark_ok};\n\
         \x20   --a-warn: {dark_warn};\n\
         \x20 }}\n\
         }}\n",
        fg = hex(neutral[10]),
        muted = hex(neutral[7]),
        subtle = hex(neutral[0]),
        border = hex(neutral[2]),
        accent = hex(blue[7]),
        danger = hex(red[7]),
        ok = hex(mint[7]),
        warn = hex(yellow[7]),
        chart1 = hex(blue[6]),
        chart2 = hex(mint[6]),
        chart3 = hex(ramp(PINK)[6]),
        chart4 = hex(yellow[6]),
        chart5 = hex(ramp(BLUSH)[6]),
        // Dark mode reads off the opposite end: the pastels themselves become
        // the foregrounds they were designed to be in the terminal.
        dark_bg = hex(mix(neutral[10], 0x00_00_00, 0.55)),
        dark_fg = hex(neutral[1]),
        dark_muted = hex(neutral[4]),
        dark_subtle = hex(mix(neutral[10], 0x00_00_00, 0.25)),
        dark_border = hex(neutral[9]),
        dark_accent = hex(blue[2]),
        // The accent flips to a light pastel in dark mode, so what sits on top
        // of it has to flip too — white on pastel is about 1.3:1.
        dark_accent_fg = hex(neutral[10]),
        dark_danger = hex(red[3]),
        dark_ok = hex(mint[2]),
        dark_warn = hex(yellow[2]),
    )
}

/// Typographic and shape defaults, so an element the model never styled still
/// looks like it belongs.
pub const BASE_CSS: &str = "\
*, *::before, *::after { box-sizing: border-box; }
body { margin: 0; background: var(--a-bg); color: var(--a-fg);
       font-family: var(--a-font); font-size: 14px; line-height: 1.6;
       -webkit-font-smoothing: antialiased; }
h1 { font-size: 1.5rem; line-height: 1.25; font-weight: 650; margin: 0 0 .5rem; letter-spacing: -0.01em; }
h2 { font-size: 1.2rem; line-height: 1.3; font-weight: 600; margin: 1.25rem 0 .4rem; }
h3 { font-size: 1rem;  line-height: 1.4; font-weight: 600; margin: 1rem 0 .3rem; }
p  { margin: 0 0 .75rem; }
a  { color: var(--a-accent); text-underline-offset: 2px; }
code, pre, kbd, samp { font-family: var(--a-mono); font-size: .875em; }
:where(button, input, select, textarea) { font: inherit; color: inherit; }
:where(button) { border-radius: var(--a-radius); }
:where(input, select, textarea) { border-radius: var(--a-radius);
  border: 1px solid var(--a-border); background: var(--a-bg); padding: .375rem .5rem; }
:where(table) { border-collapse: collapse; }
:where(hr) { border: 0; border-top: 1px solid var(--a-border); margin: 1rem 0; }
::selection { background: var(--a-accent); color: var(--a-accent-fg); }
:focus-visible { outline: 2px solid var(--a-accent); outline-offset: 2px; }
";

#[cfg(test)]
mod token_tests {
    use super::*;

    /// The kit and the utilities must agree, or a Card sits on one blue and a
    /// `bg-blue-100` div sits on another.
    #[test]
    fn kit_tokens_come_from_the_same_ramps() {
        let tokens = tokens_css();
        assert!(tokens.contains(&hex(ramp(BLUE)[7])), "accent is not blue-700");
        assert!(tokens.contains(&hex(ramp(RED)[7])), "danger is not red-700");
        assert!(!tokens.contains("#2563eb"), "the invented accent survived");
        assert!(!tokens.contains("#18181b"), "the invented neutral survived");
    }

    /// Body text has to be readable on the surface it is painted on, in both
    /// schemes — the same guarantee the ramps give the utilities.
    #[test]
    fn body_text_is_readable_in_both_schemes() {
        let neutral = ramp(WHITE);
        let light = contrast(neutral[10], 0xFF_FF_FF);
        assert!(light >= 4.5, "light-mode body text is {light:.2}:1");

        let dark_bg = mix(neutral[10], 0x00_00_00, 0.55);
        let dark = contrast(neutral[1], dark_bg);
        assert!(dark >= 4.5, "dark-mode body text is {dark:.2}:1");
    }

    /// A filled button is a foreground on an accent, and the accent flips
    /// between schemes. Leaving the text colour behind made "Apply" white on
    /// pastel blue — invisible — in dark mode.
    #[test]
    fn text_on_a_filled_control_is_readable_in_both_schemes() {
        let blue = ramp(BLUE);
        let red = ramp(RED);
        let neutral = ramp(WHITE);

        for (name, accent) in [("accent", blue[7]), ("danger", red[7])] {
            let ratio = contrast(0xFF_FF_FF, accent);
            assert!(ratio >= 4.5, "light-mode {name} button text is {ratio:.2}:1");
        }
        for (name, accent) in [("accent", blue[2]), ("danger", red[3])] {
            let ratio = contrast(neutral[10], accent);
            assert!(ratio >= 4.5, "dark-mode {name} button text is {ratio:.2}:1");
        }
    }

    #[test]
    fn muted_text_still_clears_the_bar() {
        let neutral = ramp(WHITE);
        // Muted is dimmer than body text but is still text, so it takes the
        // 700 weight rather than the 600 used for borders and large type.
        let light = contrast(neutral[7], 0xFF_FF_FF);
        assert!(light >= 4.5, "muted text is {light:.2}:1 on white");
        assert!(
            contrast(neutral[10], 0xFF_FF_FF) > light,
            "muted should be dimmer than body text"
        );
    }

    #[test]
    fn the_base_layer_defines_type_and_shape() {
        for expected in ["--a-font", "box-sizing", "h1 {", ":focus-visible"] {
            assert!(BASE_CSS.contains(expected), "{expected} missing from the base layer");
        }
    }
}
