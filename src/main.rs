mod api;
mod cache;
mod format;
mod icons;
mod theme;
mod waybar;

use std::time::Duration;

use clap::Parser;

use format::FormatData;
use icons::IconSet;
use waybar::{TooltipFormat, WaybarOutput};

#[derive(Parser)]
#[command(name = "meteobar", version, about = "Weather widget for Waybar using Open-Meteo")]
struct Cli {
    #[arg(long, help = "City name, 'City, Province', 'City, Country', or 'auto' for IP geolocation")]
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

    #[arg(long, help = "Draw the framed tooltip box (pins JetBrainsMono Nerd Font Mono for alignment); off = plain, uses your font")]
    frame: bool,
}

#[derive(Clone, clap::ValueEnum)]
enum CliUnits {
    Metric,
    Imperial,
}

fn main() {
    let cli = Cli::parse();
    let colors = theme::ThemeColors::load();

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(cli.timeout))
        .user_agent(format!("meteobar/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("failed to build HTTP client");

    let units = match cli.units {
        CliUnits::Metric => api::Units::Metric,
        CliUnits::Imperial => api::Units::Imperial,
    };
    let unit_label = match cli.units {
        CliUnits::Metric => "°C",
        CliUnits::Imperial => "°F",
    };

    let cache = cache::Cache::new();
    let output = match fetch_weather_pipeline(&cli, &client, &units, &cache) {
        Ok((weather, city)) => {
            let last_fetched = cache.last_fetched().map(|st| {
                chrono::DateTime::<chrono::Local>::from(st)
            });
            build_output(&weather, &city, &cli, unit_label, &colors, last_fetched)
        }
        Err(msg) => waybar::error_output(&msg, &colors),
    };
    print_and_exit(output);
}

fn fetch_weather_pipeline(
    cli: &Cli,
    client: &reqwest::blocking::Client,
    units: &api::Units,
    cache: &cache::Cache,
) -> Result<(api::WeatherData, String), String> {
    let json = cache.fetch_or_cached(|| {
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
        };
        serde_json::to_string(&entry).map_err(|e| format!("cache serialize failed: {e}"))
    })?;

    let entry: CacheEntry =
        serde_json::from_str(&json).map_err(|e| format!("cache parse failed: {e}"))?;
    Ok((entry.weather, entry.city))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    weather: api::WeatherData,
    city: String,
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
        return Ok(api::ResolvedLocation { lat, lon, city });
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
        city: waybar::pango_escape(city),
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
        cli.frame,
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
        Err(_) => println!(r#"{{"text":"?","tooltip":"serialization error","class":["error"],"alt":"error"}}"#),
    }
}
