# Changelog

## 0.4.2+u13 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u12 with Storage and Hosts widgets.

### Added

- Storage widget: the system drive and USB drives, a gauge and the space
  left each. Amber past 75%, red with a FULL tag past 90%. A tap opens
  Files.
- Hosts widget: up to three hosts of `~/.ssh/config`, each UP, SLOW (past
  150 ms) or DOWN on a filled tag, with its address and latency. A tap
  on a row opens `foot ssh`. `pt35d` checks the SSH port every two
  minutes, not on each open; `pt35ctl hosts` prints the result.
- SSH screen (All apps, `pt35ctl menu open ssh`): every host of
  `~/.ssh/config` and its `Include`s. A connects, Y pins to the widget.

### Fixed

- The menu's widget reads (`traffic`, `hosts`) no longer make `pt35d`
  send the bar a status it did not need.

## 0.4.2+u12 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u11 with a pile of widgets on the
home and a throughput graph.

### Added

- Widget pile: the bottom of the home's widget column shows one pinned
  widget at a time, with `1/2` in its header. L and R flip it on the
  home, and so does a tap on the header. A player that starts brings
  Media to the front.
- All widgets (X on the home, `pt35ctl menu open widgets`): Y pins or
  unpins, A shows one. Pins in `~/.local/state/pt35/widgets`.
- Network graph: the last three minutes, down as a filled area and up as
  a dotted line, on a log scale from 1 kb/s to 1 Gb/s with lines at k, M
  and G. `pt35d` keeps the history; `pt35ctl traffic` prints it.
- Media: previous, play or pause, and next buttons, a music icon, and the
  state in words beside it.

### Changed

- The widget column is split half and half.
- Network names the route in use: LAN, WIFI and the SSID, or OFFLINE.
  Rates are in bits (`kb/s`, `Mb/s`). The Wi-Fi signal bar is left to
  the top bar.
- On the home, X opens All widgets instead of searching; typing still
  searches every app. L and R no longer switch mode there: the bar's
  mode icon does.

## 0.4.2+u11 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u10 without Find on the home's
legend.

### Changed

- The home's legend no longer shows X Find. Search belongs to All apps
  (Y), and X on the home still opens it searching.

## 0.4.2+u10 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u9 with a colour of its own for
the home's tiles.

### Added

- `[color] tile`: the face of the home's tiles and panels, the one
  `[menu] tile_opacity` lets the wallpaper through. Unset is
  `background_alt`, as before. With a face this close to a dark
  wallpaper, the Tiles slider changed nothing you could see; a lighter
  tile makes it visible without lightening every list.

## 0.4.2+u9 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u8 with framed lists and Quick
settings as a list.

### Added

- `[menu] row_frames`: list icons sit in a square frame. Quick settings
  is then drawn like any list: full rows that scroll, the list's
  selection with its ink, hairlines between rows, and switches as a
  knob with ON or OFF beside it, in the display face when there is one:
  green when on, red when off. Off by default.

### Fixed

- The selection pill stops short of the scrollbar instead of running
  under it.

## 0.4.2+u8 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u7 with a media widget on the home.

### Added

- The home's widget column shows a MEDIA panel in place of NETWORK while
  an MPRIS player plays or is paused: state as a shape and a word
  (PLAYING, PAUSED), title, artist, position in segments and as
  `1:23 / 7:29`. A tap on the panel plays or pauses. Read with
  `playerctl` off the Wayland thread; with no `playerctl` or no player,
  NETWORK stays. mpv needs `mpv-mpris`; VLC speaks MPRIS on its own.

## 0.4.2+u7 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u6 with a new menu mark.

### Changed

- The menu button, the launcher's tile and the fastfetch logo draw an
  eight-blade shutter instead of the skull, 15x15. Its gaps run along
  rows, columns and diagonals, so it stays sharp at scale 1 in the bar.
  `[bar] menu_icon = "skull"` keeps its name and now means the shutter.

## 0.4.2+u6 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u5 with a CRT sweep, a framed
legend and see-through tiles.

### Added

- `[effects] sweep`: a soft band of light rolls down the whole screen in
  1.6s, like a CRT's refresh, at least this many seconds apart.
  `[effects] sweep_jitter` adds up to that many seconds more to each gap,
  at random. Only while the menu is up, never over an app. The bar and
  the menu time it off the wall clock, random part included, so it is
  one band across both. `[effects] sweep_strength` is its brightness in
  percent (8). 0, the default, is off.
- `[menu] hint_frames`: the legend's buttons as square keys, each with
  its action in a frame. Off by default.
