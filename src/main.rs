mod api;
mod cache;
mod forecast;
mod format;
mod icons;
mod structured;
mod theme;
mod waybar;

use std::time::Duration;

use clap::Parser;

use format::FormatData;
use icons::IconSet;
use waybar::{NoColorScope, Paint, TooltipFormat, WaybarOutput};

#[derive(Parser)]
#[command(
    name = "meteobar",
    version,
    about = "Weather widget for Waybar using Open-Meteo"
)]
struct Cli {
    #[arg(
        long,
        help = "City name, 'City, Province', 'City, Country', or 'auto' for IP geolocation"
    )]
    location: Option<String>,

    #[arg(long, requires = "lon", allow_hyphen_values = true)]
    lat: Option<f64>,

    #[arg(long, requires = "lat", allow_hyphen_values = true)]
    lon: Option<f64>,

    #[arg(long, help = "Display name for the location (used with --lat/--lon)")]
    city_name: Option<String>,

    #[arg(long, default_value = "{icon} {temp}°")]
    format: String,

    #[arg(long, value_enum, default_value_t = TooltipFormat::Days)]
    tooltip_format: TooltipFormat,

    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=7))]
    days: u8,

    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=24))]
    hours: u8,

    #[arg(long, value_enum, default_value_t = CliUnits::Metric)]
    units: CliUnits,

    #[arg(long, value_enum, default_value_t = IconSet::Nerd)]
    icons: IconSet,

    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=60))]
    timeout: u64,

    #[arg(
        long,
        help = "Draw the framed tooltip box (pins a Mono Nerd Font for alignment); off = plain, uses your font"
    )]
    frame: bool,

    #[arg(
        long,
        default_value = "JetBrainsMono Nerd Font Mono",
        help = "Font family pinned in framed mode — must be a complete Mono Nerd Font"
    )]
    frame_font: String,

    #[arg(long, value_enum, default_value_t = OutputFormat::Waybar, help = "Output format: 'waybar' JSON with a pango tooltip, or raw structured 'json'")]
    output: OutputFormat,

    #[arg(long = "no-color", value_enum, num_args = 0..=1, default_missing_value = "all", value_name = "WHAT", help = "Drop color markup: all (default), bar, or tooltip. NO_COLOR is honored too; this flag wins")]
    no_color: Option<NoColorScope>,
}

#[derive(Clone, clap::ValueEnum)]
enum CliUnits {
    Metric,
    Imperial,
}

