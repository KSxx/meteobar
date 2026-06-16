# AGENTS.md — meteobar

Weather widget for Waybar (Rust). See the workspace `AGENTS.md` for the shared
widget contract (exit 0 with valid Waybar JSON on every path) and the AUR rules.

- Build: `make build`; install: `make install PREFIX=~/.local`. Lint: `cargo clippy`; format: `cargo fmt`.

## Release

A release is automated by pushing a tag — do NOT build or upload the binary by hand:

1. Bump `version` in `Cargo.toml` + `Cargo.lock`; commit `chore: release X.Y.Z`.
2. `git tag vX.Y.Z && git push origin master --tags`.
3. The tag push triggers `.github/workflows/release.yml`, which builds and publishes the GitHub release with the asset `meteobar-X.Y.Z-x86_64-linux` (consumed by the `meteobar-bin` AUR package).
4. Only after the release exists, bump both AUR repos (`aur/meteobar` source + `aur/meteobar-bin`) per the workspace `AGENTS.md`. Order matters: `updpkgsums` fetches the tag tarball AND the release asset, so both must already be live.