- `[menu] tile_opacity`: the home's tiles and panels at this opacity, so
  the wallpaper shows through. Below 100 their corners are square. 100 by
  default. With a home, Quick settings has a Tiles slider for it, from 20
  to 100%, saved in `appearance.toml`.

## 0.4.2+u5 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u4 with a display face and visible
scanlines.

### Added

- `[font] family_display`: a face for the big readouts, the home's clock
  and the bar's. Empty, the default, is the mono face. `[bar] clock_size`
  sizes the bar's clock in it.
- `[effects] glow`: a halo round the home's clock, in pixels.
- `[effects] scanline_period`: one dark line in this many. 2, the
  default, is every other line, which a 3.5" panel shows as a dimmer
  screen rather than lines. Keep the bar height a multiple of it, so the
  bar's lines meet the menu's.
- Fonts are looked for in `~/.local/share/fonts` first.

### Changed

- The home has no header: it is the desktop, not a screen with a name.
  The grid takes the room.
- A focused dock slot draws its icon in `accent_fg`, like a launcher tile.
- Quick settings gauges start a third of the way across, or past the
  label when it is longer.

## 0.4.2+u4 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u3 with a home of pins only.

### Changed

- The home shows the pinned entries and nothing else. Y opens All apps,
  where A opens, B goes back, X searches and Y pins. Settings and Power
  are at the end of All apps and pin like an app. A home with no pins
  says so. In Mouse mode, a right click on a tile offers All apps.
- The left column of the home is the widget column, in code too
  (`widgets.rs`).

## 0.4.2+u3 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u2 plus a home screen.

### Added

- `[menu] launcher_home`: with a grid, the launcher is a home. Pinned
  apps, then All apps, Settings and Power tiles; no side column, Windows
  stays on Select. A column on the left shows the clock and date, CPU,
  RAM and temperature gauges, and the network: name, address, Wi-Fi
  signal, rates. Gauges with nothing to read are left out. A search opens
  All apps, a new list screen (`apps`) of every app, where Y pins.

## 0.4.2+u2 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2+u1 plus a launcher grid and screen effects.

### Added

- `[menu] launcher_columns`: 2 or more lays the launcher out as a grid,
  icon above name, at `[icons] size_launcher` (64). The picked tile is
  filled in `accent` and its icon drawn in `accent_fg`. Right leaves the
  grid for the side column from the last tile of a line. 1, the default,
  keeps the list.
- `[icons] glow`: a soft halo under each grid icon, in pixels. 0, the
  default, draws none.
- `[effects] scanlines`: CRT lines on the bar and the menu, every other
  line darker by this many percent. `pt35-bar --scanlines` lays them over
  the apps as a click-through overlay below the bar; start it from your
  sway config. 0, the default, draws none.

### Fixed

- The scrollbar thumb of a grid is sized by tiles, not lines.

## 0.4.2+u1 (2026-11-01)

PocketTerm 3.5 Unleashed build: 0.4.2 plus one change.

### Added

- `~/.config/pt35/wallpaper.svg`, when there is one, is drawn under the
  launcher. resvg is built without text, so the SVG has to be shapes.

## 0.4.2 (2026-10-21)

After upgrading, reflash the keyboard with `sudo pt35-kbd flash`.

### Fixed

- The keyboard works after a boot again. The keyboard sends a terminal title
  each time its console opens. Since 0.4.0, pt35d opened the console and only
  then ran `stty` to turn the echo off. In that gap, Linux echoed the title
  back to the keyboard with its emoji mangled into bytes that are not UTF-8.
  The pt35 firmware failed to read them and stopped, and every key was dead
  until the next power cycle. pt35d now turns the echo off itself, right
  after it opens the port. The firmware reads its console as raw bytes, so a
  bad byte no longer stops it.
- The firmware keeps reading keys after an error, such as a busy USB link.
  Before, any error stopped it until the next power cycle.
- `pt35-kbd` now restarts the keyboard firmware when the keyboard does not
  answer it. Before, it left the keyboard stopped.

## 0.4.1 (2026-10-19)

### Fixed

- The installer no longer stops halfway when a config file the package ships
  is already on disk. That happens on a move from a source install to the
  package, and after editing `/etc/keyd/pocketterm35.conf`. dpkg asked which
  copy to keep, and with no terminal the install failed. The installer now
  keeps the file on disk and puts the package's copy beside it as
  `.dpkg-dist`.
- The installer no longer tells you to flash a keyboard that already runs the
  pt35 firmware. It asks the running pt35d.

## 0.4.0 (2026-10-19)

After upgrading, reflash the keyboard with `sudo pt35-kbd flash` and run
`pt35-ai agents --reset` if you use the AI agent. The firmware and the agent
rules below only take effect then.

