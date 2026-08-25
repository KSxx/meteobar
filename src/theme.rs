use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeColors {
    pub border: String,
    pub text: String,
    pub dim: String,
    pub accent: String,
    pub green: String,
    pub yellow: String,
    pub orange: String,
    pub error: String,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            border: "#61afef".into(),
            text: "#abb2bf".into(),
            dim: "#5c6370".into(),
            accent: "#61afef".into(),
            green: "#98c379".into(),
            yellow: "#e5c07b".into(),
            orange: "#d19a66".into(),
            error: "#e06c75".into(),
        }
    }
}

const THEME_SUFFIX: &str = "omarchy/current/theme/colors.toml";

/// pywal's cache file. The original pywal (dylanaraps) is archived, but this
/// path is the de-facto standard: the maintained `pywal16` fork writes the
/// same file, and `wallust` can target it for pywal compatibility — so one
/// path covers all three ecosystems.
const PYWAL_SUFFIX: &str = "wal/colors.json";

/// Resolve an XDG base directory: the env var when set and non-empty,
/// otherwise the conventional path under `$HOME`. Split from the env lookup
/// so tests can exercise the precedence without mutating the environment.
fn dir_from(
    env_value: Option<OsString>,
    home: Option<PathBuf>,
    home_suffix: &str,
) -> Option<PathBuf> {
    match env_value {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => home.map(|home| home.join(home_suffix)),
    }
}

fn resolve_dir(env_var: &str, home_suffix: &str) -> Option<PathBuf> {
    dir_from(std::env::var_os(env_var), dirs::home_dir(), home_suffix)
}

/// Directory Omarchy keeps the active theme under: `$XDG_STATE_HOME`, or
/// `~/.local/state` when unset. This is the current location; the old
/// `~/.config` path is kept as a fallback for legacy installs.
fn state_dir() -> Option<PathBuf> {
    resolve_dir("XDG_STATE_HOME", ".local/state")
}

/// Candidate colors.toml locations, most current first.
fn candidate_paths(state: Option<PathBuf>, config: Option<PathBuf>) -> Vec<PathBuf> {
    [state, config]
        .into_iter()
        .flatten()
        .map(|dir| dir.join(THEME_SUFFIX))
        .collect()
}

/// First existing colors.toml, or None when no Omarchy theme is installed —
/// the caller then falls through to pywal, and finally to the built-in palette.
fn theme_colors_file() -> Option<PathBuf> {
    candidate_paths(state_dir(), dirs::config_dir())
        .into_iter()
        .find(|path| path.is_file())
}

fn pywal_path(cache: Option<PathBuf>) -> Option<PathBuf> {
    cache.map(|dir| dir.join(PYWAL_SUFFIX))
}

/// pywal's colors.json when one exists.
fn pywal_colors_file() -> Option<PathBuf> {
    pywal_path(resolve_dir("XDG_CACHE_HOME", ".cache")).filter(|path| path.is_file())
}

/// pywal cache document. Both sections are optional and read as plain string
/// maps so a partial or unusual file still yields whatever keys it does have.
#[derive(Deserialize, Default)]
struct PywalColors {
    #[serde(default)]
    special: HashMap<String, String>,
    #[serde(default)]
    colors: HashMap<String, String>,
}

impl ThemeColors {
    /// Theme resolution chain: Omarchy theme (state dir, then the legacy
    /// config dir) → pywal cache → built-in One Dark defaults. pywal is only
    /// consulted when no Omarchy theme was found.
    pub fn load() -> Self {
        Self::load_from(theme_colors_file(), pywal_colors_file())
    }

    fn load_from(omarchy: Option<PathBuf>, pywal: Option<PathBuf>) -> Self {
        if let Some(path) = omarchy {
            if let Ok(content) =
                crate::safe_read::read_bounded(&path, crate::safe_read::CONFIG_LIMIT)
            {
                return Self::from_toml(&content);
            }
        }
        if let Some(path) = pywal {
            if let Ok(content) =
                crate::safe_read::read_bounded(&path, crate::safe_read::CONFIG_LIMIT)
            {
                return Self::from_pywal_json(&content);
            }
        }
        Self::default()
    }

    fn from_toml(content: &str) -> Self {
        Self::from_map(&parse_toml_flat(content))
    }

    /// Invalid JSON degrades to the built-in palette, like a missing file.
    fn from_pywal_json(content: &str) -> Self {
        match serde_json::from_str::<PywalColors>(content) {
            Ok(parsed) => Self::from_pywal(&parsed),
            Err(_) => Self::default(),
        }
    }

