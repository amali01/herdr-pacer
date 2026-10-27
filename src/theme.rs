//! The colors of Herdr's theme that pacer's sidebar text takes, so it reads
//! like Herdr's own: an agent's name in subtext0, as Herdr draws one, and a
//! usage line that gets high in peach, yellow and red. Herdr's `fg` takes
//! only hex, so the palettes are copied from Herdr 0.9.5 (src/app/state.rs).
//! shortcut: with `auto_switch` the colors follow `name`, not the terminal's
//! light or dark; a theme Herdr adds later draws in catppuccin until copied.

/// subtext0, peach, yellow, red of each built-in theme.
const THEMES: [(&str, [&str; 4]); 17] = [
    ("catppuccin", ["#a6adc8", "#fab387", "#f9e2af", "#f38ba8"]),
    ("catppuccin-latte", ["#6c6f85", "#fe640b", "#df8e1d", "#d20f39"]),
    ("tokyo-night", ["#a9b1d6", "#ff9e64", "#e0af68", "#f7768e"]),
    ("tokyo-night-day", ["#6172b0", "#b15c00", "#8c6c3e", "#f52a65"]),
    ("dracula", ["#d2d2dc", "#ffb86c", "#f1fa8c", "#ff5555"]),
    ("nord", ["#d8dee9", "#d08770", "#ebcb8b", "#bf616a"]),
    ("gruvbox", ["#d5c4a1", "#fe8019", "#fabd2f", "#fb4934"]),
    ("gruvbox-light", ["#504945", "#af3a03", "#b57614", "#9d0006"]),
    ("one-dark", ["#969ca8", "#d19a66", "#e5c07b", "#e06c75"]),
    ("one-light", ["#686b77", "#986801", "#c18401", "#e45649"]),
    ("solarized", ["#839496", "#cb4b16", "#b58900", "#dc322f"]),
    ("solarized-light", ["#839496", "#cb4b16", "#b58900", "#dc322f"]),
    ("kanagawa", ["#c8c3aa", "#ffa066", "#c0a36e", "#c34043"]),
    ("kanagawa-lotus", ["#43436c", "#cc6d00", "#77713f", "#c84053"]),
    ("rose-pine", ["#c8c5dc", "#ea9a97", "#f6c177", "#eb6f92"]),
    ("rose-pine-dawn", ["#797593", "#d7827e", "#ea9d34", "#b4637a"]),
    ("vesper", ["#a0a0a0", "#ffc799", "#ffc799", "#ff8080"]),
];

/// The colors by name, none on the `terminal` theme: it draws in the host
/// terminal's palette, which hex cannot name, so Herdr's defaults stand.
#[derive(Debug, Default, PartialEq)]
pub struct Palette {
    pub subtext0: Option<String>,
    pub peach: Option<String>,
    pub yellow: Option<String>,
    pub red: Option<String>,
}

/// Herdr's own spellings of a theme's name.
fn canonical(name: &str) -> &'static str {
    match name.to_lowercase().replace([' ', '_'], "-").as_str() {
        "catppuccin-mocha" => "catppuccin",
        "latte" | "light" => "catppuccin-latte",
        "tokyonight" => "tokyo-night",
        "tokyo-day" | "tokyonight-day" => "tokyo-night-day",
        "gruvbox-dark" => "gruvbox",
        "onedark" => "one-dark",
        "onelight" => "one-light",
        "solarized-dark" => "solarized",
        "lotus" => "kanagawa-lotus",
        "rosepine" => "rose-pine",
        "rosepine-dawn" | "dawn" => "rose-pine-dawn",
        n => THEMES.iter().map(|t| t.0).chain(["terminal"]).find(|&t| t == n).unwrap_or("catppuccin"),
    }
}

/// The palette of a config.toml's `[theme]`: its `name`, then whatever hex
/// `[theme.custom]` puts over it.
pub fn of(theme: Option<&toml_edit::Item>) -> Palette {
    let name = canonical(theme.and_then(|t| t.get("name")).and_then(|n| n.as_str()).unwrap_or("catppuccin"));
    let base = THEMES.iter().find(|t| t.0 == name).map(|t| t.1.map(String::from));
    // only the hex a sidebar `fg` takes: anything else would make Herdr refuse the whole config
    let hex = |c: &&str| c.strip_prefix('#').is_some_and(|h| matches!(h.len(), 3 | 6) && h.chars().all(|d| d.is_ascii_hexdigit()));
    let custom = |k: &str| theme?.get("custom")?.get(k)?.as_str().filter(hex).map(String::from);
    let pick = |i: usize, k: &str| custom(k).or_else(|| base.as_ref().map(|b| b[i].clone()));
    Palette { subtext0: pick(0, "subtext0"), peach: pick(1, "peach"), yellow: pick(2, "yellow"), red: pick(3, "red") }
}

/// The palette of the theme config.toml picks.
pub fn current() -> Palette {
    let config = std::fs::read_to_string(crate::setup::config_path()).unwrap_or_default();
    of(config.parse::<toml_edit::DocumentMut>().ok().as_ref().and_then(|d| d.get("theme")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(toml: &str) -> Palette {
        let doc: toml_edit::DocumentMut = toml.parse().unwrap();
        of(doc.get("theme"))
    }

    #[test]
    fn a_theme_and_its_overrides_pick_the_colors() {
        assert_eq!(theme("[theme]\nname = \"tokyo-night\"\n").subtext0.as_deref(), Some("#a9b1d6"));
        assert_eq!(theme("[theme]\nname = \"TokyoNight\"\n").red.as_deref(), Some("#f7768e"), "Herdr's spellings");
        assert_eq!(theme("[theme]\nname = \"Rose Pine\"\n").subtext0.as_deref(), Some("#c8c5dc"));
        assert_eq!(theme("").subtext0.as_deref(), Some("#a6adc8"), "catppuccin when none is named");
        assert_eq!(theme("[theme]\nname = \"terminal\"\n"), Palette::default(), "the terminal's own palette");
        let custom = theme("[theme]\nname = \"nord\"\n[theme.custom]\nred = \"#123456\"\nyellow = \"yellow\"\npeach = \"#oops\\\"\"\nsubtext0 = \"#abc\"\n");
        assert_eq!((custom.red.as_deref(), custom.yellow.as_deref()), (Some("#123456"), Some("#ebcb8b")), "only hex overrides");
        assert_eq!((custom.peach.as_deref(), custom.subtext0.as_deref()), (Some("#d08770"), Some("#abc")), "malformed hex never reaches the rows");
    }
}