#[derive(Clone, clap::ValueEnum)]
enum OutputFormat {
    /// Waybar module JSON: text, pango tooltip, class, alt (default)
    Waybar,
    /// Raw structured JSON for other frontends (e.g. the Omarchy shell plugin)
    Json,
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return report_cli_error(err),
    };
    // Theme colors load before the fetch/cache pipeline so a theme change
    // while a fetch is in flight cannot alter which palette this run uses.
    let colors = theme::ThemeColors::load();
    let color_choice =
        waybar::resolve_color_choice(cli.no_color, std::env::var("NO_COLOR").ok().as_deref());

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(cli.timeout))
        .user_agent(format!("meteobar/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("failed to build HTTP client");

    let units = match cli.units {
        CliUnits::Metric => api::Units::Metric,
        CliUnits::Imperial => api::Units::Imperial,
    };
    let imperial = matches!(cli.units, CliUnits::Imperial);
    let unit_label = if imperial { "°F" } else { "°C" };

    let cache_key = cache::CacheKey {
        location: cache_location_descriptor(&cli),
        units: if imperial { "imperial" } else { "metric" },
        days: cli.days,
        hours: cli.hours,
    };
    let cache = cache::Cache::new(&cache_key);
    let result = fetch_weather_pipeline(&cli, &client, &units, &cache, &cache_key);
    let last_fetched = cache
        .last_fetched()
        .map(chrono::DateTime::<chrono::Local>::from);

    match cli.output {
        OutputFormat::Waybar => {
            let output = match result {
                Ok((weather, city, _short, freshness)) => build_output(
                    &weather,
                    &city,
                    &cli,
                    unit_label,
                    &colors,
                    last_fetched,
                    if freshness.stale {
                        Some(freshness.stale_reason.unwrap_or("unknown"))
                    } else {
                        None
                    },
                    color_choice,
                ),
                // The error tooltip follows the tooltip surface's setting.
                Err(msg) => waybar::error_output(&msg, &colors, Paint::new(color_choice.tooltip)),
            };
            print_and_exit(output);
        }
        OutputFormat::Json => {
            let output = match result {
                Ok((weather, city, short, freshness)) => structured::build(
                    &weather,
                    &city,
                    &short,
                    &structured::Request {
                        days: cli.days,
                        hours: cli.hours,
                        icon_set: &cli.icons,
                        imperial,
                    },
                    structured::CacheInfo::from_freshness(&freshness),
                    &colors,
                ),
                Err(msg) => structured::error_output(&msg, &cli.icons, imperial, &colors),
            };
            print_structured_and_exit(output);
        }
    }
}

/// clap's default on a bad argument is a usage dump on stderr and a non-zero
/// exit, which leaves the widget blank. The contract is exit 0 with a valid
/// payload on every path, so argument errors are reported through whichever
/// output mode the command line asked for.
fn report_cli_error(err: clap::Error) {
    use clap::error::ErrorKind;

    // --help / --version arrive here as "errors"; they are not failures.
    if matches!(
        err.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        let _ = err.print();
        return;
    }

    let message = clap_error_message(&err.render().to_string());
    let args: Vec<String> = std::env::args().collect();
    let colors = theme::ThemeColors::load();
    if args_request_json(&args) {
        print_structured_and_exit(structured::error_output(
            &message,
            &IconSet::Nerd,
            false,
            &colors,
        ));
    } else {
        // clap never produced a Cli, so recover the monochrome scope from the
        // raw argv: a scoped --no-color must keep its precedence over NO_COLOR
        // even when some *other* argument is what failed to parse.
        let choice = waybar::resolve_color_choice(
            no_color_scope_from_args(&args),
            std::env::var("NO_COLOR").ok().as_deref(),
        );
        print_and_exit(waybar::error_output(
            &message,
            &colors,
            Paint::new(choice.tooltip),
        ));
    }
}

/// Recover `--no-color[=WHAT]` from raw arguments. An unparseable value is
/// ignored rather than guessed at, leaving NO_COLOR to decide.
fn no_color_scope_from_args(args: &[String]) -> Option<NoColorScope> {
    let scope = |value: &str| match value {
        "all" => Some(NoColorScope::All),
        "bar" => Some(NoColorScope::Bar),
        "tooltip" => Some(NoColorScope::Tooltip),
        _ => None,
    };
    let mut found = None;
    for (i, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--no-color=") {
            found = scope(value);
        } else if arg == "--no-color" {
            // A bare flag defaults to "all"; a following bare word may be its
            // value (clap's num_args = 0..=1).
            found = args
                .get(i + 1)
                .and_then(|next| scope(next))
                .or(Some(NoColorScope::All));
        }
    }
    found
}

/// Condense clap's multi-line usage output to the one line that says what is
/// actually wrong.
fn clap_error_message(rendered: &str) -> String {
    let first = rendered.lines().next().unwrap_or("").trim();
    let message = first.strip_prefix("error: ").unwrap_or(first).trim();
    if message.is_empty() {
        "invalid arguments".to_string()
    } else {
        message.to_string()
    }
}

/// Whether the (unparseable) command line asked for structured JSON. Exact
/// adjacency only, so a stray "json" elsewhere cannot flip the output mode.
fn args_request_json(args: &[String]) -> bool {
    args.iter().enumerate().any(|(i, arg)| {
        arg == "--output=json"
            || (arg == "--output" && args.get(i + 1).is_some_and(|value| value == "json"))
    })
}

/// Location component of the cache key: the request input as given. Never
/// geocodes — resolving before the cache lookup would defeat the cache.
fn cache_location_descriptor(cli: &Cli) -> String {
    if let (Some(lat), Some(lon)) = (cli.lat, cli.lon) {
        let name = cli.city_name.as_deref().unwrap_or("");
        return format!("coords:{lat},{lon}|name:{name}");
    }
    if let Some(ref location) = cli.location {
        let trimmed = location.trim();
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("auto") {
            return format!("loc:{trimmed}");
        }
    }
    "auto".to_string()
}