    /// Map a pywal palette onto widget colors, field by field: anything the
    /// file omits — or spells with a non-hex value — keeps its default.
    fn from_pywal(parsed: &PywalColors) -> Self {
        let mut colors = Self::default();
        let special = |key: &str| {
            parsed
                .special
                .get(key)
                .map(String::as_str)
                .filter(|value| is_hex_color(value))
        };
        let color = |key: &str| {
            parsed
                .colors
                .get(key)
                .map(String::as_str)
                .filter(|value| is_hex_color(value))
        };

        if let Some(foreground) = special("foreground") {
            colors.text = foreground.to_string();
        }
        if let (Some(foreground), Some(background)) = (special("foreground"), special("background"))
        {
            colors.dim = blend_hex(foreground, background, 0.5);
        }
        // pywal has no accent slot; color4 is its blue, with the cursor color
        // as the fallback highlight.
        if let Some(accent) = color("color4").or_else(|| special("cursor")) {
            colors.border = accent.to_string();
            colors.accent = accent.to_string();
        }
        if let Some(green) = color("color2") {
            colors.green = green.to_string();
        }
        if let Some(yellow) = color("color3") {
            colors.yellow = yellow.to_string();
        }
        if let Some(red) = color("color1") {
            colors.error = red.to_string();
        }
        // Orange has no pywal slot either. Synthesize it as the midpoint of
        // yellow and red rather than aliasing it to red, which would flatten
        // any gauge that reads orange and red as distinct steps. Needs both
        // ends: with either missing, orange keeps its default.
        if let (Some(yellow), Some(red)) = (color("color3"), color("color1")) {
            colors.orange = blend_hex(yellow, red, 0.5);
        }
        colors
    }

    /// Map theme colors onto widget colors. Every field degrades on its own:
    /// a key the theme does not define keeps its built-in default instead of
    /// sinking the whole load. Named keys — how current Omarchy themes ship
    /// their palette — win; the `color1/2/3` fallbacks keep legacy themes
    /// working exactly as before (where `color1` stood in for red).
    fn from_map(map: &HashMap<String, String>) -> Self {
        let mut colors = Self::default();
        let get = |key: &str| map.get(key).filter(|value| is_hex_color(value));
        let first = |keys: &[&str]| keys.iter().find_map(|key| get(key)).cloned();

        if let Some(accent) = first(&["accent"]) {
            colors.border = accent.clone();
            colors.accent = accent;
        }
        if let Some(foreground) = first(&["foreground"]) {
            colors.text = foreground;
        }
        if let (Some(foreground), Some(background)) = (get("foreground"), get("background")) {
            colors.dim = blend_hex(foreground, background, 0.5);
        }
        if let Some(green) = first(&["green", "color2"]) {
            colors.green = green;
        }
        if let Some(yellow) = first(&["yellow", "color3"]) {
            colors.yellow = yellow;
        }
        if let Some(orange) = first(&["orange", "red", "color1"]) {
            colors.orange = orange;
        }
        if let Some(error) = first(&["red", "color1"]) {
            colors.error = error;
        }
        colors
    }
}

fn parse_toml_flat(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().trim_matches('"').to_string();
            map.insert(key, value);
        }
    }
    map
}

