# CLAUDE.md

## Tooling

- Build: `cargo build --release`
- Install: `make install` (installs to `/usr/local/bin`, override with `PREFIX=`)
- Lint: `cargo clippy`
- Format: `cargo fmt`
- No test suite exists

## Non-Obvious Rules

- Output must be valid Waybar JSON (`{"text": ..., "tooltip": ..., "class": ..., "alt": ...}`)
- Tooltip uses Pango markup for colors and formatting — escape user-facing strings
- Tooltip always uses Nerd Font icons regardless of `--icons` setting (for monospace alignment)
- Response cache uses flock-based file locking (`cache.rs`) with 60s TTL
- Theme colors are read from Omarchy (`~/.config/omarchy/current/theme/colors.toml`), falling back to One Dark
- Font Awesome icons are wrapped in Pango markup (`<span>`) for correct rendering in Waybar
