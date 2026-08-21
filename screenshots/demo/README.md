# Demo fixture

`demo-data` gives meteobar a fixed, deliberately varied Berlin forecast so
README screenshots show a full widget instead of whatever the weather happens
to be: five condition glyphs across the week, an hourly series that crosses
sunset (day *and* night icons), rain chances spanning the whole ramp, and a
wide min/max spread so the panel's range bars sit at clearly different offsets.

It does **not** reimplement the CLI. It seeds a private cache (via
`XDG_CACHE_HOME`) with the fixture data and then execs the real binary, so both
output modes, the theme chain, `--no-color`, `--frame`, `--icons`, `--units`
and the argument-error contract are all production behavior. Only the weather
is fake. Dates and the "Updated" stamp are computed at run time.

Put it ahead of the real binary on `PATH` (the `meteobar` wrapper next to it
makes that work), then take the shot:

```bash
PATH="$PWD/screenshots/demo:$PATH" meteobar --output json   # panel / Omarchy plugin
PATH="$PWD/screenshots/demo:$PATH" meteobar --frame         # waybar tooltip
```

It finds the real binary at `target/release/meteobar`, or wherever
`METEOBAR_BIN` points. Not wired into the build, the install target, or the
test suite.