fn fetch_weather_pipeline(
    cli: &Cli,
    client: &reqwest::blocking::Client,
    units: &api::Units,
    cache: &cache::Cache,
    cache_key: &cache::CacheKey,
) -> Result<(api::WeatherData, String, String, cache::Freshness), String> {
    let (json, freshness) = cache.fetch_or_cached(|| {
        let location = resolve_location(cli, client)?;
        let weather = api::fetch_weather(
            client,
            location.lat,
            location.lon,
            cli.days,
            cli.hours,
            units,
        )?;
        let entry = CacheEntry {
            weather,
            city: location.city,
            city_short: Some(location.short),
            params: Some(cache_key.canonical()),
            fetched_at: Some(
                chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            ),
        };
        serde_json::to_string(&entry).map_err(|e| format!("cache serialize failed: {e}"))
    })?;

    let entry: CacheEntry =
        serde_json::from_str(&json).map_err(|e| format!("cache parse failed: {e}"))?;
    let short = entry.city_short.unwrap_or_else(|| entry.city.clone());
    Ok((entry.weather, entry.city, short, freshness))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    weather: api::WeatherData,
    city: String,
    /// The bare place name, without province or country. Optional so a cache
    /// file written by an older build still parses; it falls back to `city`.
    #[serde(default)]
    city_short: Option<String>,
    /// Canonical request descriptor the entry was fetched for (diagnostic).
    #[serde(default)]
    params: Option<String>,
    /// Fetch time recorded in the entry itself (diagnostic; freshness for
    /// output purposes still derives from the cache file's mtime).
    #[serde(default)]
    fetched_at: Option<String>,
}

fn resolve_location(
    cli: &Cli,
    client: &reqwest::blocking::Client,
) -> Result<api::ResolvedLocation, String> {
    if let (Some(lat), Some(lon)) = (cli.lat, cli.lon) {
        let city = cli
            .city_name
            .clone()
            .unwrap_or_else(|| format!("{:.2},{:.2}", lat, lon));
        // An explicit --city-name is already the label the user chose; there is
        // nothing to shorten.
        let short = city.clone();
        return Ok(api::ResolvedLocation {
            lat,
            lon,
            city,
            short,
        });
    }

    if let Some(ref location) = cli.location {
        let trimmed = location.trim();
        if trimmed.is_empty() {
            return api::geolocate_ip(client);
        }
        if trimmed.eq_ignore_ascii_case("auto") {
            return api::geolocate_ip(client);
        }
        return api::geocode(client, trimmed);
    }

    api::geolocate_ip(client)
}

fn build_output(
    weather: &api::WeatherData,
    city: &str,
    cli: &Cli,
    unit_label: &str,
    colors: &theme::ThemeColors,
    last_fetched: Option<chrono::DateTime<chrono::Local>>,
    stale_reason: Option<&str>,
    color_choice: waybar::ColorChoice,
) -> WaybarOutput {
    let icon_info = icons::get_icon(
        weather.current.weather_code,
        weather.current.is_day == 1,
        &cli.icons,
    );

    let current = &weather.current;
    let today_rain = weather
        .daily
        .precipitation_probability_max
        .first()
        .copied()
        .unwrap_or(0);

    let data = FormatData {
        icon: icon_info.icon,
        temp: format!("{}", current.temperature_2m.round() as i32),
        feels_like: format!(
            "{}",
            current
                .apparent_temperature
                .unwrap_or(current.temperature_2m)
                .round() as i32
        ),
        humidity: format!(
            "{}",
            current.relative_humidity_2m.unwrap_or(0.0).round() as i32
        ),
        wind: format!("{}", current.wind_speed_10m.unwrap_or(0.0).round() as i32),
        wind_dir: format::degrees_to_cardinal(current.wind_direction_10m.unwrap_or(0.0))
            .to_string(),
        pressure: format!("{}", current.pressure_msl.unwrap_or(0.0).round() as i32),
        // Raw: format::render escapes every substituted value itself.
        city: city.to_string(),
        min: format!(
            "{}",
            weather
                .daily
                .temperature_2m_min
                .first()
                .unwrap_or(&0.0)
                .round() as i32
        ),
        max: format!(
            "{}",
            weather
                .daily
                .temperature_2m_max
                .first()
                .unwrap_or(&0.0)
                .round() as i32
        ),
        rain_chance: format!("{}", today_rain),
        description: icon_info.description.to_string(),
    };

    // The bar surface emits no color markup of its own — the template renders
    // plain text and the only span it can carry is the Font Awesome *font*
    // selector, which is structural and stays in monochrome. `color_choice.bar`
    // therefore has nothing to strip today; the state exists so the four modes
    // stay uniform across the widget family.
    let text = format::render(&cli.format, &data);
    let tooltip = waybar::build_tooltip(
        city,
        weather,
        &cli.tooltip_format,
        cli.days,
        cli.hours,
        unit_label,
        colors,
        last_fetched,
        stale_reason,
        cli.frame,
        &cli.frame_font,
        Paint::new(color_choice.tooltip),
    );

    WaybarOutput {
        text,
        tooltip,
        class: vec![icon_info.css_class.to_string()],
        alt: icon_info.css_class.to_string(),
    }
}