### Security

- Joining a Wi-Fi network no longer runs its name as a shell command. Before,
  picking a nearby network named `$(...)` ran that command. The menu now passes
  the name to nmcli as an argument.
- The Default and Code agents ask before they run `git diff` or `git log`. The
  `--output` flag of either can overwrite any file.

### Fixed

- Pressing `2` on a confirmation such as Shut down or Close all no longer
  answers Yes.
- The face buttons keep working after a sway reload with `$mod+Shift+c`. pt35d
  sees the reload and binds them again.
- The patched firmware prints nothing but its brightness answer. Its debug
  output could fill the console buffer and stop the keyboard.
- `B50` now reads back 50, not 49. Fn `-` stops at 5% instead of turning the
  panel off. Letting go of Fn before Select takes the screenshot and no longer
  opens the switcher.
- A typo in one theme file now drops only that file, and the menu shows which
  file and line. The next Appearance pick no longer wipes a broken
  `appearance.toml`. The shell writes picks, pins and recents to a temporary
  file and renames it, so a power cut cannot leave half a file.
- When a window closes, the switcher's cursor stays on the window it was on.
  Closing a card behind you no longer moves you to another app.
- A search you are typing stays when a Bluetooth or Wi-Fi scan refreshes the
  list.
- A tap on a filtered Quick panel changes the row you tapped. The sliders move
  on each press instead of a press or two later.
- Each app with a profile shows once in the launcher and always starts with
  its profile. A `.desktop` file in `~/.local/share/applications` overrides the
  system copy, and one with `Hidden=true` hides the app. The launcher reads
  `XDG_DATA_DIRS` and checks `TryExec`, `OnlyShowIn` and `NotShowIn`.
- You can join a Wi-Fi network whose name has a `:` or `\` in it. The Quick
  panel shows the network's SSID and whether the radio is on.
- Apps started from the shell no longer stay behind as zombie processes.
- pt35d asks a stock keyboard for its firmware three times per plug-in instead
  of every 10 seconds, so `pt35-kbd flash` gets through. It notices when
  `pt35-kbd restore` takes the patched firmware away.
- With pt35d not running, the menu can still close. The sway config restarts
  pt35d and the bar when they die.
- A helper that hangs, such as wpctl, bluetoothctl, nmcli or grim, no longer
  hangs the daemon or a menu screen. Each one now has a time limit.
- The bar shows the wired icon only with a cable in, and never a `--` battery.
  With many windows open it shrinks the dock slots instead of drawing them
  under the status icons.
- The menu draws the last frame of each glide, so the highlight lands exactly
  on its row.
- Menu errors are visible. The hint bar used to cover the error line.
- The wheel stops at the end of a list instead of jumping to the other end.
  Releasing a scrollbar drag over a row no longer opens that row. In Mouse
  mode, tapping the B pill opens the row's options, as pressing B does.
- Uninstalling a source install through curl now also undoes the greetd setup.
- postinst no longer creates a missing `~/.config` owned by root.
- `pt35-ai` downloads a model again when its pinned checksum changes.
- `pt35-kbd write` checks the new file before it replaces the old one.

### Changed

- A, B and X no longer repeat in Buttons mode. Holding one while the menu
  opened could crash sway 1.10.
- Qt apps fall back to XWayland when the Qt Wayland plugin is missing.

## 0.3.2 (2026-09-24)

### Fixed

- An app's context menu no longer covers the launcher. Opening the menu over
  a right-click menu left the popup on top, holding the keyboard. The popup
  now closes when the menu opens.
- The DarkWire Default agent stops reaching for sudo. It tried
  `sudo journalctl`, the sudo rule refused it, and it gave up. The prompt now
  says logs need no sudo and shows the command for recent errors. Run
  `pt35-ai agents --reset` to pick up the new prompt.

## 0.3.1 (2026-09-24)

### Added

- A local AI agent: `pt35-ai` installs llama.cpp, llama-swap and pinned
  models, with darkwire as the chat client and five agents. The DarkWire
  tile opens it.

### Changed

- The menu is the desktop when nothing is open on the workspace in front,
  so an app that exits leaves you on the launcher.

### Fixed

- Lock was a white screen: swaylock with no config paints white. It now
  uses the theme's colours, unless you have a swaylock config of your own.
- Screen off had no way back but ssh. Any key or touch now turns the
  panel on again. This needs `swayidle`, now a dependency.
- The power key no longer opens the power menu under the lock, where it
  waited after the unlock with Screen off selected.

## 0.3.0 (2026-09-23)

First release.