/// Strict `#rgb` / `#rgba` / `#rrggbb` / `#rrggbbaa` check. Values that fail
/// it are ignored so a malformed theme key can never reach Pango markup.
fn is_hex_color(s: &str) -> bool {
    match s.strip_prefix('#') {
        Some(hex) => {
            matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// Parse every form `is_hex_color` accepts — `#rgb`, `#rgba`, `#rrggbb`,
/// `#rrggbbaa` — so derived colors are computed from the theme instead of
/// silently falling back to the built-in palette. Alpha is dropped: these
/// values end up in Pango `foreground` attributes, which are opaque.
fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let pair = |s: &str| u8::from_str_radix(s, 16).ok();
    let double = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();
    match hex.len() {
        3 | 4 => {
            let mut chars = hex.chars();
            Some((
                double(chars.next()?)?,
                double(chars.next()?)?,
                double(chars.next()?)?,
            ))
        }
        6 | 8 => Some((pair(&hex[0..2])?, pair(&hex[2..4])?, pair(&hex[4..6])?)),
        _ => None,
    }
}

fn to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Lightness of a color in HSL terms, 0.0-1.0.
fn lightness(color: &str) -> Option<f32> {
    let (r, g, b) = parse_hex(color)?;
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    Some((max + min) / 2.0)
}

fn hsl_to_hex(hue: f32, saturation: f32, lightness: f32) -> String {
    let c = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = (hue.rem_euclid(1.0)) * 6.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = lightness - c / 2.0;
    let channel = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    to_hex(channel(r), channel(g), channel(b))
}

/// One stop of a value→color ramp: the color, and the percentage it sits at.
/// Publishing the positions as well as the colors is what keeps the Waybar
/// tooltip and the QML panel on the same ramp — change a threshold here and
/// both frontends move together.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RampStop {
    pub pct: u8,
    pub color: String,
}

/// Color for `pct` on a ramp, interpolating between the surrounding stops.
/// Both frontends use this rule, so the same probability paints the same
/// color in the tooltip and in the panel.
pub fn ramp_color(stops: &[RampStop], pct: u8) -> String {
    let Some(first) = stops.first() else {
        return String::new();
    };
    if pct <= first.pct {
        return first.color.clone();
    }
    for pair in stops.windows(2) {
        let (low, high) = (&pair[0], &pair[1]);
        if pct <= high.pct {
            let span = (high.pct - low.pct) as f32;
            let ratio = if span <= 0.0 {
                1.0
            } else {
                (pct - low.pct) as f32 / span
            };
            return blend_hex(&low.color, &high.color, ratio);
        }
    }
    stops[stops.len() - 1].color.clone()
}

impl ThemeColors {
    /// Precipitation-probability ramp: calm below 30%, hedging through the
    /// middle, emphatic once rain is likely.
    pub fn precip_ramp(&self) -> Vec<RampStop> {
        vec![
            RampStop {
                pct: 0,
                color: self.green.clone(),
            },
            RampStop {
                pct: 30,
                color: self.yellow.clone(),
            },
            RampStop {
                pct: 60,
                color: self.accent.clone(),
            },
        ]
    }

    /// Cold/warm anchors for temperature read-outs. Fixed hues so they stay
    /// recognisable, but the theme foreground's lightness so they are legible
    /// on light and dark themes alike.
    fn thermal(&self, hue: f32) -> String {
        let l = lightness(&self.text).unwrap_or(0.5).clamp(0.32, 0.72);
        hsl_to_hex(hue, 0.55, l)
    }

    pub fn temp_cold(&self) -> String {
        self.thermal(0.58)
    }

    pub fn temp_warm(&self) -> String {
        self.thermal(0.02)
    }
}

fn blend_hex(c1: &str, c2: &str, ratio: f32) -> String {
    let (r1, g1, b1) = parse_hex(c1).unwrap_or((171, 178, 191));
    let (r2, g2, b2) = parse_hex(c2).unwrap_or((40, 44, 52));
    let blend =
        |a: u8, b: u8| -> u8 { (a as f32 * (1.0 - ratio) + b as f32 * ratio).round() as u8 };
    format!(
        "#{:02x}{:02x}{:02x}",
        blend(r1, r2),
        blend(g1, g2),
        blend(b1, b2)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // A current Omarchy theme (Tokyo Night): semantic keys only, no colorN.
    const MODERN_THEME: &str = r##"
mode = "dark"

accent = "#7aa2f7"
background = "#1a1b26"
foreground = "#a9b1d6"

red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
blue = "#7aa2f7"

bright_red = "#ff7a93"
"##;

    // A legacy theme: terminal-palette keys only.
    const LEGACY_THEME: &str = r##"
accent = "#61afef"
background = "#282c34"
foreground = "#abb2bf"
color1 = "#e06c75"
color2 = "#98c379"
color3 = "#e5c07b"
"##;

    #[test]
    fn state_path_is_preferred_over_config_path() {
        let paths = candidate_paths(
            Some(PathBuf::from("/home/u/.local/state")),
            Some(PathBuf::from("/home/u/.config")),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/u/.local/state/omarchy/current/theme/colors.toml"),
                PathBuf::from("/home/u/.config/omarchy/current/theme/colors.toml"),
            ]
        );
    }

    #[test]
    fn legacy_config_path_is_still_a_candidate() {
        // With no state dir resolvable, the old config path still gets tried.
        let paths = candidate_paths(None, Some(PathBuf::from("/home/u/.config")));
        assert_eq!(
            paths,
            vec![PathBuf::from(
                "/home/u/.config/omarchy/current/theme/colors.toml"
            )]
        );
        assert!(candidate_paths(None, None).is_empty());
    }

    #[test]
    fn named_keys_are_used() {
        let colors = ThemeColors::from_toml(MODERN_THEME);
        assert_eq!(colors.accent, "#7aa2f7");
        assert_eq!(colors.border, "#7aa2f7");
        assert_eq!(colors.text, "#a9b1d6");
        assert_eq!(colors.green, "#9ece6a");
        assert_eq!(colors.yellow, "#e0af68");
        assert_eq!(colors.orange, "#eb927b");
        assert_eq!(colors.error, "#f7768e");
        // Nothing was left at the built-in palette.
        assert_ne!(colors, ThemeColors::default());
    }

    #[test]
    fn missing_color1_no_longer_sinks_the_whole_load() {
        // The regression: a modern theme defines no colorN at all. The old
        // loader returned None here and every user silently got the defaults.
        let map = parse_toml_flat(MODERN_THEME);
        assert!(!map.contains_key("color1"));
        let colors = ThemeColors::from_map(&map);
        assert_eq!(colors.accent, "#7aa2f7");
        assert_eq!(colors.error, "#f7768e");
    }

    #[test]
    fn legacy_color_keys_are_used_when_named_are_absent() {
        let colors = ThemeColors::from_toml(LEGACY_THEME);
        assert_eq!(colors.green, "#98c379");
        assert_eq!(colors.yellow, "#e5c07b");
        // color1 stands in for both red and orange on legacy themes.
        assert_eq!(colors.orange, "#e06c75");
        assert_eq!(colors.error, "#e06c75");
    }

    #[test]
    fn named_keys_win_over_legacy_keys() {
        let theme = r##"
green = "#9ece6a"
yellow = "#e0af68"
orange = "#eb927b"
red = "#f7768e"
color1 = "#111111"
color2 = "#222222"
color3 = "#333333"
"##;
        let colors = ThemeColors::from_toml(theme);
        assert_eq!(colors.green, "#9ece6a");
        assert_eq!(colors.yellow, "#e0af68");
        assert_eq!(colors.orange, "#eb927b");
        assert_eq!(colors.error, "#f7768e");
    }

    #[test]
    fn orange_falls_back_to_red_before_color1() {
        let theme = r##"
red = "#f7768e"
color1 = "#111111"
"##;
        assert_eq!(ThemeColors::from_toml(theme).orange, "#f7768e");
    }

    #[test]
    fn no_theme_falls_back_to_defaults() {
        // Empty, comment-only, and non-TOML content must all degrade quietly.
        assert_eq!(ThemeColors::from_toml(""), ThemeColors::default());
        assert_eq!(
            ThemeColors::from_toml("# just a comment\n\n"),
            ThemeColors::default()
        );
        assert_eq!(
            ThemeColors::from_toml("<<<not toml at all>>>"),
            ThemeColors::default()
        );
        assert_eq!(
            ThemeColors::from_map(&HashMap::new()),
            ThemeColors::default()
        );
    }

    #[test]
    fn partial_theme_keeps_defaults_for_missing_fields() {
        let colors = ThemeColors::from_toml("accent = \"#7aa2f7\"\n");
        let default = ThemeColors::default();
        assert_eq!(colors.accent, "#7aa2f7");
        assert_eq!(colors.border, "#7aa2f7");
        // Untouched fields keep the built-in palette instead of vanishing.
        assert_eq!(colors.text, default.text);
        assert_eq!(colors.dim, default.dim);
        assert_eq!(colors.green, default.green);
        assert_eq!(colors.error, default.error);
    }

    #[test]
    fn empty_values_are_ignored() {
        let colors = ThemeColors::from_toml("accent = \"\"\ngreen = \"\"\n");
        assert_eq!(colors.accent, ThemeColors::default().accent);
        assert_eq!(colors.green, ThemeColors::default().green);
    }

    // ---- pywal tier ------------------------------------------------------

    const PYWAL_JSON: &str = r##"{
  "special": { "background": "#1a1b26", "foreground": "#a9b1d6", "cursor": "#ffffff" },
  "colors": {
    "color0": "#1a1b26",
    "color1": "#f7768e",
    "color2": "#9ece6a",
    "color3": "#e0af68",
    "color4": "#7aa2f7",
    "color5": "#ad8ee6"
  }
}"##;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "meteobar-theme-test-{}-{}",
            tag,
            std::process::id()
        ));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn pywal_is_used_when_no_omarchy_theme_exists() {
        let dir = scratch_dir("pywal-only");
        let pywal = write(&dir, "colors.json", PYWAL_JSON);

        let colors = ThemeColors::load_from(None, Some(pywal));
        assert_eq!(colors.text, "#a9b1d6"); // special.foreground
        assert_eq!(colors.accent, "#7aa2f7"); // colors.color4
        assert_eq!(colors.border, "#7aa2f7");
        assert_eq!(colors.green, "#9ece6a"); // colors.color2
        assert_eq!(colors.yellow, "#e0af68"); // colors.color3
        assert_eq!(colors.error, "#f7768e"); // colors.color1
        assert_eq!(colors.dim, "#62667e"); // fg (+) bg blend
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn omarchy_theme_wins_when_both_exist() {
        let dir = scratch_dir("both");
        let omarchy = write(&dir, "colors.toml", LEGACY_THEME);
        let pywal = write(&dir, "colors.json", PYWAL_JSON);

        let colors = ThemeColors::load_from(Some(omarchy), Some(pywal));
        // Omarchy values, not pywal's.
        assert_eq!(colors.accent, "#61afef");
        assert_eq!(colors.green, "#98c379");
        assert_eq!(colors.error, "#e06c75");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_files_fall_through_to_defaults() {
        let dir = scratch_dir("missing");
        let absent_toml = dir.join("nope.toml");
        let absent_json = dir.join("nope.json");

        assert_eq!(ThemeColors::load_from(None, None), ThemeColors::default());
        assert_eq!(
            ThemeColors::load_from(Some(absent_toml.clone()), Some(absent_json.clone())),
            ThemeColors::default()
        );
        // An unreadable Omarchy path still falls through to a usable pywal file.
        let pywal = write(&dir, "colors.json", PYWAL_JSON);
        assert_eq!(
            ThemeColors::load_from(Some(absent_toml), Some(pywal)).accent,
            "#7aa2f7"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn garbage_and_empty_pywal_json_falls_back_to_defaults() {
        for content in ["", "   ", "not json at all", "{", "[]", "null"] {
            assert_eq!(
                ThemeColors::from_pywal_json(content),
                ThemeColors::default(),
                "content: {content:?}"
            );
        }
        // Valid JSON with the sections empty or absent: still just defaults.
        assert_eq!(ThemeColors::from_pywal_json("{}"), ThemeColors::default());
        assert_eq!(
            ThemeColors::from_pywal_json(r##"{"special":{},"colors":{}}"##),
            ThemeColors::default()
        );
    }

    #[test]
    fn pywal_orange_is_synthesized_from_yellow_and_red() {
        let colors = ThemeColors::from_pywal_json(PYWAL_JSON);
        // Midpoint of yellow #e0af68 and red #f7768e — never aliased to red.
        assert_eq!(colors.orange, "#ec937b");
        assert_ne!(colors.orange, colors.error);
        assert_ne!(colors.orange, colors.yellow);

        // With either end missing, orange keeps its default rather than
        // collapsing onto red.
        let only_red = ThemeColors::from_pywal_json(r##"{"colors":{"color1":"#f7768e"}}"##);
        assert_eq!(only_red.error, "#f7768e");
        assert_eq!(only_red.orange, ThemeColors::default().orange);
    }

    #[test]
    fn pywal_accent_falls_back_to_cursor_and_ignores_non_hex() {
        // No color4 → cursor is the highlight.
        let colors =
            ThemeColors::from_pywal_json(r##"{"special":{"cursor":"#abcdef"},"colors":{}}"##);
        assert_eq!(colors.accent, "#abcdef");

        // Non-hex values are ignored per-field; a bad color4 defers to cursor.
        let colors = ThemeColors::from_pywal_json(
            r##"{"special":{"cursor":"#abcdef","foreground":"rgb(1,2,3)"},
                "colors":{"color4":"blue","color2":"not-a-color"}}"##,
        );
        assert_eq!(colors.accent, "#abcdef");
        assert_eq!(colors.text, ThemeColors::default().text);
        assert_eq!(colors.green, ThemeColors::default().green);
    }

    #[test]
    fn xdg_cache_home_is_honored_for_the_pywal_path() {
        // Explicit XDG_CACHE_HOME wins.
        assert_eq!(
            pywal_path(dir_from(
                Some(OsString::from("/xdg/cache")),
                Some(PathBuf::from("/home/u")),
                ".cache"
            )),
            Some(PathBuf::from("/xdg/cache/wal/colors.json"))
        );
        // Unset or empty falls back to ~/.cache.
        for empty in [None, Some(OsString::from(""))] {
            assert_eq!(
                pywal_path(dir_from(empty, Some(PathBuf::from("/home/u")), ".cache")),
                Some(PathBuf::from("/home/u/.cache/wal/colors.json"))
            );
        }
        // No env and no home: nothing to look at.
        assert_eq!(pywal_path(dir_from(None, None, ".cache")), None);
    }

    #[test]
    fn is_hex_color_accepts_only_hex_forms() {
        for good in ["#abc", "#abcd", "#aabbcc", "#aabbccdd", "#ABCDEF"] {
            assert!(is_hex_color(good), "should accept {good}");
        }
        for bad in [
            "",
            "#",
            "abcdef",
            "#ab",
            "#abcde",
            "#abcdefg",
            "#gggggg",
            "blue",
            "rgb(1,2,3)",
        ] {
            assert!(!is_hex_color(bad), "should reject {bad}");
        }
    }

    // ---- ramp + derived colors -------------------------------------------

    #[test]
    fn ramp_interpolates_between_published_stops() {
        let stops = vec![
            RampStop {
                pct: 0,
                color: "#000000".into(),
            },
            RampStop {
                pct: 50,
                color: "#808080".into(),
            },
            RampStop {
                pct: 100,
                color: "#ffffff".into(),
            },
        ];
        // Exact stops resolve to their own color.
        assert_eq!(ramp_color(&stops, 0), "#000000");
        assert_eq!(ramp_color(&stops, 50), "#808080");
        assert_eq!(ramp_color(&stops, 100), "#ffffff");
        // Midway between two stops is their midpoint, not a jump.
        assert_eq!(ramp_color(&stops, 25), "#404040");
        // Beyond the ends clamps rather than extrapolating.
        assert_eq!(ramp_color(&stops, 200), "#ffffff");
        assert_eq!(ramp_color(&[], 50), "");
    }

    #[test]
    fn precip_ramp_publishes_its_stop_positions() {
        let ramp = ThemeColors::default().precip_ramp();
        let positions: Vec<u8> = ramp.iter().map(|s| s.pct).collect();
        assert_eq!(positions, vec![0, 30, 60]);
        // Ascending, so interpolation is well defined.
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn thermal_anchors_follow_the_theme_lightness() {
        let dark = ThemeColors {
            text: "#e0e0e0".into(),
            ..ThemeColors::default()
        };
        let light = ThemeColors {
            text: "#202020".into(),
            ..ThemeColors::default()
        };
        // A light foreground yields light anchors, a dark one dark anchors.
        assert!(lightness(&dark.temp_cold()).unwrap() > lightness(&light.temp_cold()).unwrap());
        // Cold reads bluer than warm in both cases.
        for colors in [&dark, &light] {
            let (cold, warm) = (colors.temp_cold(), colors.temp_warm());
            assert_ne!(cold, warm);
            let (cr, _, cb) = parse_hex(&cold).unwrap();
            let (wr, _, wb) = parse_hex(&warm).unwrap();
            assert!(cb > cr, "cold {cold} should be blue-dominant");
            assert!(wr > wb, "warm {warm} should be red-dominant");
        }
    }

    #[test]
    fn short_and_alpha_hex_forms_are_blended_not_ignored() {
        // #rgb / #rgba / #rrggbbaa all parse; alpha is dropped.
        assert_eq!(parse_hex("#abc"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_hex("#abcd"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_hex("#aabbccdd"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_hex("#gg0000"), None);
        assert_eq!(parse_hex("#abcde"), None);

        // A theme written in shorthand derives a real dim instead of silently
        // falling back to the One Dark tone.
        let colors = ThemeColors::from_toml("foreground = \"#fff\"\nbackground = \"#000\"\n");
        assert_eq!(colors.dim, "#808080");
        assert_ne!(colors.dim, ThemeColors::default().dim);
    }

    #[test]
    fn dim_blends_foreground_and_background() {
        let colors = ThemeColors::from_toml(MODERN_THEME);
        // Midpoint of #a9b1d6 and #1a1b26.
        assert_eq!(colors.dim, "#62667e");
        // Without a background there is nothing to blend against.
        let no_bg = ThemeColors::from_toml("foreground = \"#a9b1d6\"\n");
        assert_eq!(no_bg.dim, ThemeColors::default().dim);
    }
}