fn print_and_exit(output: WaybarOutput) {
    match serde_json::to_string(&output) {
        Ok(json) => println!("{json}"),
        Err(_) => println!(
            r#"{{"text":"?","tooltip":"serialization error","class":["error"],"alt":"error"}}"#
        ),
    }
}

fn print_structured_and_exit(output: structured::StructuredOutput) {
    match serde_json::to_string(&output) {
        Ok(json) => println!("{json}"),
        Err(_) => println!(r#"{{"schema_version":1,"error":{{"message":"serialization error"}}}}"#),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_color_flag_parses_all_four_states() {
        let parse = |extra: &[&str]| {
            let mut argv = vec!["meteobar"];
            argv.extend_from_slice(extra);
            Cli::try_parse_from(argv).map(|cli| cli.no_color)
        };
        assert_eq!(parse(&[]).unwrap(), None);
        assert_eq!(parse(&["--no-color"]).unwrap(), Some(NoColorScope::All));
        assert_eq!(parse(&["--no-color=all"]).unwrap(), Some(NoColorScope::All));
        assert_eq!(parse(&["--no-color=bar"]).unwrap(), Some(NoColorScope::Bar));
        assert_eq!(
            parse(&["--no-color=tooltip"]).unwrap(),
            Some(NoColorScope::Tooltip)
        );
    }

    #[test]
    fn unknown_no_color_value_is_an_argument_error() {
        // `Cli` has no Debug impl, so match rather than unwrap_err().
        let err = match Cli::try_parse_from(["meteobar", "--no-color=purple"]) {
            Ok(_) => panic!("an unknown --no-color value must not parse"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), ErrorKind::InvalidValue);
        // It reaches the exit-0 reporting path with a one-line message.
        let message = clap_error_message(&err.render().to_string());
        assert!(message.contains("purple"), "message was: {message}");
        assert!(!message.contains('\n'));
    }

    #[test]
    fn bare_no_color_does_not_swallow_the_next_argument() {
        // num_args(0..=1) must not eat a following flag or its value.
        let cli = Cli::try_parse_from(["meteobar", "--no-color", "--location", "Berlin"]).unwrap();
        assert_eq!(cli.no_color, Some(NoColorScope::All));
        assert_eq!(cli.location.as_deref(), Some("Berlin"));
    }

    #[test]
    fn clap_error_message_condenses_to_one_line() {
        assert_eq!(
            clap_error_message("error: invalid value 'x'\n\nUsage: meteobar\n"),
            "invalid value 'x'"
        );
        assert_eq!(clap_error_message(""), "invalid arguments");
        assert_eq!(clap_error_message("   \n"), "invalid arguments");
    }

    #[test]
    fn scoped_monochrome_survives_an_argument_error() {
        // NO_COLOR=1 --no-color=bar --bogus must still color the tooltip: the
        // scoped flag is the more specific instruction even when parsing failed.
        let argv = args(&["meteobar", "--no-color=bar", "--bogus"]);
        assert_eq!(no_color_scope_from_args(&argv), Some(NoColorScope::Bar));
        let choice = waybar::resolve_color_choice(no_color_scope_from_args(&argv), Some("1"));
        assert!(choice.tooltip);
        assert!(!choice.bar);

        // A bare flag still means "all", with or without a following value.
        assert_eq!(
            no_color_scope_from_args(&args(&["meteobar", "--no-color", "--bogus"])),
            Some(NoColorScope::All)
        );
        assert_eq!(
            no_color_scope_from_args(&args(&["meteobar", "--no-color", "tooltip", "--bogus"])),
            Some(NoColorScope::Tooltip)
        );
        // An unparseable value is ignored, leaving NO_COLOR to decide.
        assert_eq!(
            no_color_scope_from_args(&args(&["meteobar", "--no-color=purple"])),
            None
        );
        assert_eq!(no_color_scope_from_args(&args(&["meteobar"])), None);
    }

    #[test]
    fn args_request_json_matches_only_adjacent_output_json() {
        assert!(args_request_json(&args(&["meteobar", "--output", "json"])));
        assert!(args_request_json(&args(&["meteobar", "--output=json"])));
        assert!(!args_request_json(&args(&["meteobar"])));
        assert!(!args_request_json(&args(&[
            "meteobar", "--output", "waybar"
        ])));
        // "json" that is not the value of --output must not flip the mode.
        assert!(!args_request_json(&args(&[
            "meteobar",
            "--location",
            "json"
        ])));
        assert!(!args_request_json(&args(&["meteobar", "--output"])));
    }
}
