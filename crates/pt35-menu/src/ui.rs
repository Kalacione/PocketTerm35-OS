//! The fullscreen menu surface.
//!
//! Three bands: a header with the screen title, a list of thumb-sized rows, and
//! a hint bar that says what the twelve physical buttons do *here*. The hint bar
//! exists because six of those buttons are letters — without a legend on screen
//! there is nothing to tell you that X searches and Y goes home.

use anyhow::Result;
use pt35_common::menu::Layout;
use pt35_common::theme::{Rgb, Theme};
use pt35_ui::canvas::Canvas;
use pt35_ui::font::Font;
use pt35_ui::keys::{Key, Mode};
use pt35_ui::layer::{self, App, SurfaceSpec};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::model::{Model, Step};
use crate::widgets::Widget;
use crate::{exec, providers};

/// One entry in the bottom legend. The pill is coloured like the physical
/// button so the legend can be read at a glance instead of word by word.
/// The launcher's side buttons: not apps, so not in the app list.
#[derive(Clone, Copy)]
enum Side {
    Screen(pt35_common::menu::Builtin),
    Page(&'static str),
}

/// Label, what it opens, icon, and the colour that marks it. Three of them, so
/// the colour is what you actually navigate by.
struct SideButton {
    label: &'static str,
    target: Side,
    icon: &'static str,
    tint: fn(&pt35_common::theme::Colors) -> Rgb,
}

/// How often the menu wakes up.
const TICK: Duration = Duration::from_millis(250);
/// A right-click menu: its rows, as a label and the button each one stands
/// for, and where they were drawn.
struct Popup {
    x: i32,
    y: i32,
    items: Vec<(&'static str, &'static str)>,
    hot: usize,
    rects: Vec<(i32, i32, i32, i32)>,
}

/// Height of one row of the right-click menu.
const POPUP_ROW: i32 = 40;

/// How far left of the drawn scrollbar still counts as on it.
const SCROLLBAR_SLOP: i32 = 12;
/// Width of the widget column: the clock, the machine, the network.
const WIDGET_WIDTH: i32 = 216;
/// Height a launcher tile adds under its icon: the name and the margins.
const LAUNCHER_LABEL: u32 = 32;

/// What a tap on the widget pile does.
#[derive(Clone, Debug, PartialEq)]
enum WidgetHit {
    /// The header: the next widget.
    Next,
    /// A player command for `playerctl`.
    Player(&'static str),
    /// Open something and leave the menu: Files, an SSH session.
    Run(crate::model::Command),
}

/// A percent from the theme as an alpha.
fn opacity(percent: u8) -> u8 {
    (percent.min(100) as u32 * 255 / 100) as u8
}
/// How many of those ticks go by between reads of the daemon.
const POLL_TICKS: u8 = 4;

const SIDE: &[SideButton] = &[
    SideButton {
        label: "Windows",
        target: Side::Screen(pt35_common::menu::Builtin::Windows),
        icon: "multitasking-view",
        tint: |c| c.button_x,
    },
    SideButton {
        label: "Settings",
        target: Side::Screen(pt35_common::menu::Builtin::Quick),
        icon: "preferences-system",
        tint: |c| c.accent,
    },
    SideButton {
        label: "Power",
        target: Side::Page("power"),
        icon: "system-shutdown",
        tint: |c| c.critical,
    },
];

#[derive(Clone, Copy)]
struct Hint {
    button: &'static str,
    action: &'static str,
}

// No Start anywhere but the search legend. B goes back and, at the top screen,
// back means out: two keys for one job is one key wasted on a device with
// twelve of them.
const NAV_HINTS: &[Hint] = &[
    Hint {
        button: "A",
        action: "Open",
    },
    Hint {
        button: "B",
        action: "Back",
    },
    Hint {
        button: "X",
        action: "Find",
    },
    Hint {
        button: "Y",
        action: "Home",
    },
];

const WINDOW_HINTS: &[Hint] = &[
    Hint {
        button: "<>",
        action: "Move",
    },
    Hint {
        button: "A",
        action: "Focus",
    },
    Hint {
        button: "B",
        action: "Back",
    },
    // Searching three tiles you can see is not worth a key. Closing all of
    // them is, and it asks first.
    Hint {
        button: "X",
        action: "Close all",
    },
    Hint {
        button: "Y",
        action: "Close",
    },
];

const MOUSE_HINTS: &[Hint] = &[
    Hint {
        button: "A",
        action: "Click",
    },
    Hint {
        button: "B",
        action: "Options",
    },
    Hint {
        button: "X/Y",
        action: "Scroll",
    },
    Hint {
        button: "R",
        action: "Buttons mode",
    },
];

const QUICK_HINTS: &[Hint] = &[
    Hint {
        button: "<>",
        action: "Adjust",
    },
    Hint {
        button: "B",
        action: "Back",
    },
    Hint {
        button: "Y",
        action: "Home",
    },
];

// Start here clears the search rather than closing the menu: the list decides
// that before the model ever sees the key.
// Start earns its place here: it drops the whole filter at once, which
// Backspace only does one character at a time.
const FILTER_HINTS: &[Hint] = &[
    Hint {
        button: "Enter",
        action: "Open",
    },
    Hint {
        button: "^v",
        action: "Move",
    },
    Hint {
        button: "Start",
        action: "Clear",
    },
    Hint {
        button: "Bksp",
        action: "Exit",
    },
];

pub struct Menu {
    theme: Theme,
    font: Font,
    mono: Font,
    /// For the things you press. Regular text at 16px on this panel is thin.
    bold: Font,
    mono_bold: Font,
    model: Model,
    status: Option<pt35_common::ipc::Status>,
    icons: pt35_ui::icon::Icons,
    /// Which of the launcher's side buttons has the cursor, if any. The apps
    /// are a list and these three are not, so they are tracked apart.
    side: Option<usize>,
    windows: usize,
    /// Nothing is open on the workspace behind the menu.
    desktop: bool,
    /// Hit boxes recorded by the last draw, so touch never has to re-derive
    /// the layout and drift from it.
    /// `(left, top, right, bottom, row index)`. The index is carried rather
    /// than inferred from the hit box's position in this list.
    row_hits: Vec<(i32, i32, i32, i32, usize)>,
    /// Where the switcher's strip is held while the pointer drives it. `None`
    /// off the switcher.
    strip_scroll: Option<i32>,
    /// The last thing that moved the selection was the pointer, not a key.
    pointer_driven: bool,
    /// B picked up the home tile under the cursor: the D-pad moves it.
    moving: bool,
    /// The right-click menu, while it is up.
    popup: Option<Popup>,
    /// The header's breadcrumbs: `(left, right, depth)`.
    crumb_hits: Vec<(i32, i32, usize)>,
    /// The scrollbar as last drawn: `(left, right, top, bottom, thumb height)`,
    /// hit box included. `None` when everything fits.
    scrollbar: Option<(i32, i32, i32, i32, i32)>,
    /// The launcher's side column, same idea. Touch is the way back in when the
    /// RP2040 that owns the keyboard drops off the USB bus, and Windows,
    /// Settings and Power were unreachable that way.
    side_hits: Vec<(i32, i32, i32, i32, usize)>,
    /// The widget pile's touch targets: its header, the player's buttons.
    widget_hits: Vec<(i32, i32, i32, i32, WidgetHit)>,
    /// The pinned widgets and the one the pile shows.
    widgets: Vec<crate::widgets::Widget>,
    widget: usize,
    /// Whether the player was playing at the last read: one that starts
    /// brings Media to the front.
    was_playing: bool,
    hint_hits: Vec<(i32, i32, i32, &'static str)>,
    error: Option<String>,
    /// A slow provider still running on a worker thread, and the screen it
    /// belongs to. The menu draws a placeholder until it lands.
    loading: Option<(pt35_common::menu::Builtin, Receiver<providers::Items>)>,
    /// Ticks since the daemon was last asked. See [`Menu::tick`].
    since_poll: u8,
    /// App profiles, for matching a launcher row to an open window.
    apps: pt35_common::apps::AppTable,
    motion: Motion,
    /// The window last reported to pt35d as under the switcher's cursor.
    hover_sent: Option<i64>,
    /// Whether pt35d was last told the launcher is in front.
    launcher_sent: Option<(bool, Option<String>)>,
    /// The machine and the network, for the widget column.
    readings: crate::widgets::Readings,
    /// A Bluetooth scan is running until then; the list is re-read meanwhile.
    scan_until: Option<std::time::Instant>,
    /// Switcher previews read so far, by window id, with the file's mtime so
    /// a fresher capture replaces them.
    previews: std::collections::HashMap<i64, Preview>,
    /// Captures that would not decode, by the mtime they had, so a bad file is
    /// read once and not on every frame.
    bad_previews: std::collections::HashMap<i64, std::time::SystemTime>,
    /// The cards the last frame drew. Only those are waiting for a picture.
    cards_drawn: Vec<i64>,
    /// A drag that began on the scrollbar, until the button or finger lifts.
    scrolling: bool,
    /// A finger's pan not yet worth a whole line, in pixels.
    pan: f64,
    /// Drawn under the launcher. `None` keeps the theme's background.
    wallpaper: Option<pt35_ui::wallpaper::Wallpaper>,
    /// `[font] family_display`, for the clock. None draws it in mono.
    display: Option<Font>,
}

/// One window's picture, as pt35d captured it.
struct Preview {
    modified: std::time::SystemTime,
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

fn preview_path(id: i64) -> std::path::PathBuf {
    pt35_common::paths::previews_dir().join(format!("{id}.ppm"))
}

fn preview_modified(id: i64) -> Option<std::time::SystemTime> {
    std::fs::metadata(preview_path(id)).ok()?.modified().ok()
}

fn read_preview(id: i64) -> Option<Preview> {
    let path = preview_path(id);
    let modified = preview_modified(id)?;
    let (width, height, rgb) = pt35_ui::pixel::parse_ppm(&std::fs::read(&path).ok()?)?;
    Some(Preview {
        modified,
        width,
        height,
        rgb,
    })
}

/// The two movements the menu has: the selection gliding to its new row, and
/// a new screen settling in from a few pixels down. Both are over in about a
/// tenth of a second, and nothing is drawn for them once they are.
struct Motion {
    enabled: bool,
    screen: (usize, String),
    entered: std::time::Instant,
    from: Option<i32>,
    to: Option<i32>,
    moved: std::time::Instant,
}

const GLIDE: Duration = Duration::from_millis(90);
const ENTER: Duration = Duration::from_millis(120);
/// How far a new screen rises as it settles.
const ENTER_RISE: f32 = 12.0;

/// Ease-out: quick to start, soft to land.
fn ease(elapsed: Duration, span: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / span.as_secs_f32()).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

impl Motion {
    fn new(enabled: bool) -> Self {
        let long_ago = std::time::Instant::now() - Duration::from_secs(1);
        Self {
            enabled,
            screen: (0, String::new()),
            entered: long_ago,
            from: None,
            to: None,
            moved: long_ago,
        }
    }

    /// Note which screen is up. A new one starts its rise and forgets the old
    /// selection, so nothing glides across from a different list.
    fn screen(&mut self, id: (usize, String)) {
        if id != self.screen {
            self.screen = id;
            self.entered = std::time::Instant::now();
            self.from = None;
            self.to = None;
        }
    }

    /// Pixels to push the body down while the screen settles.
    fn rise(&self) -> i32 {
        if !self.enabled {
            return 0;
        }
        ((1.0 - ease(self.entered.elapsed(), ENTER)) * ENTER_RISE) as i32
    }

    /// Where to draw the selection on its way to `target`.
    fn glide(&mut self, target: i32) -> i32 {
        if !self.enabled {
            return target;
        }
        if self.to != Some(target) {
            self.from = Some(self.current().unwrap_or(target));
            self.to = Some(target);
            self.moved = std::time::Instant::now();
        }
        self.current().unwrap_or(target)
    }

    fn current(&self) -> Option<i32> {
        let (from, to) = (self.from?, self.to?);
        let k = ease(self.moved.elapsed(), GLIDE);
        Some(from + ((to - from) as f32 * k) as i32)
    }

    fn busy(&self) -> bool {
        self.enabled && (self.moved.elapsed() < GLIDE || self.entered.elapsed() < ENTER)
    }
}

impl Menu {
    pub fn new(
        theme: Theme,
        font: Font,
        mono: Font,
        bold: Font,
        mono_bold: Font,
        model: Model,
    ) -> Self {
        let status = crate::live::status();
        // With the pt35 firmware the face buttons are F-keys, so a letter is
        // a letter: typing never presses a button.
        let pt35_firmware = status.as_ref().is_some_and(|s| s.pt35_firmware);
        pt35_ui::keys::set_letters_are_buttons(!pt35_firmware);
        let icons = pt35_ui::icon::Icons::new(&theme.icons.theme);
        let theme_animations = theme.menu.animations;
        let (widgets, widget) = crate::widgets::pile();
        Self {
            icons,
            side: None,
            theme,
            font,
            mono,
            bold,
            mono_bold,
            model,
            windows: status.as_ref().map(|s| s.windows.len()).unwrap_or(0),
            // No daemon is no desktop: the menu has to be closable then, or it
            // covers everything with nothing able to take it down.
            desktop: status.as_ref().is_some_and(|s| s.workspace_empty()),
            status,
            row_hits: Vec::new(),
            strip_scroll: None,
            pointer_driven: false,
            moving: false,
            popup: None,
            crumb_hits: Vec::new(),
            scrollbar: None,
            side_hits: Vec::new(),
            widget_hits: Vec::new(),
            widgets,
            widget,
            was_playing: false,
            hint_hits: Vec::new(),
            error: None,
            loading: None,
            since_poll: 0,
            apps: pt35_common::load_config("pt35/apps.toml"),
            motion: Motion::new(theme_animations),
            hover_sent: None,
            launcher_sent: None,
            readings: crate::widgets::Readings::default(),
            scan_until: None,
            previews: std::collections::HashMap::new(),
            bad_previews: std::collections::HashMap::new(),
            cards_drawn: Vec::new(),
            scrolling: false,
            pan: 0.0,
            wallpaper: None,
            display: None,
        }
    }

    /// A card with no picture yet whose capture is there to be read.
    fn preview_wanted(&self, id: i64) -> bool {
        !self.previews.contains_key(&id)
            && preview_modified(id).is_some_and(|m| self.bad_previews.get(&id) != Some(&m))
    }

    /// Whether a launcher row already has a window open.
    ///
    /// The daemon focuses rather than duplicating when you pick one of these,
    /// so the row says so before you press it.
    fn is_running(&self, payload: &str) -> bool {
        let Some(status) = self.status.as_ref() else {
            return false;
        };
        let Some(binary) = payload
            .strip_prefix("exec:")
            .and_then(pt35_common::apps::command_binary)
            .map(|b| b.rsplit('/').next().unwrap_or(&b).to_lowercase())
        else {
            return match payload.strip_prefix("app:") {
                Some(id) => self
                    .apps
                    .get(id)
                    .is_some_and(|app| status.windows.iter().any(|w| app.matches_app(&w.app))),
                None => false,
            };
        };
        status
            .windows
            .iter()
            .any(|w| w.app.to_lowercase() == binary)
    }

    /// True when this menu is standing in for a desktop, because nothing is
    /// open behind it. It cannot be closed then: there would be nothing there.
    fn is_desktop(&self) -> bool {
        self.desktop && self.model.depth() == 1
    }

    /// A screen you read and do not press: System, About.
    fn readonly(&self) -> bool {
        self.model
            .dynamic_builtin()
            .is_some_and(|builtin| providers::screen(builtin).readonly)
    }

    /// The launcher row Y would pin: an app, on the top screen, with no search
    /// open.
    fn pin_target(&self) -> Option<String> {
        let filtering = self.model.screen().list.mode() == Mode::Filter;
        let all_apps = matches!(
            self.model.dynamic_builtin(),
            Some(
                pt35_common::menu::Builtin::Apps
                    | pt35_common::menu::Builtin::Widgets
                    | pt35_common::menu::Builtin::Ssh
            )
        );
        // On the home, Y opens All apps: pins are made and undone there.
        let top = self.on_launcher() && self.model.depth() == 1 && !self.home();
        if !(top || all_apps) || filtering || self.side.is_some() {
            return None;
        }
        self.model
            .selected_payload()
            .filter(|p| {
                all_apps && providers::pinnable(p)
                    || p.starts_with("app:")
                    || p.starts_with("exec:")
            })
            .map(str::to_string)
    }

    /// What a right click on the selected row offers, as a label and the
    /// button that does it. Empty when a plain click is all the row has.
    fn row_options(&self) -> Vec<(&'static str, &'static str)> {
        // In Mouse mode Y is the wheel: All apps is reached from here.
        if self.home() {
            return vec![("Open", "A"), ("Apps", "Y")];
        }
        if let Some(payload) = self.pin_target() {
            let pinned = crate::providers::pins().contains(&payload);
            return vec![("Open", "A"), (if pinned { "Unpin" } else { "Pin" }, "Y")];
        }
        if self.model.dynamic_builtin() == Some(pt35_common::menu::Builtin::Windows) {
            return match self.model.selected_payload() {
                Some(p) if p != providers::HOME => {
                    vec![("Switch to", "A"), ("Close", "Y"), ("Close all", "X")]
                }
                _ => Vec::new(),
            };
        }
        Vec::new()
    }

    /// The right-click menu, drawn last so it sits over everything. It opens
    /// at the pointer and moves in to stay on screen.
    fn draw_popup(&mut self, canvas: &mut Canvas) {
        let Some(mut popup) = self.popup.take() else {
            return;
        };
        let theme = self.theme.clone();
        let size = theme.font.size_hint;
        let pad = 14;
        let mut widest = 0;
        for (label, _) in &popup.items {
            widest = widest.max(self.font.measure(label, size) as i32);
        }
        let width = widest + pad * 2;
        let width = width.max(140);
        let height = POPUP_ROW * popup.items.len() as i32;
        let x = popup.x.min(canvas.width as i32 - width - 2).max(2);
        let y = popup.y.min(canvas.height as i32 - height - 2).max(2);
        let radius = theme.menu.radius;
        canvas.rounded_rect(
            x - 1,
            y - 1,
            (width + 2) as u32,
            (height + 2) as u32,
            radius,
            theme.color.accent,
        );
        canvas.rounded_rect(
            x,
            y,
            width as u32,
            height as u32,
            radius,
            theme.color.background_alt,
        );
        popup.rects.clear();
        for (i, (label, _)) in popup.items.iter().enumerate() {
            let top = y + i as i32 * POPUP_ROW;
            let ink = if i == popup.hot {
                canvas.rect(x, top, width as u32, POPUP_ROW as u32, theme.color.accent);
                theme.color.accent_fg
            } else {
                theme.color.foreground
            };
            let baseline = top + (POPUP_ROW as f32 * 0.64) as i32;
            self.font.draw(canvas, label, x + pad, baseline, size, ink);
            popup.rects.push((x, top, x + width, top + POPUP_ROW));
        }
        self.popup = Some(popup);
    }

    /// The next or the previous widget in the pile, round the ends.
    fn flip_widget(&mut self, forward: bool) -> bool {
        let count = self.widgets.len();
        if count < 2 {
            return true;
        }
        self.widget = if forward {
            (self.widget + 1) % count
        } else {
            (self.widget + count - 1) % count
        };
        crate::widgets::set_shown(self.widgets[self.widget]);
        true
    }

    /// Bring a widget to the front of the pile, pinning it if it was not.
    fn show_widget(&mut self, widget: Widget) {
        if !self.widgets.contains(&widget) {
            if let Err(e) = crate::widgets::toggle_pin(widget) {
                self.error = Some(format!("pinning: {e}"));
            }
            self.widgets = crate::widgets::pinned();
        }
        if let Some(at) = self.widgets.iter().position(|w| *w == widget) {
            self.widget = at;
            crate::widgets::set_shown(widget);
        }
    }

    /// The launcher is a home of pinned apps with the widget column.
    fn raining(&self) -> bool {
        self.theme.effects.rain > 0 && self.home()
    }

    fn home(&self) -> bool {
        self.on_launcher()
            && self.theme.menu.launcher_home
            && self.model.screen().list.columns() > 1
    }

    /// A key while a home tile is picked up. The D-pad swaps it with the tile
    /// it points at, and writes the pins at once: nothing is lost if the menu
    /// closes mid-move. A or B puts it down. Anything else puts it down and
    /// then does what it does (`None`).
    fn move_tile(&mut self, key: &Key) -> Option<bool> {
        use pt35_ui::keys::{button, navigate, Button, Navigation};
        if !self.home() {
            self.moving = false;
            return None;
        }
        if matches!(button(key, Mode::Nav), Some(Button::A | Button::B)) {
            self.moving = false;
            return Some(true);
        }
        let nav = navigate(key, Mode::Nav);
        if !matches!(
            nav,
            Navigation::Up | Navigation::Down | Navigation::Left | Navigation::Right
        ) {
            self.moving = false;
            return None;
        }
        let list = &self.model.screen().list;
        let at = list.selected()?;
        let Some(to) = providers::grid_step(at, list.len(), list.columns(), nav) else {
            return Some(true);
        };
        let mut items = self.model.dynamic_items()?.to_vec();
        if let Err(e) = providers::swap_pins(&items[at].payload, &items[to].payload) {
            self.error = Some(format!("pinning: {e}"));
            return Some(true);
        }
        let payload = items[at].payload.clone();
        items.swap(at, to);
        self.model.replace_dynamic(items);
        self.model.focus_payload(&payload);
        Some(true)
    }

    /// True when Right moves along the launcher grid rather than out of it.
    fn launcher_moves_right(&self) -> bool {
        let list = &self.model.screen().list;
        let columns = list.columns();
        let at = list.selected_position();
        columns > 1 && at % columns + 1 < columns && at + 1 < list.len()
    }

    fn on_launcher(&self) -> bool {
        matches!(
            self.model.screen().source,
            crate::model::Source::Dynamic {
                builtin: pt35_common::menu::Builtin::Launcher,
                ..
            }
        )
    }

    /// Push a builtin screen, reading its rows off the Wayland thread when they
    /// are slow to come by.
    fn open(&mut self, builtin: pt35_common::menu::Builtin) {
        let screen = providers::screen(builtin);
        let Some(scanning) = screen.scanning else {
            self.model
                .push_dynamic(builtin, screen.title, (screen.rows)());
            return;
        };
        self.model
            .push_dynamic(builtin, screen.title, providers::placeholder(scanning));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send((screen.rows)());
        });
        self.loading = Some((builtin, rx));
    }

    /// Open a screen from inside the menu. The switcher then starts on the
    /// Launcher card: the launcher is the "window" you were in.
    fn open_here(&mut self, builtin: pt35_common::menu::Builtin) {
        self.open(builtin);
        if builtin == pt35_common::menu::Builtin::Windows {
            self.model.screen_mut().list.select_item(0);
        }
    }

    fn open_side(&mut self, index: usize) -> bool {
        self.side = None;
        match SIDE.get(index).map(|button| button.target) {
            Some(Side::Screen(builtin)) => {
                self.open_here(builtin);
                true
            }
            Some(Side::Page(page)) => {
                self.model.open_page(page);
                true
            }
            None => true,
        }
    }

    /// Feed a tap to the model as if the matching button had been pressed.
    ///
    /// Every pill that is drawn is hit-tested and every one that is
    /// hit-tested does something.
    fn press(&mut self, button: &str) -> bool {
        let key = match button {
            // In Mouse mode B is the right button, and its pill says Options.
            "B" if self.mouse_mode() => return self.options_here(),
            "<>" => Key::new(pt35_ui::keys::sym::RIGHT),
            "Bksp" => Key::new(pt35_ui::keys::sym::BACKSPACE),
            "^v" => Key::new(pt35_ui::keys::sym::DOWN),
            // The button's own keysym, not its letter: with the pt35
            // firmware a letter is only a letter.
            "A" | "A B" => Key::new(pt35_ui::keys::sym::BUTTON_A),
            "B" => Key::new(pt35_ui::keys::sym::BUTTON_B),
            "X" => Key::new(pt35_ui::keys::sym::BUTTON_X),
            "Y" => Key::new(pt35_ui::keys::sym::BUTTON_Y),
            "R" | "L R" => Key::new(pt35_ui::keys::sym::BUTTON_R),
            "X/Y" => Key::new(pt35_ui::keys::sym::DOWN),
            "Start" => Key::new(pt35_ui::keys::sym::PAUSE),
            "Enter" => Key::new(pt35_ui::keys::sym::RETURN),
            _ => return true,
        };
        // Through `key`, so a pill does what its button does: R, L and Select
        // are handled there, before the list sees them.
        App::key(self, key)
    }

    fn mouse_mode(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|s| s.input_mode == pt35_common::ipc::InputMode::Mouse)
    }

    /// The right-click menu of the row under the cursor, as if clicked there.
    fn options_here(&mut self) -> bool {
        let cursor = self.model.screen().list.cursor_index();
        match self.row_hits.iter().find(|hit| hit.4 == cursor).copied() {
            Some((left, top, right, bottom, _)) => {
                self.back_click(f64::from((left + right) / 2), f64::from((top + bottom) / 2))
            }
            None => true,
        }
    }

    /// Ask the daemon to do something and wait for the reply. Waiting is the
    /// point: the daemon has applied the change by the time it answers, so the
    /// value on screen can be re-read with no sleep.
    fn ctl(&mut self, args: &[&str]) -> Result<(), String> {
        let request = pt35_common::ipc::Request::from_ctl(args)?;
        match crate::live::request(&request) {
            Ok(pt35_common::ipc::Response::Error { message }) => Err(message),
            Ok(_) => Ok(()),
            Err(message) => Err(message),
        }
    }

    /// Re-read the daemon and rebuild the current screen from it.
    ///
    /// Both halves matter. The rows of a page screen read `self.status` as they
    /// draw, but a builtin screen's rows were built by its provider and are
    /// frozen: without the rebuild, toggling Wi-Fi or Touch leaves the pill on
    /// its old value, which is the opposite of watching it flip.
    fn resync(&mut self) {
        self.status = crate::live::status();
        self.windows = self.status.as_ref().map_or(0, |s| s.windows.len());
        self.desktop = self.status.as_ref().is_some_and(|s| s.workspace_empty());
        let Some(builtin) = self.model.dynamic_builtin() else {
            return;
        };
        let screen = providers::screen(builtin);
        if screen.scanning.is_none() {
            self.model.replace_dynamic((screen.rows)());
            return;
        }
        // A slow screen reads off the Wayland thread, as when it opened. The
        // old rows stay up meanwhile: a radio that has just been switched on
        // has news worth the wait.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if matches!(builtin, pt35_common::menu::Builtin::Bluetooth) {
                // `power on` returns before the adapter says so.
                std::thread::sleep(Duration::from_millis(800));
            }
            let _ = tx.send((screen.rows)());
        });
        self.loading = Some((builtin, rx));
    }

    fn apply(&mut self, step: Step) -> bool {
        self.error = None;
        match step {
            Step::Close(payload) => {
                match payload.strip_prefix("con:").and_then(|id| id.parse().ok()) {
                    Some(id) => {
                        let request = pt35_common::ipc::Request::Window {
                            action: pt35_common::ipc::WindowAction::CloseId(id),
                        };
                        // Counted now, not at the next poll: closing the last
                        // window here makes this menu the desktop, and B must
                        // not leave it.
                        if let Some(status) = self.status.as_mut() {
                            status.windows.retain(|w| w.id != id);
                            self.desktop = status.workspace_empty();
                        }
                        match crate::live::request(&request) {
                            Ok(pt35_common::ipc::Response::Error { message }) => {
                                self.error = Some(message)
                            }
                            Err(message) => self.error = Some(message),
                            Ok(_) => {}
                        }
                    }
                    None => self.error = Some("that row is not a window".into()),
                }
                // Take the row out now. sway answers `kill` before the client
                // has actually gone, so re-reading would show it still open. The
                // The tick puts it back if the app refuses to close.
                self.model.drop_dynamic(&payload);
                // Counted now, not at the next poll: closing the last window
                // makes this menu the desktop, and B must not leave it.
                self.windows = self.windows.saturating_sub(1);
                true
            }
            Step::Adjust(adjust, up) => {
                if adjust == pt35_common::menu::Adjust::TileOpacity {
                    // A theme pick, so Appearance's file, not the daemon.
                    let now = self.theme.menu.tile_opacity.min(100) as i64;
                    let next = if up { now + 5 } else { now - 5 }
                        .clamp(providers::TILE_OPACITY_MIN as i64, 100);
                    let entry = [("tile_opacity", toml::Value::Integer(next))];
                    match pt35_common::set_appearance("menu", &entry, false) {
                        Ok(()) => self.theme = pt35_common::load_theme(),
                        Err(e) => self.error = Some(e.to_string()),
                    }
                    if let Some(items) = self.model.dynamic_items() {
                        let mut items = items.to_vec();
                        for item in items.iter_mut() {
                            if item.payload == "adjust:tile_opacity" {
                                *item = providers::opacity_row(self.theme.menu.tile_opacity);
                            }
                        }
                        self.model.replace_dynamic(items);
                    }
                    return true;
                }
                let args = adjust.step(up);
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                if let Err(message) = self.ctl(&args) {
                    self.error = Some(message);
                }
                let quick = self.model.dynamic_builtin() == Some(pt35_common::menu::Builtin::Quick);
                match self.model.dynamic_items().filter(|_| quick) {
                    Some(items) => {
                        let mut items = items.to_vec();
                        self.status = crate::live::status();
                        if let Some(status) = &self.status {
                            providers::patch_levels(&mut items, status);
                        }
                        self.model.replace_dynamic(items);
                    }
                    None => self.resync(),
                }
                true
            }
            Step::RunStay(command) => {
                if let Err(message) = exec::perform(&command) {
                    self.error = Some(message);
                }
                if matches!(&command, crate::model::Command::Helper(c) if c.contains("bluetooth scan"))
                {
                    self.scan_until = Some(std::time::Instant::now() + Duration::from_secs(13));
                }
                if matches!(command, crate::model::Command::Theme(_)) {
                    self.theme = pt35_common::load_theme();
                }
                // A shell payload is a detached script, so its effect lands after
                // it has been spawned. Everything else went through the daemon,
                // which had already applied it when it replied.
                if matches!(
                    exec::plan(&command),
                    exec::Plan::Shell(_) | exec::Plan::Spawn(_)
                ) {
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
                self.resync();
                true
            }
            // The menu is the desktop, so closing it means going back to the
            // top screen. There is nothing behind it to close onto.
            Step::Quit if self.desktop => {
                let step = self.model.go_home();
                self.apply(step)
            }
            Step::Quit => {
                // Tell the daemon on the way out rather than leaving it to
                // notice on its next poll, which left every button dead for up
                // to two seconds after the menu had gone.
                let _ = crate::live::request(&pt35_common::ipc::Request::Menu {
                    action: pt35_common::ipc::Toggle::Off,
                    page: None,
                });
                false
            }
            Step::Run(command) => {
                // A launch that fails must not close the menu: the screen would
                // go back to an empty workspace with no word of what happened.
                if let Err(message) = exec::perform(&command) {
                    self.error = Some(message);
                    return true;
                }
                // Tell pt35d now, as Quit does: otherwise it holds the menu's
                // bindings and the bar's state until its next poll.
                let _ = crate::live::request(&pt35_common::ipc::Request::Menu {
                    action: pt35_common::ipc::Toggle::Off,
                    page: None,
                });
                false
            }
            Step::Open(builtin) => {
                self.open_here(builtin);
                true
            }
            Step::TogglePin(payload) => {
                match providers::toggle_pin(&payload) {
                    Ok(pinned) => {
                        let here = self
                            .model
                            .dynamic_builtin()
                            .unwrap_or(pt35_common::menu::Builtin::Launcher);
                        self.model.replace_dynamic(providers::items(here));
                        if here != pt35_common::menu::Builtin::Launcher {
                            self.model.refresh_launcher(providers::items(
                                pt35_common::menu::Builtin::Launcher,
                            ));
                        }
                        // It just moved: keep the cursor on it.
                        self.model.focus_payload(&payload);
                        let _ = pinned;
                        if payload.starts_with("widget:") {
                            self.widgets = crate::widgets::pinned();
                            self.widget = self.widget.min(self.widgets.len().saturating_sub(1));
                        }
                    }
                    Err(e) => self.error = Some(format!("pinning: {e}")),
                }
                true
            }
            Step::ShowWidget(id) => {
                if let Some(widget) = Widget::from_id(&id) {
                    self.show_widget(widget);
                }
                let step = self.model.go_home();
                self.apply(step)
            }
            Step::Redraw | Step::Nothing => true,
        }
    }

    fn draw_header(&mut self, canvas: &mut Canvas) -> i32 {
        let theme = self.theme.clone();
        let height = theme.menu.header_height;
        let pad = theme.menu.padding_x as i32;
        let track = theme.font.tracking;
        canvas.rect(0, 0, canvas.width, height, theme.color.background_alt);
        if theme.menu.title_header {
            canvas.rect(
                0,
                height as i32 - 1,
                canvas.width,
                1,
                theme.color.foreground,
            );
            let baseline = self.draw_title_trail(canvas);
            self.draw_header_readout(canvas, baseline);
            return height as i32;
        }
        canvas.rect(0, height as i32 - 2, canvas.width, 2, theme.color.accent);

        let baseline = (height as f32 * 0.64) as i32;
        let size = theme.font.size_title;
        self.crumb_hits.clear();
        // The path is the way back: PT35 is the top screen, and each screen
        // on the way is a link to itself. Only the one in front is plain.
        let trail: Vec<String> = self
            .model
            .trail()
            .iter()
            .map(|t| t.to_uppercase())
            .collect();
        let mut x = pad;
        let start = x;
        x = self
            .mono
            .draw_tracked(canvas, "PT35", x, baseline, size, theme.color.accent, track);
        if trail.len() > 1 {
            self.crumb_hits.push((start - 4, x + 4, 1));
        }
        let current = trail.last().cloned().unwrap_or_default();
        if trail.len() <= 1 {
            self.mono.draw_tracked(
                canvas,
                &format!(" // {current}"),
                x,
                baseline,
                size,
                theme.color.foreground,
                track,
            );
        } else {
            // The screens between the top and this one, fewest first to go
            // when the header runs out of room.
            let room = canvas.width as i32 * 2 / 3;
            let mut middle: Vec<(usize, String)> = trail[1..trail.len() - 1]
                .iter()
                .cloned()
                .enumerate()
                .map(|(i, t)| (i + 2, t))
                .collect();
            let mut widths: Vec<i32> = Vec::new();
            for (_, title) in &middle {
                widths.push(
                    self.mono
                        .measure_tracked(&format!(" < {title}"), size, track)
                        as i32,
                );
            }
            let tail =
                self.mono
                    .measure_tracked(&format!(" < {current}"), size, track) as i32;
            let mut elided = false;
            while !middle.is_empty() && x + widths.iter().sum::<i32>() + tail > room {
                middle.remove(0);
                widths.remove(0);
                elided = true;
            }
            let sep = theme.color.muted;
            if elided {
                x = self
                    .mono
                    .draw_tracked(canvas, " < ..", x, baseline, size, sep, track);
            }
            for (depth, title) in middle {
                x = self
                    .mono
                    .draw_tracked(canvas, " < ", x, baseline, size, sep, track);
                let left = x;
                x = self.mono.draw_tracked(
                    canvas,
                    &title,
                    x,
                    baseline,
                    size,
                    theme.color.accent,
                    track,
                );
                self.crumb_hits.push((left - 4, x + 4, depth));
            }
            x = self
                .mono
                .draw_tracked(canvas, " < ", x, baseline, size, sep, track);
            self.mono.draw_tracked(
                canvas,
                &current,
                x,
                baseline,
                size,
                theme.color.foreground,
                track,
            );
        }
        self.draw_header_readout(canvas, baseline);
        height as i32
    }

    /// `[menu] title_header`: the way back small and dim, then the screen in
    /// front in the display face, with separators like a shell prompt's.
    fn draw_title_trail(&mut self, canvas: &mut Canvas) -> i32 {
        let theme = self.theme.clone();
        let height = theme.menu.header_height as f32;
        let pad = theme.menu.padding_x as i32;
        let track = theme.font.tracking;
        let small = theme.font.size_hint;
        let sep = theme.color.muted;
        let baseline = (height * 0.75) as i32;
        // A pixel face like VT323 draws small for its size.
        let size = if self.display.is_some() {
            height - 2.0
        } else {
            height * 0.56
        };
        let trail: Vec<String> = self
            .model
            .trail()
            .iter()
            .map(|t| t.to_uppercase())
            .collect();
        // (depth, name): PT35 is the top screen, depth 1.
        let (title, mut parents) = match trail.split_last() {
            Some((last, above)) if !above.is_empty() => {
                let mut parents = vec![(1, "PT35".to_string())];
                parents.extend(
                    above[1..]
                        .iter()
                        .cloned()
                        .enumerate()
                        .map(|(i, t)| (i + 2, t)),
                );
                (last.clone(), parents)
            }
            _ => ("PT35".to_string(), Vec::new()),
        };
        let title_w = match self.display.as_mut() {
            Some(face) => face.measure(&title, size),
            None => self.mono.measure(&title, size),
        } as i32;
        // Room for the readout on the right. The screens just under PT35 go
        // first, the way bash's PROMPT_DIRTRIM shortens a path. Depth 0 is the
        // ellipsis that stands in for them: drawn, not a link.
        let room = canvas.width as i32 - 2 * pad - 64 - title_w;
        loop {
            let width: i32 = parents
                .iter()
                .map(|(_, t)| {
                    self.mono
                        .measure_tracked(&format!("{t} \u{276f} "), small, track)
                        as i32
                })
                .sum();
            if width <= room || parents.len() < 2 || (parents.len() == 2 && parents[1].0 == 0) {
                break;
            }
            if parents[1].0 == 0 {
                parents.remove(2);
            } else {
                parents[1] = (0, "\u{2026}".to_string());
            }
        }
        let mut x = pad;
        for (depth, name) in &parents {
            let left = x;
            x = self
                .mono
                .draw_tracked(canvas, name, x, baseline, small, sep, track);
            if *depth > 0 {
                self.crumb_hits.push((left - 4, x + 4, *depth));
            }
            x = self
                .mono
                .draw_tracked(canvas, " \u{276f} ", x, baseline, small, sep, track);
        }
        let face = self.display.as_mut().unwrap_or(&mut self.mono);
        face.draw_glow(
            canvas,
            &title,
            x + 2,
            baseline + 1,
            size,
            theme.color.accent,
            theme.effects.glow,
        );
        baseline
    }

    /// Right of the header: the search field, or where you are in a list
    /// that runs off the screen.
    fn draw_header_readout(&mut self, canvas: &mut Canvas, baseline: i32) {
        let theme = self.theme.clone();
        let pad = theme.menu.padding_x as i32;
        let track = theme.font.tracking;
        let filtering = self.model.screen().list.mode() == Mode::Filter;
        let filter = self.model.screen().list.filter().to_string();
        let list = &self.model.screen().list;
        // The legend already says X searches. What the header can add is where
        // you are in a list that runs off the screen.
        let (text, color) = if filtering && filter.is_empty() {
            (format!("[{}_]", theme.menu.filter_hint), theme.color.muted)
        } else if filtering {
            (format!("[{filter}_]"), theme.color.accent)
        } else if list.len() > list.rows() * list.columns().max(1) && !self.readonly() {
            (
                format!("{}/{}", list.selected().map_or(0, |i| i + 1), list.len()),
                theme.color.muted,
            )
        } else {
            (String::new(), theme.color.muted)
        };
        let width = self
            .mono
            .measure_tracked(&text, theme.font.size_hint, track) as i32;
        self.mono.draw_tracked(
            canvas,
            &text,
            canvas.width as i32 - pad - width,
            baseline,
            theme.font.size_hint,
            color,
            track,
        );
    }

    fn draw_rows(&mut self, canvas: &mut Canvas, top: i32, bottom: i32) {
        let theme = &self.theme;
        let pad = theme.menu.padding_x as i32;
        let row_h = theme.menu.row_height as i32;
        let size = theme.font.size_menu;

        self.row_hits.clear();
        let rows = self.model.visible_rows();
        if rows.is_empty() {
            self.draw_nothing_here(canvas, top, canvas.width as i32);
            return;
        }

        let cursor = self.model.screen().list.cursor_row();
        let numbers = theme.menu.show_numbers && !self.model.is_confirm();
        // App icons are drawn in their own colours, status icons in the text's.
        let full_colour =
            self.model.dynamic_builtin() == Some(pt35_common::menu::Builtin::DesktopEntries);
        let icon_size = 24;
        let has_icons = rows
            .iter()
            .any(|row| !row.icon.is_empty() || row.tint.is_some());
        let radius = theme.menu.radius;
        // The pill is drawn first and on its own, so it can be between rows.
        let pill = self.motion.glide(top + cursor as i32 * row_h);
        canvas.rounded_rect(
            pad / 2,
            pill + 3,
            self.pill_width(canvas.width),
            (row_h - 6) as u32,
            radius,
            theme.color.accent,
        );

        for (index, row) in rows.iter().enumerate() {
            let y = top + index as i32 * row_h;
            if y + row_h > bottom {
                break;
            }
            self.row_hits
                .push((0, y, canvas.width as i32, y + row_h, index));
            // The row under the pill takes the pill's ink, even mid-glide.
            let selected = (pill - y).abs() < row_h / 2;
            if !selected && index + 1 < rows.len() {
                // Hairline between rows. Without it a list of short labels reads
                // as floating text.
                canvas.rect(
                    pad,
                    y + row_h - 1,
                    canvas.width - 2 * pad as u32,
                    1,
                    theme.color.border,
                );
            }
            let fg = if selected {
                theme.color.accent_fg
            } else {
                theme.color.foreground
            };
            let dim = if selected {
                theme.color.accent_fg
            } else {
                theme.color.muted
            };
            let baseline = y + (row_h as f32 * 0.66) as i32;

            let mut x = pad + 6;
            if numbers && index < 9 {
                let number = format!("{}", index + 1);
                self.mono
                    .draw(canvas, &number, x, baseline, theme.font.size_hint, dim);
                x += 22;
            }
            let centre = y + row_h / 2;
            let framed = theme.menu.row_frames && has_icons;
            if framed {
                let colour = if selected { fg } else { theme.color.border };
                canvas.frame(x, centre - 16, 32, 32, colour);
                x += 4;
            }
            if let Some(swatch) = row.tint {
                canvas.rounded_rect(x, centre - 10, 20, 20, radius, fg);
                canvas.rounded_rect(x + 2, centre - 8, 16, 16, radius, swatch);
            } else if full_colour {
                let name = self.icons.resolve(&[&row.icon], icon_size);
                if let Some(icon) = name.and_then(|name| self.icons.get(&name, icon_size)) {
                    icon.draw(canvas, x, centre - icon_size as i32 / 2);
                }
            } else if let Some(icon) = self.icons.get_symbolic(&row.icon, icon_size) {
                icon.draw_tinted(
                    canvas,
                    x,
                    centre - icon_size as i32 / 2,
                    if selected { fg } else { theme.color.muted },
                );
            } else if has_icons {
                // No such icon in the theme, or no theme: the letter stands in.
                let letter = row.label.chars().next().unwrap_or('?').to_string();
                let w = self.bold.measure(&letter, theme.font.size_hint) as i32;
                self.bold.draw(
                    canvas,
                    &letter,
                    x + (icon_size as i32 - w) / 2,
                    centre + (theme.font.size_hint / 3.0) as i32,
                    theme.font.size_hint,
                    dim,
                );
            }
            // Indented on every row or none, so labels stay in one column.
            if has_icons {
                x += icon_size as i32 + if framed { 16 } else { 12 };
            }

            // Quick settings read out their value on the right, where the
            // chevron would be on a row that opens something.
            let right_text = match (row.adjust, row.state) {
                (Some(adjust), _) => Some(crate::live::value(adjust, self.status.as_ref())),
                (None, Some(field)) => Some(crate::live::state_value(field, self.status.as_ref())),
                (None, None) if row.submenu => Some(">".to_string()),
                (None, None) if !row.note.is_empty() => Some(row.note.clone()),
                (None, None) => None,
            };
            let from_note = row.adjust.is_none() && row.state.is_none() && !row.submenu;
            let mut right_width = 0;
            if let Some(text) = &right_text {
                let mono = row.adjust.is_some() || row.state.is_some() || !row.submenu;
                let size_right = if mono { theme.font.size_hint } else { size };
                let width = if mono {
                    self.mono.measure(text, size_right) as i32
                } else {
                    self.font.measure(text, size_right) as i32
                };
                let tx = canvas.width as i32 - pad - width;
                let colour = match (selected, from_note) {
                    (true, _) => fg,
                    (false, true) => theme.color.muted,
                    (false, false) => theme.color.accent,
                };
                if mono {
                    self.mono
                        .draw(canvas, text, tx, baseline, size_right, colour);
                } else {
                    self.font.draw(canvas, text, tx, baseline, size_right, dim);
                }
                right_width = width + 10;
            }
            // A tick for the one row that is current, left of its readout.
            let current_cpu = self
                .status
                .as_ref()
                .and_then(|s| s.cpu_profile)
                .is_some_and(|p| row.payload == format!("action:cpu {}", p.label()));
            if row.active || current_cpu {
                let tick = "\u{2713}";
                let w = self.bold.measure(tick, size) as i32;
                let colour = if selected { fg } else { theme.color.accent };
                right_width += w + 8;
                self.bold.draw(
                    canvas,
                    tick,
                    canvas.width as i32 - pad - right_width + 8,
                    baseline,
                    size,
                    colour,
                );
            }

            let room = canvas
                .width
                .saturating_sub(x as u32 + pad as u32 + right_width as u32);
            let label = self.font.elide(&row.label, size, room);
            // Red for a row that ends the session, so it reads as one before
            // the confirmation does.
            let ink = if row.confirm && !selected {
                theme.color.critical
            } else {
                fg
            };
            self.font.draw(canvas, &label, x, baseline, size, ink);
        }

        // Scroll indicator: a slim bar on the right, only when it means something.
        self.draw_scrollbar(canvas, canvas.width as i32, top, bottom);
    }

    /// What an empty screen says. Drawing nothing at all reads as broken
    /// rather than empty.
    fn draw_nothing_here(&mut self, canvas: &mut Canvas, top: i32, right: i32) {
        let filtering = self.model.screen().list.mode() == Mode::Filter;
        let text = if filtering {
            "no matches"
        } else if self.status.is_none() {
            // Not the same thing as nothing being there. A dead daemon returns
            // an empty list from every screen that asks it.
            "pt35d is not answering"
        } else {
            "nothing here"
        };
        let size = self.theme.font.size_menu;
        let pad = self.theme.menu.padding_x as i32;
        let colour = if self.status.is_none() && !filtering {
            self.theme.color.critical
        } else {
            self.theme.color.muted
        };
        let text = self.font.elide(text, size, (right - 2 * pad).max(0) as u32);
        self.font.draw(canvas, &text, pad, top + 40, size, colour);
    }

    /// The only thing that says a list goes on past the bottom of the screen.
    ///
    /// `edge` is the right-hand boundary of the list, which is the panel edge on
    /// most screens and the split on the launcher.
    /// The selection pill stops short of the scrollbar rather than run under it.
    fn pill_width(&self, width: u32) -> u32 {
        let pad = self.theme.menu.padding_x / 2;
        let right = if self.model.screen().list.scroll_progress().is_some() {
            width.saturating_sub(self.theme.menu.scrollbar_width + 4)
        } else {
            width.saturating_sub(pad)
        };
        right.saturating_sub(pad)
    }

    fn draw_scrollbar(&mut self, canvas: &mut Canvas, edge: i32, top: i32, bottom: i32) {
        let Some(progress) = self.model.screen().list.scroll_progress() else {
            return;
        };
        let theme = &self.theme;
        let total = self.model.screen().list.len().max(1);
        let list = &self.model.screen().list;
        let visible = list.rows() * list.columns();
        let track_h = (bottom - top).max(1) as u32;
        let thumb_h = ((visible as f32 / total as f32) * track_h as f32)
            .max(24.0)
            .min(track_h as f32) as u32;
        let thumb_y = top + (progress.clamp(0.0, 1.0) * (track_h - thumb_h) as f32) as i32;
        let bar = theme.menu.scrollbar_width as i32;
        let left = edge - bar - 2;
        let radius = theme.menu.radius;
        canvas.rounded_rect(left, top, bar as u32, track_h, radius, theme.color.border);
        canvas.rounded_rect(
            left,
            thumb_y,
            bar as u32,
            thumb_h,
            radius,
            theme.color.muted,
        );
        // Wider than it is drawn: a bar a thumb can hit would crowd the rows.
        self.scrollbar = Some((left - SCROLLBAR_SLOP, edge, top, bottom, thumb_h as i32));
    }

    /// Put the list where the pointer is on the scrollbar, the thumb centred
    /// under it. The cursor comes along into view.
    fn scroll_to(&mut self, y: i32) -> bool {
        let Some((_, _, top, bottom, thumb)) = self.scrollbar else {
            return false;
        };
        let room = (bottom - top - thumb).max(1);
        let fraction = (y - top - thumb / 2) as f32 / room as f32;
        self.model.screen_mut().list.scroll_to(fraction)
    }

    fn on_scrollbar(&self, x: i32, y: i32) -> bool {
        self.scrollbar
            .is_some_and(|(l, r, t, b, _)| x >= l && x < r && y >= t && y < b)
    }

    /// Pill colour and text colour for a legend entry.
    fn button_colors(&self, button: &str) -> (Rgb, Rgb) {
        let c = &self.theme.color;
        match button {
            "A" => (c.button_a, c.accent_fg),
            "B" => (c.button_b, c.accent_fg),
            "X" => (c.button_x, c.background),
            "Y" => (c.button_y, c.background),
            "Start" => (c.ok, c.background),
            _ => (c.button_neutral, c.foreground),
        }
    }

    fn draw_tiles(&mut self, canvas: &mut Canvas, top: i32, bottom: i32) {
        let theme = self.theme.clone();
        let pad = theme.menu.padding_x as i32;
        let gap = theme.menu.gap as i32;
        let columns = theme.menu.columns.max(1) as i32;
        let tile_w = (canvas.width as i32 - pad * 2 - gap * (columns - 1)) / columns;

        let rows = self.model.visible_rows();
        if rows.is_empty() {
            self.row_hits.clear();
            self.draw_nothing_here(canvas, top, canvas.width as i32);
            return;
        }
        // Grow the tiles to fill the body rather than leaving a dead band under
        // a short grid. Never shrink below the configured height.
        let lines = ((rows.len() as i32 + columns - 1) / columns).max(1);
        let available = bottom - top;
        // Grown to fill the body, but only so far: two windows in the picker
        // must not become two half-screen slabs.
        let tile_h = ((available - gap * (lines - 1)) / lines).clamp(
            theme.menu.tile_height as i32,
            theme.menu.tile_height as i32 * 3 / 2,
        );
        let radius = theme.menu.radius;

        let cursor = self.model.screen().list.cursor_index();
        self.row_hits.clear();

        for (index, row) in rows.iter().enumerate() {
            let column = index as i32 % columns;
            let line = index as i32 / columns;
            let x = pad + column * (tile_w + gap);
            let y = top + line * (tile_h + gap);
            if y + tile_h > bottom {
                break;
            }
            self.row_hits.push((x, y, x + tile_w, y + tile_h, index));
            let focused = index == cursor;
            let tint = row.tint.unwrap_or(theme.color.accent);
            let note = row
                .builtin
                .and_then(|b| crate::live::note(b, self.status.as_ref(), self.windows))
                .unwrap_or_else(|| row.note.clone());

            if focused {
                // Halo, then border, then face: the focused tile is the only
                // thing on screen allowed to glow.
                canvas.rounded_rect(
                    x - 2,
                    y - 2,
                    (tile_w + 4) as u32,
                    (tile_h + 4) as u32,
                    radius + 2,
                    tint,
                );
                canvas.rounded_rect(
                    x + 1,
                    y + 1,
                    (tile_w - 2) as u32,
                    (tile_h - 2) as u32,
                    radius,
                    theme.color.background_alt,
                );
            } else {
                canvas.rounded_rect(
                    x,
                    y,
                    tile_w as u32,
                    tile_h as u32,
                    radius,
                    theme.color.border,
                );
                canvas.rounded_rect(
                    x + 1,
                    y + 1,
                    (tile_w - 2) as u32,
                    (tile_h - 2) as u32,
                    radius,
                    theme.color.background_alt,
                );
            }

            let badge = 40;
            let bx = x + 12;
            let by = y + (tile_h - badge) / 2;
            let icon_size = theme.icons.size_tile;
            let resolved = self.icons.resolve(&[&row.icon], icon_size);
            let has_icon = resolved.is_some();
            if row.icon == providers::SKULL_ICON {
                canvas.rounded_rect(bx, by, badge as u32, badge as u32, theme.menu.radius, tint);
                pt35_ui::pixel::draw_centred(
                    canvas,
                    pt35_ui::pixel::SKULL,
                    bx + badge / 2,
                    by + badge / 2,
                    2,
                    theme.color.accent_fg,
                );
            } else if has_icon {
                // The icon carries the colour, so the badge steps back to a
                // tinted outline.
                canvas.rounded_rect(bx, by, badge as u32, badge as u32, theme.menu.radius, tint);
                canvas.rounded_rect(
                    bx + 1,
                    by + 1,
                    (badge - 2) as u32,
                    (badge - 2) as u32,
                    theme.menu.radius,
                    theme.color.background,
                );
                let inset = (badge - icon_size as i32) / 2;
                let name = resolved.unwrap_or_default();
                if let Some(icon) = self.icons.get(&name, icon_size) {
                    icon.draw(canvas, bx + inset, by + inset);
                }
            } else {
                canvas.rounded_rect(bx, by, badge as u32, badge as u32, theme.menu.radius, tint);
                let glyph_size = theme.font.size_title;
                let glyph_w = self.font.measure(&row.glyph, glyph_size) as i32;
                self.font.draw(
                    canvas,
                    &row.glyph,
                    bx + (badge - glyph_w) / 2,
                    by + badge / 2 + (glyph_size / 3.0) as i32,
                    glyph_size,
                    theme.color.background,
                );
            }

            if focused {
                // Corner ticks: instrument framing, and they survive on a busy
                // wallpaper better than a full ring.
                let t = 10;
                for (cx, cy, dx, dy) in [
                    (x + 4, y + 4, 1, 1),
                    (x + tile_w - 5, y + 4, -1, 1),
                    (x + 4, y + tile_h - 5, 1, -1),
                    (x + tile_w - 5, y + tile_h - 5, -1, -1),
                ] {
                    canvas.rect(cx.min(cx + dx * t), cy, t as u32, 2, tint);
                    canvas.rect(cx, cy.min(cy + dy * t), 2, t as u32, tint);
                }
            }

            let text_x = bx + badge + 10;
            let room = (x + tile_w - text_x - 8).max(8) as u32;
            let label = self.font.elide(&row.label, theme.font.size_menu, room);
            let has_note = !note.is_empty();
            let label_y = if has_note {
                y + tile_h / 2 - 2
            } else {
                y + tile_h / 2 + 7
            };
            self.font.draw(
                canvas,
                &label,
                text_x,
                label_y,
                theme.font.size_menu,
                theme.color.foreground,
            );
            if has_note {
                let note = self.mono.elide(&note, theme.font.size_hint, room);
                self.mono.draw(
                    canvas,
                    &note,
                    text_x,
                    label_y + 18,
                    theme.font.size_hint,
                    theme.color.muted,
                );
            }
        }
    }

    /// The widget column: the time and the machine on one panel, the network
    /// on the other, as on a deck's status screen.
    fn draw_widgets(&mut self, canvas: &mut Canvas, x: i32, top: i32, bottom: i32) {
        let theme = self.theme.clone();
        let gap = theme.menu.gap as i32;
        let w = WIDGET_WIDTH;
        let small = theme.font.size_hint;
        // Half and half: the machine on top, the pile under it.
        let system_h = (bottom - top - gap) / 2;
        self.draw_panel(canvas, x, top, w, system_h, "SYSTEM");

        let now = chrono::Local::now();
        let time = now.format(&theme.bar.clock_format).to_string();
        let (clock, big) = match self.display.as_mut() {
            // A pixel face like VT323 draws small for its size.
            Some(display) => (display, 72.0),
            None => (&mut self.mono, 58.0),
        };
        let tw = clock.measure(&time, big) as i32;
        let baseline = top + 26 + (big * 0.72) as i32;
        clock.draw_glow(
            canvas,
            &time,
            x + (w - tw) / 2,
            baseline,
            big,
            theme.color.foreground,
            theme.effects.glow,
        );
        let date = now.format("%a %d %b %Y").to_string().to_uppercase();
        let dw = self.mono.measure(&date, small) as i32;
        self.mono.draw(
            canvas,
            &date,
            x + (w - dw) / 2,
            baseline + 24,
            small,
            theme.color.muted,
        );

        let r = &self.readings;
        let gauges = [
            (
                "CPU",
                r.cpu.map(|c| c as f32 / 100.0),
                r.cpu.map(|c| format!("{c}%")),
            ),
            (
                "RAM",
                r.memory.map(|m| m as f32 / 100.0),
                r.memory.map(|m| format!("{m}%")),
            ),
            (
                "TEMP",
                r.temperature.map(|t| ((t - 30.0) / 55.0).clamp(0.0, 1.0)),
                r.temperature.map(|t| format!("{t:.0}°")),
            ),
        ];
        let mut y = baseline + 44;
        for (label, level, value) in gauges {
            // A gauge with nothing to read is left out, never shown empty.
            let (Some(level), Some(value)) = (level, value) else {
                continue;
            };
            self.mono
                .draw(canvas, label, x + 12, y + 10, small, theme.color.muted);
            self.draw_segments(canvas, x + 64, y, 96, level);
            let vw = self.mono.measure(&value, small) as i32;
            self.mono.draw(
                canvas,
                &value,
                x + w - 12 - vw,
                y + 10,
                small,
                theme.color.foreground,
            );
            y += 22;
        }

        let pile_top = top + system_h + gap;
        let (h, count) = (bottom - pile_top, self.widgets.len());
        self.widget_hits.clear();
        let Some(&widget) = self.widgets.get(self.widget.min(count.saturating_sub(1))) else {
            self.draw_panel(canvas, x, pile_top, w, h, "WIDGETS");
            let text = self.font.elide("X: All widgets", small, (w - 24) as u32);
            self.font.draw(
                canvas,
                &text,
                x + 12,
                pile_top + 40,
                small,
                theme.color.muted,
            );
            return;
        };
        let title = widget.label().to_uppercase();
        self.draw_panel(canvas, x, pile_top, w, h, &title);
        if count > 1 {
            // Where you are in the pile, in words: "2/3", then an arrow drawn
            // as a shape, since a tap on the header is the next one.
            let text = format!("{}/{}", self.widget + 1, count);
            let tw = self.mono.measure(&text, small) as i32;
            let (ax, ay) = (x + w - 16, pile_top + 4);
            for i in 0..5 {
                canvas.rect(
                    ax + i,
                    ay + i,
                    1,
                    (10 - 2 * i) as u32,
                    theme.color.foreground,
                );
            }
            self.mono.draw(
                canvas,
                &text,
                ax - 8 - tw,
                pile_top + 14,
                small,
                theme.color.foreground,
            );
            // As tall as a finger needs, over the top of the panel.
            self.widget_hits
                .push((x, pile_top, x + w, pile_top + 32, WidgetHit::Next));
        }
        match widget {
            Widget::Network => self.draw_network(canvas, x, pile_top, w, h),
            Widget::Media => self.draw_media(canvas, x, pile_top, w, h),
            Widget::Storage => self.draw_storage(canvas, x, pile_top, w, h),
            Widget::Hosts => self.draw_hosts(canvas, x, pile_top, w, h),
        }
    }

    /// The route in use, LAN or Wi-Fi, and its last minutes on a log graph.
    fn draw_network(&mut self, canvas: &mut Canvas, x: i32, top: i32, w: i32, h: i32) {
        let theme = self.theme.clone();
        let small = theme.font.size_hint;
        let status = self.status.clone().unwrap_or_default();
        let (kind, name) = match (&status.network, &status.ethernet, &self.readings.ssid) {
            (None, _, _) => ("OFFLINE", None),
            (Some(iface), Some(wired), _) if iface == wired => ("LAN", None),
            (Some(_), _, Some(ssid)) => ("WIFI", Some(ssid.clone())),
            (Some(iface), _, None) => ("NET", Some(iface.clone())),
        };
        let y = top + 40;
        self.mono_bold
            .draw(canvas, kind, x + 12, y, small, theme.color.foreground);
        if let Some(name) = name {
            let nx = x + 12 + self.mono_bold.measure(kind, small) as i32 + 8;
            let name = self
                .font
                .elide(&name, small, (x + w - 12 - nx).max(0) as u32);
            self.font
                .draw(canvas, &name, nx, y, small, theme.color.foreground);
        }
        if status.network.is_none() {
            return;
        }
        if let Some(address) = self.readings.address.clone() {
            self.mono
                .draw(canvas, &address, x + 12, y + 20, small, theme.color.muted);
        }
        let chart = (x + 12, y + 30, w - 24, top + h - 30 - (y + 30));
        let samples = self
            .readings
            .traffic
            .as_ref()
            .filter(|t| t.iface == status.network)
            .map(|t| t.samples.clone())
            .unwrap_or_default();
        self.draw_traffic(canvas, chart, &samples);
        let Some([down, up]) = samples.last().copied() else {
            return;
        };
        // The legend is a shape, not a hue: a filled swatch for down, a dotted
        // one for up, as on the graph.
        let size = small * 0.875;
        let base = top + h - 10;
        let mut tx = x + 12;
        for (i, text) in [
            format!("↓{}", crate::widgets::rate(down)),
            format!("↑{}", crate::widgets::rate(up)),
        ]
        .iter()
        .enumerate()
        {
            if i == 0 {
                canvas.rect_alpha(tx, base - 9, 8, 9, theme.color.foreground, 150);
            } else {
                canvas.rect(tx, base - 5, 3, 2, theme.color.accent);
                canvas.rect(tx + 5, base - 5, 3, 2, theme.color.accent);
            }
            tx += 10;
            self.mono
                .draw(canvas, text, tx, base, size, theme.color.foreground);
            tx += self.mono.measure(text, size) as i32 + 10;
        }
    }

    /// A word on a filled tag: the state reads without its colour.
    fn draw_tag(&mut self, canvas: &mut Canvas, right: i32, top: i32, text: &str, fill: Rgb) {
        let size = self.theme.font.size_hint * 0.8;
        let tw = self.mono_bold.measure(text, size) as i32;
        let w = tw + 12;
        canvas.rect(right - w, top, w as u32, 18, fill);
        let ink = self.theme.color.background;
        self.mono_bold
            .draw(canvas, text, right - w + 6, top + 14, size, ink);
    }

    /// The system drive and any USB one: a gauge each and the space left.
    /// Green, then amber past 75%, then red and FULL past 90%. A tap opens
    /// Files.
    fn draw_storage(&mut self, canvas: &mut Canvas, x: i32, top: i32, w: i32, h: i32) {
        let theme = self.theme.clone();
        let small = theme.font.size_hint * 0.875;
        let volumes = match self.readings.volumes.clone() {
            Some(v) if !v.is_empty() => v,
            v => {
                let text = if v.is_none() {
                    "Reading..."
                } else {
                    "No drive"
                };
                self.mono
                    .draw(canvas, text, x + 12, top + 40, small, theme.color.muted);
                return;
            }
        };
        for (i, v) in volumes.iter().take(3).enumerate() {
            let y = top + 30 + i as i32 * 50;
            let used = v.used();
            let colour = match used {
                u if u >= 0.9 => theme.color.critical,
                u if u >= 0.75 => theme.color.warning,
                _ => theme.color.ok,
            };
            self.mono_bold.draw(
                canvas,
                v.kind,
                x + 12,
                y + 14,
                small,
                theme.color.foreground,
            );
            let nx = x + 12 + self.mono_bold.measure(v.kind, small) as i32 + 8;
            let free = crate::widgets::bytes(v.free);
            let mut right = x + w - 12;
            if used >= 0.9 {
                self.draw_tag(canvas, right, y + 1, "FULL", colour);
                right -= 52;
            }
            let fw = self.mono.measure(&free, small) as i32;
            self.mono
                .draw(canvas, &free, right - fw, y + 14, small, theme.color.muted);
            let name = self
                .font
                .elide(&v.name, small, (right - fw - 8 - nx).max(0) as u32);
            self.font
                .draw(canvas, &name, nx, y + 14, small, theme.color.foreground);
            self.draw_segments_in(canvas, x + 12, y + 22, w - 24, used, colour);
        }
        self.widget_hits.push((
            x,
            top + 32,
            x + w,
            top + h,
            WidgetHit::Run(crate::model::Command::App("files".into())),
        ));
    }

    /// The pinned hosts of `~/.ssh/config`, as pt35d last found them. UP,
    /// SLOW past 150 ms, DOWN on a filled tag. A tap on a row connects.
    fn draw_hosts(&mut self, canvas: &mut Canvas, x: i32, top: i32, w: i32, h: i32) {
        let theme = self.theme.clone();
        let small = theme.font.size_hint * 0.8;
        let hosts = self.readings.hosts.clone();
        if hosts.is_empty() {
            let text = self
                .font
                .elide("All apps > SSH: Y pins", small, (w - 24) as u32);
            self.font
                .draw(canvas, &text, x + 12, top + 40, small, theme.color.muted);
            return;
        }
        let row_h = ((h - 34) / 3).min(50);
        for (i, host) in hosts.iter().take(3).enumerate() {
            let y = top + 26 + i as i32 * row_h;
            let (rx, rw, rh) = (x + 8, w - 16, row_h - 4);
            canvas.frame(rx, y, rw as u32, rh as u32, theme.color.border);
            let (word, colour, filled) = match (host.up, host.latency_ms) {
                (None, _) => ("CHECK", theme.color.muted, false),
                (Some(false), _) => ("DOWN", theme.color.critical, true),
                (Some(true), Some(ms)) if ms > 150 => ("SLOW", theme.color.warning, false),
                (Some(true), _) => ("UP", theme.color.ok, false),
            };
            let (tx, tw) = (rx + 6, 52);
            if filled {
                canvas.rect(tx, y + 6, tw as u32, 18, colour);
            } else {
                canvas.frame(tx, y + 6, tw as u32, 18, colour);
            }
            let ink = if filled {
                theme.color.background
            } else {
                colour
            };
            let lw = self.mono_bold.measure(word, small) as i32;
            self.mono_bold
                .draw(canvas, word, tx + (tw - lw) / 2, y + 20, small, ink);
            let name = self.font.elide(
                &host.name,
                small * 1.1,
                (rx + rw - 6 - (tx + tw + 8)) as u32,
            );
            self.font.draw(
                canvas,
                &name,
                tx + tw + 8,
                y + 20,
                small * 1.1,
                theme.color.foreground,
            );
            let detail = match (host.up, host.latency_ms) {
                (Some(true), Some(0)) => "<1 ms".into(),
                (Some(true), Some(ms)) => format!("{ms} ms"),
                (Some(false), _) => "no answer".into(),
                _ => String::new(),
            };
            let dw = self.mono.measure(&detail, small) as i32;
            self.mono.draw(
                canvas,
                &detail,
                rx + rw - 6 - dw,
                y + rh - 7,
                small,
                if host.up == Some(false) {
                    theme.color.foreground
                } else {
                    theme.color.muted
                },
            );
            if let Some(address) = &host.address {
                let address = self
                    .mono
                    .elide(address, small, (rw - 18 - dw).max(0) as u32);
                self.mono
                    .draw(canvas, &address, tx, y + rh - 7, small, theme.color.muted);
            }
            self.widget_hits.push((
                rx,
                y,
                rx + rw,
                y + rh,
                WidgetHit::Run(crate::model::Command::Dynamic {
                    builtin: pt35_common::menu::Builtin::Ssh,
                    payload: format!("host:{}", host.name),
                }),
            ));
        }
    }

    /// Down as a filled area under a line, up as a dotted line, newest on the
    /// right. Lines at 1k, 1M and 1G: the scale's units.
    fn draw_traffic(
        &mut self,
        canvas: &mut Canvas,
        (x, y, w, h): (i32, i32, i32, i32),
        samples: &[[f32; 2]],
    ) {
        let theme = self.theme.clone();
        if w < 8 || h < 8 {
            return;
        }
        canvas.rect_alpha(x, y, w as u32, h as u32, theme.color.background, 110);
        let label = theme.font.size_hint * 0.7;
        for (level, text) in [(1.0, "1G"), (0.5, "1M"), (0.0, "1k")] {
            let ly = y + h - 1 - ((h - 1) as f32 * level) as i32;
            for dx in (0..w).step_by(5) {
                canvas.rect(x + dx, ly, 2, 1, theme.color.border);
            }
            let tw = self.mono.measure(text, label) as i32;
            let ty = if level == 1.0 { ly + 10 } else { ly - 2 };
            if level > 0.0 {
                self.mono
                    .draw(canvas, text, x + w - 2 - tw, ty, label, theme.color.muted);
            }
        }
        self.mono
            .draw(canvas, "-3 min", x + 2, y + 10, label, theme.color.muted);
        if samples.is_empty() {
            return;
        }
        let step = (w - 1) as f32 / (pt35_common::ipc::TRAFFIC_SAMPLES - 1) as f32;
        let at = |column: i32, which: usize| -> Option<i32> {
            // Samples back from the newest, at this column.
            let age = (w - 1 - column) as f32 / step;
            let pos = samples.len() as f32 - 1.0 - age;
            if pos < 0.0 {
                return None;
            }
            let (i, f) = (pos.floor() as usize, pos.fract());
            let a = samples[i][which];
            let b = samples.get(i + 1).map_or(a, |s| s[which]);
            let level = crate::widgets::log_level(a + (b - a) * f);
            Some(y + h - 1 - ((h - 1) as f32 * level) as i32)
        };
        let (mut last_down, mut last_up) = (None, None);
        for column in 0..w {
            let cx = x + column;
            if let Some(dy) = at(column, 0) {
                canvas.rect_alpha(cx, dy, 1, (y + h - dy) as u32, theme.color.foreground, 90);
                let from = last_down.unwrap_or(dy);
                let (lo, hi) = (from.min(dy), from.max(dy));
                canvas.rect(cx, lo, 1, (hi - lo + 2) as u32, theme.color.foreground);
                last_down = Some(dy);
            }
            if let Some(uy) = at(column, 1) {
                if column % 6 < 3 {
                    let from = last_up.unwrap_or(uy);
                    let (lo, hi) = (from.min(uy), from.max(uy));
                    canvas.rect(cx, lo, 1, (hi - lo + 2) as u32, theme.color.accent);
                }
                last_up = Some(uy);
            }
        }
    }

    /// The player: what plays, how far in, and its three buttons. The state is
    /// a shape and a word, never a colour alone. A tap elsewhere on the panel
    /// plays or pauses, as before the buttons.
    fn draw_media(&mut self, canvas: &mut Canvas, x: i32, top: i32, w: i32, h: i32) {
        let theme = self.theme.clone();
        let small = theme.font.size_hint;
        let (fg, muted) = (theme.color.foreground, theme.color.muted);
        let bottom = top + h;
        // The cover's place: the theme's music icon in a frame.
        let (bx, by, bs) = (x + 12, top + 28, 44);
        canvas.frame(bx, by, bs as u32, bs as u32, theme.color.border);
        if let Some(icon) = self.icons.get_symbolic("audio-x-generic", 32) {
            icon.draw_tinted(canvas, bx + 6, by + 6, fg);
        } else {
            canvas.circle(bx + 19, by + 30, 5, fg);
            canvas.rect(bx + 22, by + 10, 2, 20, fg);
            canvas.rect(bx + 22, by + 10, 9, 3, fg);
        }
        let tx = bx + bs + 10;
        let room = (x + w - 12 - tx).max(0) as u32;
        let Some(media) = self.readings.media.clone() else {
            self.mono
                .draw(canvas, "STOPPED", tx, top + 44, small, muted);
            let text = self.font.elide("Nothing playing", small, room);
            self.font.draw(canvas, &text, tx, top + 64, small, muted);
            return;
        };
        let (ix, iy) = (tx, top + 33);
        if media.playing {
            for i in 0..5 {
                canvas.rect(ix + i, iy + i, 1, (10 - 2 * i) as u32, fg);
            }
        } else {
            canvas.rect(ix, iy, 3, 10, fg);
            canvas.rect(ix + 5, iy, 3, 10, fg);
        }
        let state = if media.playing { "PLAYING" } else { "PAUSED" };
        self.mono
            .draw(canvas, state, tx + 12, top + 43, small * 0.875, fg);
        let title = self.font.elide(&media.title, small, room);
        self.font.draw(canvas, &title, tx, top + 62, small, fg);
        if let Some(artist) = &media.artist {
            let artist = self.font.elide(artist, small * 0.875, room);
            self.font
                .draw(canvas, &artist, tx, top + 79, small * 0.875, muted);
        }
        if let (Some(position), Some(length)) = (media.position, media.length) {
            let level = (position / length).clamp(0.0, 1.0) as f32;
            self.draw_segments(canvas, x + 12, top + 86, w - 24, level);
            let text = format!(
                "{} / {}",
                crate::widgets::clock(position),
                crate::widgets::clock(length)
            );
            self.mono
                .draw(canvas, &text, x + 12, top + 114, small, muted);
        }
        // Previous, play or pause, next: targets a thumb can hit, the middle
        // one filled as the main action.
        let (bw, bh, gap) = (60, 40, (w - 24 - 3 * 60) / 2);
        let by = bottom - 12 - bh;
        for (i, command) in ["previous", "play-pause", "next"].into_iter().enumerate() {
            let bx = x + 12 + i as i32 * (bw + gap);
            let main = i == 1;
            let ink = if main {
                canvas.rect(bx, by, bw as u32, bh as u32, fg);
                theme.color.background
            } else {
                canvas.frame(bx, by, bw as u32, bh as u32, theme.color.border);
                fg
            };
            let (cx, cy) = (bx + bw / 2, by + bh / 2);
            let triangle = |canvas: &mut Canvas, left: i32, forward: bool| {
                for k in 0..7 {
                    let col = if forward { left + k } else { left + 6 - k };
                    canvas.rect(col, cy - 7 + k, 1, (14 - 2 * k) as u32, ink);
                }
            };
            match command {
                "previous" => {
                    canvas.rect(cx - 8, cy - 7, 2, 14, ink);
                    triangle(canvas, cx - 5, false);
                    triangle(canvas, cx + 2, false);
                }
                "next" => {
                    triangle(canvas, cx - 8, true);
                    triangle(canvas, cx - 1, true);
                    canvas.rect(cx + 6, cy - 7, 2, 14, ink);
                }
                _ if media.playing => {
                    canvas.rect(cx - 6, cy - 7, 4, 14, ink);
                    canvas.rect(cx + 2, cy - 7, 4, 14, ink);
                }
                _ => triangle(canvas, cx - 3, true),
            }
            self.widget_hits
                .push((bx, by, bx + bw, by + bh, WidgetHit::Player(command)));
        }
        // The rest of the panel under the header plays or pauses.
        self.widget_hits
            .push((x, top + 32, x + w, bottom, WidgetHit::Player("play-pause")));
    }

    /// A panel of the widget column: a flat surface with its name on a tab.
    fn draw_panel(&mut self, canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, title: &str) {
        let theme = self.theme.clone();
        let (fw, fh) = (w as u32, h.max(2) as u32);
        let alpha = opacity(theme.menu.tile_opacity);
        canvas.rect_alpha(x + 1, y + 1, fw - 2, fh - 2, theme.color.tile(), alpha);
        canvas.frame(x, y, fw, fh, theme.color.border);
        let size = theme.font.size_hint * 0.8;
        let tw = self
            .mono_bold
            .measure_tracked(title, size, theme.font.tracking) as i32;
        canvas.rect(x, y, (tw + 12) as u32, 18, theme.color.foreground);
        self.mono_bold.draw_tracked(
            canvas,
            title,
            x + 6,
            y + 14,
            size,
            theme.color.background,
            theme.font.tracking,
        );
    }

    /// A segmented bar: eight blocks, lit up to `level` (0 to 1).
    fn draw_segments(&mut self, canvas: &mut Canvas, x: i32, y: i32, w: i32, level: f32) {
        let lit = self.theme.color.foreground;
        self.draw_segments_in(canvas, x, y, w, level, lit);
    }

    fn draw_segments_in(
        &mut self,
        canvas: &mut Canvas,
        x: i32,
        y: i32,
        w: i32,
        level: f32,
        lit_colour: Rgb,
    ) {
        let theme = &self.theme;
        let count = 8;
        let gap = 3;
        let seg = (w - gap * (count - 1)) / count;
        let lit = (level.clamp(0.0, 1.0) * count as f32).round() as i32;
        for i in 0..count {
            let color = if i < lit {
                lit_colour
            } else {
                theme.color.border
            };
            canvas.rect(x + i * (seg + gap), y, seg as u32, 10, color);
        }
    }

    /// The launcher's apps as a grid: a big icon over its name. The picked
    /// tile is filled in the accent and its icon drawn in `accent_fg`, so the
    /// selection reads from shape and contrast, not hue.
    fn draw_app_tiles(
        &mut self,
        canvas: &mut Canvas,
        rows: &[crate::model::Row],
        left: i32,
        top: i32,
        bottom: i32,
        split: i32,
    ) {
        let theme = self.theme.clone();
        let gap = theme.menu.gap as i32;
        let bar = theme.menu.scrollbar_width as i32 + 4;
        let list = &self.model.screen().list;
        let (columns, lines) = (list.columns() as i32, list.rows() as i32);
        let cursor = list.cursor_index();
        let tile_w = (split - left - bar - gap * columns) / columns;
        let tile_h = (bottom - top - gap * (lines - 1)) / lines;
        let icon_size = theme.icons.size_launcher;
        let size = theme.font.size_hint;

        for (index, row) in rows.iter().enumerate() {
            let x = left + (index as i32 % columns) * (tile_w + gap);
            let y = top + (index as i32 / columns) * (tile_h + gap);
            if y + tile_h > bottom {
                break;
            }
            self.row_hits.push((x, y, x + tile_w, y + tile_h, index));
            let focused = index == cursor && self.side.is_none();
            let (face, ink) = if focused {
                (theme.color.accent, theme.color.accent_fg)
            } else {
                (theme.color.tile(), theme.color.foreground)
            };
            let alpha = opacity(theme.menu.tile_opacity);
            if focused || alpha == 255 {
                let radius = theme.menu.radius;
                canvas.rounded_rect(
                    x,
                    y,
                    tile_w as u32,
                    tile_h as u32,
                    radius,
                    theme.color.border,
                );
                canvas.rounded_rect(
                    x + 1,
                    y + 1,
                    (tile_w - 2) as u32,
                    (tile_h - 2) as u32,
                    radius,
                    face,
                );
            } else {
                let (w, h) = (tile_w as u32, tile_h as u32);
                canvas.rect_alpha(x + 1, y + 1, w - 2, h - 2, face, alpha);
                canvas.frame(x, y, w, h, theme.color.border);
            }

            let ix = x + (tile_w - icon_size as i32) / 2;
            let iy = y + (tile_h - icon_size as i32 - LAUNCHER_LABEL as i32) / 2 + 4;
            let name = self.icons.resolve(&[&row.icon], icon_size);
            let drawn = name
                .and_then(|name| self.icons.get(&name, icon_size))
                .map(|icon| {
                    if focused {
                        icon.draw_tinted(canvas, ix, iy, ink);
                    } else {
                        icon.draw_glow(canvas, ix, iy, theme.icons.glow, theme.color.foreground);
                        icon.draw(canvas, ix, iy);
                    }
                })
                .is_some();
            if !drawn {
                let glyph = if row.glyph.is_empty() {
                    row.label.chars().next().unwrap_or('?').to_string()
                } else {
                    row.glyph.clone()
                };
                let big = theme.font.size_title * 2.0;
                let w = self.font.measure(&glyph, big) as i32;
                self.font.draw(
                    canvas,
                    &glyph,
                    x + (tile_w - w) / 2,
                    iy + icon_size as i32 / 2 + (big / 3.0) as i32,
                    big,
                    if focused { ink } else { theme.color.muted },
                );
            }

            // A tile in hand: a thick frame and a MOVE tag, never colour alone.
            if focused && self.moving {
                for inset in 1..4 {
                    canvas.frame(
                        x - inset,
                        y - inset,
                        (tile_w + 2 * inset) as u32,
                        (tile_h + 2 * inset) as u32,
                        theme.color.foreground,
                    );
                }
                let tag = "MOVE";
                let tag_size = theme.font.size_hint * 0.85;
                let tw = self.mono_bold.measure(tag, tag_size) as i32;
                canvas.rect(x + 4, y + 4, (tw + 8) as u32, 16, ink);
                self.mono_bold
                    .draw(canvas, tag, x + 8, y + 16, tag_size, face);
            }
            // Running and pinned, in the top corners: the same warnings the
            // list gives, out of the icon's way.
            if self.is_running(&row.payload) {
                canvas.rect(
                    x + tile_w - 10,
                    y + 5,
                    5,
                    5,
                    if focused { ink } else { theme.color.accent },
                );
            }
            // On the home every app is pinned: a mark on each says nothing.
            if row.active && !self.home() {
                pt35_ui::pixel::draw_centred(
                    canvas,
                    pt35_ui::pixel::PIN,
                    x + 11,
                    y + 11,
                    2,
                    if focused { ink } else { theme.color.accent },
                );
            }

            let label = self.font.elide(&row.label, size, (tile_w - 8) as u32);
            let w = self.font.measure(&label, size) as i32;
            self.font.draw(
                canvas,
                &label,
                x + (tile_w - w) / 2,
                iy + icon_size as i32 + 20,
                size,
                ink,
            );
        }
    }

    /// The launcher's apps as a list, one row each.
    fn draw_app_rows(
        &mut self,
        canvas: &mut Canvas,
        rows: &[crate::model::Row],
        top: i32,
        bottom: i32,
        split: i32,
    ) {
        let theme = self.theme.clone();
        let pad = theme.menu.padding_x as i32;
        let icon_size = 24;
        let cursor = self.model.screen().list.cursor_index();
        let row_h = theme.menu.row_height as i32;
        if self.side.is_none() && !rows.is_empty() {
            let y = self.motion.glide(top + cursor as i32 * row_h);
            canvas.rect(0, y, split as u32, row_h as u32, theme.color.background_alt);
            canvas.rect(0, y, 3, row_h as u32, theme.color.accent);
        }
        for (index, row) in rows.iter().enumerate() {
            let y = top + index as i32 * row_h;
            if y + row_h > bottom {
                break;
            }
            self.row_hits.push((0, y, split, y + row_h, index));
            let focused = index == cursor && self.side.is_none();
            let centre = y + row_h / 2;
            let name = self.icons.resolve(&[&row.icon], icon_size);
            let drawn = name
                .and_then(|name| self.icons.get(&name, icon_size))
                .map(|icon| icon.draw(canvas, pad, centre - icon_size as i32 / 2))
                .is_some();
            if !drawn {
                let glyph = if row.glyph.is_empty() {
                    row.label.chars().next().unwrap_or('?').to_string()
                } else {
                    row.glyph.clone()
                };
                self.font.draw(
                    canvas,
                    &glyph,
                    pad,
                    centre + (theme.font.size_menu / 3.0) as i32,
                    theme.font.size_menu,
                    theme.color.muted,
                );
            }
            // A dot for an app that already has a window. The daemon focuses
            // rather than starting a second copy, and this is the only warning
            // you get before you press.
            let running = self.is_running(&row.payload);
            // Clear of the scrollbar, which sits against the split.
            let bar = theme.menu.scrollbar_width as i32;
            let mut label_right = if running {
                split - bar - 14
            } else {
                split - bar - 4
            };
            if running {
                canvas.rounded_rect(split - bar - 10, centre - 3, 6, 6, 3, theme.color.accent);
            }
            if row.active {
                let (w, _) = pt35_ui::pixel::size(pt35_ui::pixel::PIN, 2);
                let x = label_right - w as i32;
                pt35_ui::pixel::draw_centred(
                    canvas,
                    pt35_ui::pixel::PIN,
                    x + w as i32 / 2,
                    centre,
                    2,
                    theme.color.accent,
                );
                label_right = x - 6;
            }

            let label_x = pad + icon_size as i32 + 12;
            let label = self.font.elide(
                &row.label,
                theme.font.size_menu,
                (label_right - label_x).max(0) as u32,
            );
            self.font.draw(
                canvas,
                &label,
                label_x,
                centre + (theme.font.size_menu / 3.0) as i32,
                theme.font.size_menu,
                if focused {
                    theme.color.accent
                } else {
                    theme.color.foreground
                },
            );
        }
    }

    /// The launcher: apps down the left, the three screens that are not apps
    /// down the right.
    fn draw_launcher(&mut self, canvas: &mut Canvas, top: i32, bottom: i32) {
        let theme = self.theme.clone();
        // The column runs to the edge: a margin outside it would read as the
        // list continuing, and 640px has none to spare.
        let split = canvas.width as i32 - 148;

        let rows = self.model.visible_rows();
        self.row_hits.clear();
        if self.home() {
            let pad = theme.menu.padding_x as i32;
            let column = pad + WIDGET_WIDTH;
            self.draw_widgets(canvas, pad, top, bottom);
            let right = canvas.width as i32;
            if rows.is_empty() {
                let size = theme.font.size_hint;
                let text = "Nothing pinned yet. Y: All apps.";
                let x = column + theme.menu.gap as i32;
                self.font
                    .draw(canvas, text, x, top + 40, size, theme.color.muted);
            }
            self.draw_app_tiles(
                canvas,
                &rows,
                column + theme.menu.gap as i32,
                top,
                bottom,
                right,
            );
            self.draw_scrollbar(canvas, right, top, bottom);
            self.side_hits.clear();
            return;
        }
        if rows.is_empty() {
            self.draw_nothing_here(canvas, top, split);
        }
        if self.model.screen().list.columns() > 1 {
            let pad = theme.menu.padding_x as i32;
            self.draw_app_tiles(canvas, &rows, pad, top, bottom, split);
        } else {
            self.draw_app_rows(canvas, &rows, top, bottom, split);
        }

        self.draw_scrollbar(canvas, split, top, bottom);

        // The side column is its own surface, not three boxes floating on the
        // list's background.
        let strip = canvas.width as i32 - split;
        canvas.rect(
            split,
            top,
            strip as u32,
            (bottom - top) as u32,
            theme.color.background_alt,
        );
        canvas.rect(split, top, 1, (bottom - top) as u32, theme.color.border);

        let gap = 8;
        let inset = 8;
        let width = strip - inset * 2;
        let height = ((bottom - top) - gap * (SIDE.len() as i32 + 1)) / SIDE.len() as i32;
        self.side_hits.clear();
        for (index, button) in SIDE.iter().enumerate() {
            let x = split + inset;
            let y = top + gap + index as i32 * (height + gap);
            // The whole strip is the target, not just the drawn pill: a thumb on
            // a 3.5" panel is wider than 8px of margin.
            self.side_hits.push((
                split,
                y - gap / 2,
                canvas.width as i32,
                y + height + gap / 2,
                index,
            ));
            let focused = self.side == Some(index);
            let tint = (button.tint)(&theme.color);
            let centre = y + height / 2;

            canvas.rounded_rect(
                x,
                y,
                width as u32,
                height as u32,
                2,
                if focused { tint } else { theme.color.border },
            );
            canvas.rounded_rect(
                x + 1,
                y + 1,
                (width - 2) as u32,
                (height - 2) as u32,
                2,
                theme.color.background,
            );
            // A bar down the edge, so which one is picked reads from the corner
            // of your eye.
            canvas.rect(x + 1, y + 1, 3, (height - 2) as u32, tint);

            let size = 28;
            if let Some(icon) = self.icons.get(button.icon, size) {
                icon.draw(
                    canvas,
                    x + (width - size as i32) / 2,
                    centre - size as i32 / 2 - 8,
                );
            }
            let track = theme.font.tracking;
            let label = button.label.to_uppercase();
            let tw = self
                .mono_bold
                .measure_tracked(&label, theme.font.size_hint, track) as i32;
            self.mono_bold.draw_tracked(
                canvas,
                &label,
                x + (width - tw) / 2,
                centre + 24,
                theme.font.size_hint,
                if focused { tint } else { theme.color.muted },
                track,
            );
        }
    }

    /// The quick panel: a switch, a slider or a readout per row, and the ways
    /// out as ordinary rows: one column, one thing per line.
    /// The window switcher: one card per window, left to right in the same
    /// order as the taskbar, Launcher first where the shutter button is. The
    /// strip scrolls to keep the selected card near the middle.
    fn draw_windows(&mut self, canvas: &mut Canvas, top: i32, bottom: i32) {
        let theme = self.theme.clone();
        let pad = theme.menu.padding_x as i32;
        let gap = theme.menu.gap as i32;
        let rows = self.model.visible_rows();
        let cursor = self.model.screen().list.cursor_index() as i32;
        self.row_hits.clear();
        self.cards_drawn.clear();
        if rows.is_empty() {
            self.draw_nothing_here(canvas, top, canvas.width as i32);
            return;
        }
        let width = canvas.width as i32;
        // Three across, so the cards either side say there is more.
        let card_w = (width - 2 * pad - 2 * gap) / 3;
        let dots = 18;
        let card_h = (bottom - top - dots - gap).min(card_w * 5 / 4);
        let card_y = top + (bottom - top - dots - card_h) / 2;
        let count = rows.len() as i32;
        let centre_of = |index: i32| pad + index * (card_w + gap) + card_w / 2;
        // The selected card is always in the middle, even at the ends of the
        // strip: where you are never depends on how many windows are open.
        //
        // Not while the pointer drives it: centring the card you point at
        // slides another one under the pointer, and that one is selected
        // next. Then the strip holds still until the selected card would
        // leave it.
        let target = match (self.pointer_driven, self.strip_scroll) {
            (true, Some(held)) => {
                let left = pad + cursor * (card_w + gap);
                held.clamp(left + card_w + pad - width, left - pad)
            }
            _ => centre_of(cursor) - width / 2,
        };
        self.strip_scroll = Some(target);
        let scroll = self.motion.glide(target);

        // Tell the bar which window the cursor is on, once per change.
        let hover = self.model.selected_payload().and_then(|p| {
            if p == providers::HOME {
                return Some(0);
            }
            p.strip_prefix("con:")?.parse().ok()
        });
        if hover != self.hover_sent {
            self.hover_sent = hover;
            let _ = crate::live::request(&pt35_common::ipc::Request::SwitcherHover { id: hover });
        }

        let icon_size = 48;
        let name_size = theme.font.size_menu * 0.85;
        let note_size = theme.font.size_hint * 0.85;
        let radius = theme.menu.radius;
        for (index, row) in rows.iter().enumerate() {
            let x = pad + index as i32 * (card_w + gap) - scroll;
            if x + card_w < 0 || x > width {
                continue;
            }
            self.row_hits
                .push((x, card_y, x + card_w, card_y + card_h, index));
            let focused = index as i32 == cursor;
            let edge = if focused {
                theme.color.accent
            } else {
                theme.color.border
            };
            let thick = if focused { 2 } else { 1 };
            canvas.rounded_rect(x, card_y, card_w as u32, card_h as u32, radius, edge);
            canvas.rounded_rect(
                x + thick,
                card_y + thick,
                (card_w - 2 * thick) as u32,
                (card_h - 2 * thick) as u32,
                radius,
                theme.color.background_alt,
            );

            let middle = x + card_w / 2;
            let icon_y = card_y + card_h / 3;
            let id = row
                .payload
                .strip_prefix("con:")
                .and_then(|id| id.parse::<i64>().ok());
            if let Some(id) = id.filter(|_| theme.menu.window_previews) {
                self.cards_drawn.push(id);
                if self.preview_wanted(id) {
                    match read_preview(id) {
                        Some(preview) => {
                            self.previews.insert(id, preview);
                        }
                        None => {
                            if let Some(modified) = preview_modified(id) {
                                self.bad_previews.insert(id, modified);
                            }
                        }
                    }
                }
            }
            let preview = id
                .filter(|_| theme.menu.window_previews)
                .and_then(|id| self.previews.get(&id));
            if let Some(preview) = preview {
                // The picture fills the top of the card at the panel's shape,
                // with the app's icon in its corner so it still reads at a
                // glance.
                let pw = card_w - 16;
                let ph =
                    (pw * preview.height as i32 / preview.width.max(1) as i32).min(card_h * 3 / 5);
                let (px, py) = (x + 8, card_y + 8);
                canvas.rect(
                    px - 1,
                    py - 1,
                    (pw + 2) as u32,
                    (ph + 2) as u32,
                    theme.color.border,
                );
                canvas.blit_rgb(
                    px,
                    py,
                    pw as u32,
                    ph as u32,
                    &preview.rgb,
                    preview.width,
                    preview.height,
                );
                let small = theme.icons.size_bar.max(20);
                let name = self.icons.resolve(&[&row.icon], small);
                if let Some(icon) = name.and_then(|name| self.icons.get(&name, small)) {
                    let (ix, iy) = (px + 4, py + ph - small as i32 - 4);
                    canvas.rect(
                        ix - 2,
                        iy - 2,
                        small + 4,
                        small + 4,
                        theme.color.background_alt,
                    );
                    icon.draw(canvas, ix, iy);
                }
            } else if row.icon == providers::SKULL_ICON {
                pt35_ui::pixel::draw_centred(
                    canvas,
                    pt35_ui::pixel::SKULL,
                    middle,
                    icon_y,
                    3,
                    theme.color.accent,
                );
            } else {
                let name = self.icons.resolve(&[&row.icon], icon_size);
                if let Some(icon) = name.and_then(|name| self.icons.get(&name, icon_size)) {
                    icon.draw(
                        canvas,
                        middle - icon_size as i32 / 2,
                        icon_y - icon_size as i32 / 2,
                    );
                }
            }

            let room = (card_w - 16).max(0) as u32;
            let label = self.font.elide(&row.label, name_size, room);
            let label_w = self.font.measure(&label, name_size) as i32;
            let name_y = card_y + card_h * 2 / 3;
            let ink = if focused {
                theme.color.accent
            } else {
                theme.color.foreground
            };
            self.font
                .draw(canvas, &label, middle - label_w / 2, name_y, name_size, ink);
            if !row.note.is_empty() {
                let note = self.mono.elide(&row.note, note_size, room);
                let note_w = self.mono.measure(&note, note_size) as i32;
                self.mono.draw(
                    canvas,
                    &note,
                    middle - note_w / 2,
                    name_y + (note_size * 1.5) as i32,
                    note_size,
                    theme.color.muted,
                );
            }
        }

        // One dot per card: where you are when the strip runs off the edges.
        let dot = 6;
        let spacing = 12;
        let dots_w = count * spacing - (spacing - dot);
        let dots_x = (width - dots_w) / 2;
        let dots_y = card_y + card_h + gap;
        for index in 0..count {
            let colour = if index == cursor {
                theme.color.accent
            } else {
                theme.color.border
            };
            canvas.rect(
                dots_x + index * spacing,
                dots_y,
                dot as u32,
                dot as u32,
                colour,
            );
        }
    }

    fn draw_quick(&mut self, canvas: &mut Canvas, top: i32, bottom: i32) {
        let theme = self.theme.clone();
        let pad = theme.menu.padding_x as i32;
        let rows = self.model.visible_rows();
        let cursor = self.model.screen().list.cursor_index();
        self.row_hits.clear();
        if rows.is_empty() {
            self.draw_nothing_here(canvas, top, canvas.width as i32);
            return;
        }

        // Framed, the panel is a list like any other: its rows, its pill.
        let framed = theme.menu.row_frames;
        let row_h = if framed {
            theme.menu.row_height as i32
        } else {
            ((bottom - top - 8) / rows.len().max(1) as i32).min(56)
        };
        let icon_size = 24;
        let readonly = self.readonly();
        if !readonly {
            let y = self.motion.glide(top + cursor as i32 * row_h);
            if framed {
                let (w, h) = (self.pill_width(canvas.width), (row_h - 6) as u32);
                canvas.rounded_rect(pad / 2, y + 3, w, h, theme.menu.radius, theme.color.accent);
            } else {
                canvas.rect(0, y, canvas.width, row_h as u32, theme.color.background_alt);
                canvas.rect(0, y, 3, row_h as u32, theme.color.accent);
            }
        }

        for (index, row) in rows.iter().enumerate() {
            let y = top + index as i32 * row_h;
            let focused = index == cursor && !readonly;
            self.row_hits
                .push((0, y, canvas.width as i32, y + row_h, index));
            let centre = y + row_h / 2;
            // On the framed pill, everything takes the pill's ink.
            let inked = framed && focused;
            let (fg, dim, lit) = if inked {
                let ink = theme.color.accent_fg;
                (ink, ink, ink)
            } else {
                (
                    theme.color.foreground,
                    theme.color.muted,
                    theme.color.accent,
                )
            };
            if framed {
                let colour = if inked { fg } else { theme.color.border };
                canvas.frame(pad - 4, centre - 16, 32, 32, colour);
                if !focused && index + 1 < rows.len() {
                    let w = canvas.width - 2 * pad as u32;
                    canvas.rect(pad, y + row_h - 1, w, 1, theme.color.border);
                }
            }
            if let Some(icon) = self.icons.get_symbolic(&row.icon, icon_size) {
                let tint = if focused { lit } else { theme.color.muted };
                icon.draw_tinted(canvas, pad, centre - icon_size as i32 / 2, tint);
            }
            let label_x = pad + icon_size as i32 + if framed { 16 } else { 12 };
            let label_end = self.font.draw(
                canvas,
                &row.label,
                label_x,
                centre + (theme.font.size_menu / 3.0) as i32,
                theme.font.size_menu,
                fg,
            );
            // A gauge takes the right two thirds, and never the label.
            let gauge_x = (canvas.width as i32 / 3).max(label_end + 16);

            let right = canvas.width as i32 - pad;
            match row.glyph.split(':').next().unwrap_or("") {
                "switch" if framed => {
                    let on = row.glyph.ends_with("on");
                    let x = self.draw_toggle(canvas, right, centre, on);
                    self.note(canvas, &row.note, x - 10, centre, dim);
                }
                "switch" => {
                    let on = row.glyph.ends_with("on");
                    let w = 62;
                    let h = 26;
                    let x = right - w;
                    let colour = if on {
                        theme.color.accent
                    } else {
                        theme.color.border
                    };
                    canvas.rounded_rect(x, centre - h / 2, w as u32, h as u32, 2, colour);
                    let text = if on { "ON" } else { "OFF" };
                    let tw = self.mono_bold.measure(text, theme.font.size_hint) as i32;
                    self.mono_bold.draw(
                        canvas,
                        text,
                        x + (w - tw) / 2,
                        centre + 5,
                        theme.font.size_hint,
                        if on {
                            theme.color.accent_fg
                        } else {
                            theme.color.muted
                        },
                    );
                    self.note(canvas, &row.note, x - 10, centre, dim);
                }
                "bar" => {
                    let value: i32 = row
                        .glyph
                        .split(':')
                        .nth(1)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let note_w = self.mono.measure(&row.note, theme.font.size_hint) as i32;
                    let bar_x = gauge_x;
                    let bar_w = right - note_w - 12 - bar_x;
                    canvas.rect(bar_x, centre - 5, bar_w as u32, 10, theme.color.border);
                    let filled = bar_w * value.clamp(0, 100) / 100;
                    let colour = match value {
                        85..=100 => theme.color.critical,
                        60..=84 => theme.color.warning,
                        _ => lit,
                    };
                    canvas.rect(bar_x, centre - 5, filled as u32, 10, colour);
                    self.mono.draw(
                        canvas,
                        &row.note,
                        right - note_w,
                        centre + 5,
                        theme.font.size_hint,
                        fg,
                    );
                }
                "slide" => {
                    let value: i32 = row
                        .glyph
                        .split(':')
                        .nth(1)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let note_w = self.mono.measure(&row.note, theme.font.size_hint) as i32;
                    let track_x = gauge_x;
                    let track_w = right - note_w - 12 - track_x;
                    canvas.rect(track_x, centre - 2, track_w as u32, 4, theme.color.border);
                    let filled = track_w * value.clamp(0, 100) / 100;
                    canvas.rect(track_x, centre - 2, filled as u32, 4, lit);
                    canvas.rounded_rect(track_x + filled - 4, centre - 9, 8, 18, 2, lit);
                    self.mono.draw(
                        canvas,
                        &row.note,
                        right - note_w,
                        centre + 5,
                        theme.font.size_hint,
                        fg,
                    );
                }
                // A row that leads somewhere says so, the same way a submenu
                // row in a plain list does.
                "nav" => {
                    let size = theme.font.size_menu;
                    let colour = if focused { lit } else { dim };
                    let w = self.font.measure(">", size) as i32;
                    self.font.draw(
                        canvas,
                        ">",
                        right - w,
                        centre + (size / 3.0) as i32,
                        size,
                        colour,
                    );
                }
                _ => self.note(canvas, &row.note, right, centre, dim),
            }
            if readonly && !framed && index + 1 < rows.len() {
                canvas.rect(
                    pad,
                    y + row_h - 1,
                    canvas.width - 2 * pad as u32,
                    1,
                    theme.color.border,
                );
            }
        }
        self.draw_scrollbar(canvas, canvas.width as i32, top, bottom);
    }

    /// Right-aligned readout next to a switch.
    fn note(&mut self, canvas: &mut Canvas, note: &str, right: i32, centre: i32, colour: Rgb) {
        if note.is_empty() {
            return;
        }
        let size = self.theme.font.size_hint;
        let width = self.mono.measure(note, size) as i32;
        self.mono
            .draw(canvas, note, right - width, centre + 5, size, colour);
    }

    /// A toggle switch ending at `right`: a knob at the off or the on end, and
    /// the state written in the free half as 0 or 1, so it never rests on
    /// colour alone. Returns its left edge.
    fn draw_toggle(&mut self, canvas: &mut Canvas, right: i32, centre: i32, on: bool) -> i32 {
        let (w, h) = (64, 26);
        let (x, y) = (right - w, centre - h / 2);
        let theme = &self.theme;
        // The word carries the state; the colour and the knob's side back it up.
        let (fill, text) = if on {
            (theme.color.ok, "ON")
        } else {
            (theme.color.critical, "OFF")
        };
        let ink = fill.ink();
        canvas.rect(x, y, w as u32, h as u32, fill);
        let knob_w = 20;
        let knob_x = if on { x + w - knob_w - 3 } else { x + 3 };
        canvas.rect(
            knob_x,
            y + 3,
            knob_w as u32,
            (h - 6) as u32,
            theme.color.background,
        );
        let free = if on { x } else { x + knob_w + 3 };
        let (face, size) = match self.display.as_mut() {
            Some(display) => (display, 24.0),
            None => (&mut self.mono_bold, 16.0),
        };
        let tw = face.measure(text, size) as i32;
        let half = w - knob_w - 3;
        face.draw(canvas, text, free + (half - tw) / 2, centre + 7, size, ink);
        x
    }

    fn draw_hints(&mut self, canvas: &mut Canvas, top: i32) {
        let theme = &self.theme;
        let height = theme.menu.hint_height;
        let framed = theme.menu.hint_frames;
        // Framed hints are surfaces of their own, so the strip goes dark.
        let strip = if framed {
            theme.color.background
        } else {
            theme.color.background_alt
        };
        canvas.rect(0, top, canvas.width, height, strip);
        canvas.rect(0, top, canvas.width, 1, theme.color.border);

        let filtering = self.model.screen().list.mode() == Mode::Filter;
        let on_switcher = matches!(
            self.model.screen().source,
            crate::model::Source::Dynamic {
                builtin: pt35_common::menu::Builtin::Windows,
                ..
            }
        );
        let on_quick_setting = self.model.focused_adjust().is_some();
        let readonly = self.readonly();
        let empty = self.model.screen().list.is_empty();
        let mouse = self.mouse_mode();
        let hints: &[Hint] = match (filtering, on_switcher, on_quick_setting) {
            (true, _, _) => FILTER_HINTS,
            // In Mouse mode the face buttons are the mouse, here as anywhere.
            _ if mouse => MOUSE_HINTS,
            (false, true, _) => WINDOW_HINTS,
            (false, false, true) => QUICK_HINTS,
            (false, false, false) => NAV_HINTS,
        };
        let rooted = self.model.depth() == 1;
        let back_closes = self.model.back_closes();
        // The launcher is never closed from inside it, desktop or not.
        let pinned = self.is_desktop() || (rooted && self.model.launcher_is_root());
        let on_home_tile = self.model.selected_payload() == Some(providers::HOME);
        let hints: Vec<Hint> = hints
            .iter()
            // At the top screen there is nowhere to go home to, and on the
            // desktop there is nothing to close the menu onto. A legend that
            // names a key which does nothing is worse than a shorter legend.
            .filter(|hint| !(rooted && hint.button == "Y"))
            // B as Back, that is. In Mouse mode it is the right button.
            .filter(|hint| !(pinned && hint.button == "B" && !mouse))
            .filter(|hint| !(on_switcher && on_home_tile && hint.action == "Close"))
            .map(|hint| {
                if on_switcher && on_home_tile && hint.button == "A" {
                    Hint {
                        button: "A",
                        action: "Open",
                    }
                } else {
                    *hint
                }
            })
            .filter(|hint| !self.model.is_confirm() || hint.button != "X")
            // Nothing to open on a dashboard or an empty list, nothing to find
            // on a dashboard.
            .filter(|hint| !((readonly || empty) && !filtering && hint.button == "A"))
            .filter(|hint| !(readonly && hint.button == "X"))
            .map(|hint| {
                // At the top screen back means out. On the screen the menu was
                // opened on it is still "Back": back to your app.
                if rooted && !back_closes && hint.button == "B" && !mouse {
                    Hint {
                        button: "B",
                        action: "Close",
                    }
                } else {
                    hint
                }
            })
            .collect();
        let mut hints = hints;
        // Y on a launcher row pins it: the one thing Y is free for at the top.
        // In Mouse mode Y is the wheel: a right click pins, from its menu.
        // Search is All apps' own: on the home X is every widget, and the
        // shoulders flip the pile.
        if self.home() && !mouse {
            hints.retain(|hint| hint.button != "Y" && hint.button != "X");
            hints.push(Hint {
                button: "X",
                action: "Widgets",
            });
            hints.push(Hint {
                button: "Y",
                action: "Apps",
            });
            if self.widgets.len() > 1 {
                hints.push(Hint {
                    button: "L R",
                    action: "Flip",
                });
            }
            if self.model.selected_payload().is_some() {
                hints.insert(
                    1.min(hints.len()),
                    Hint {
                        button: "B",
                        action: "Move",
                    },
                );
            }
        }
        // A tile in hand: the legend says so in words, not only the frame.
        if self.moving {
            hints = vec![
                Hint {
                    button: "<>",
                    action: "Move",
                },
                Hint {
                    button: "A B",
                    action: "Drop",
                },
            ];
        }
        if !mouse && self.pin_target().is_some() {
            // Not also Home: Y does one thing per screen.
            hints.retain(|hint| hint.button != "Y");
            let pinned = self
                .model
                .visible_rows()
                .get(self.model.screen().list.cursor_index())
                .is_some_and(|row| row.active);
            hints.push(Hint {
                button: "Y",
                action: if pinned { "Unpin" } else { "Pin" },
            });
        }
        let size = theme.font.size_hint;
        let baseline = top + (height as f32 * 0.62) as i32;
        let centre = top + height as i32 / 2;

        // Each chip takes the width it needs and the slack is shared out
        // between them: even slots cut the last word off when the legend is
        // long, and "Clos..." is worse than a tighter gap.
        // A framed hint: a square key, then its action inside the frame.
        let key_h = 26;
        let key_w = |w: i32| {
            if framed {
                (w + 12).max(key_h)
            } else {
                (w + 14).max(24)
            }
        };
        let after_key = if framed { 8 } else { 6 };
        let frame_pad = if framed { 10 } else { 0 };
        let widths: Vec<i32> = hints
            .iter()
            .map(|hint| {
                let key = key_w(self.bold.measure(hint.button, size) as i32);
                key + after_key + self.font.measure(hint.action, size) as i32 + frame_pad
            })
            .collect();
        let total: i32 = widths.iter().sum();
        let count = hints.len().max(1) as i32;
        let gap = ((canvas.width as i32 - total) / (count + 1)).max(4);
        let tight = total + gap * (count + 1) > canvas.width as i32;

        self.hint_hits.clear();
        let mut x = gap;
        for (index, hint) in hints.iter().enumerate() {
            let start = x;
            let (pill, ink) = self.button_colors(hint.button);
            let label_w = self.bold.measure(hint.button, size) as i32;
            let pill_w = key_w(label_w);
            // Only when it cannot fit: elide the words, never the pill.
            let room = if tight {
                (canvas.width as i32 - x - pill_w - after_key - frame_pad - 4).max(0) as u32
            } else {
                widths[index] as u32
            };
            let action = self.font.elide(hint.action, size, room);
            let action_w = self.font.measure(&action, size) as i32;
            let label_colour = if framed {
                let (y, w) = (
                    centre - key_h / 2,
                    pill_w + after_key + action_w + frame_pad,
                );
                let theme = &self.theme;
                canvas.rect(x, y, w as u32, key_h as u32, theme.color.background_alt);
                canvas.frame(x, y, w as u32, key_h as u32, theme.color.border);
                let radius = theme.menu.radius;
                canvas.rounded_rect(x, y, pill_w as u32, key_h as u32, radius, pill);
                theme.color.foreground
            } else {
                canvas.rounded_rect(x, centre - 12, pill_w as u32, 24, 12, pill);
                self.theme.color.muted
            };
            self.bold.draw(
                canvas,
                hint.button,
                x + (pill_w - label_w) / 2,
                baseline,
                size,
                ink,
            );
            x += pill_w + after_key;
            self.font
                .draw(canvas, &action, x, baseline, size, label_colour);
            x += action_w + frame_pad;
            self.hint_hits.push((start, top, x + gap / 2, hint.button));
            x += gap;
        }
    }
}

impl App for Menu {
    fn scanlines(&self) -> (u8, u32) {
        self.theme.effects.scanline()
    }

    fn sweep(&self) -> Option<layer::Sweep> {
        // The menu sits under the bar.
        layer::Sweep::from_theme(&self.theme, self.theme.bar.height as i32)
    }

    fn background(&self) -> Rgb {
        self.theme.color.background
    }

    fn animating(&self) -> bool {
        self.motion.busy()
    }

    fn tick_interval(&self) -> Option<Duration> {
        // Fast enough that a finished Wi-Fi scan appears the moment it lands.
        // The daemon is only asked every fourth tick: a status read costs it a
        // sway tree walk, and four a second of that is not free on a Pi.
        Some(TICK)
    }

    fn tick(&mut self) -> bool {
        let mut drawn = false;
        // A slow provider finishing is worth a frame on its own.
        if let Some((builtin, rx)) = &self.loading {
            match rx.try_recv() {
                Ok(items) => {
                    if self.model.dynamic_builtin() == Some(*builtin) {
                        self.model.replace_dynamic(items);
                    }
                    self.loading = None;
                    drawn = true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.loading = None;
                    self.error = Some("that screen could not be read".into());
                    drawn = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        // Re-read the daemon, so a window closing behind the menu or a toggle
        // flipping shows without reopening it.
        // A capture pt35d took as the menu opened lands a moment after the
        // first frame. Drop pictures whose file has changed, so the next draw
        // reads the new one.
        if self.model.dynamic_builtin() == Some(pt35_common::menu::Builtin::Windows) {
            let stale: Vec<i64> = self
                .previews
                .iter()
                .filter(|(id, p)| {
                    std::fs::metadata(preview_path(**id))
                        .and_then(|m| m.modified())
                        .map_or(true, |m| m != p.modified)
                })
                .map(|(id, _)| *id)
                .collect();
            if !stale.is_empty() {
                for id in stale {
                    self.previews.remove(&id);
                }
                drawn = true;
            }
            // And a first capture, for a card on screen that had none. Cards
            // out of sight do not count: they would keep this redrawing for as
            // long as the switcher is open.
            let missing = self
                .cards_drawn
                .clone()
                .into_iter()
                .any(|id| self.preview_wanted(id));
            if missing && self.theme.menu.window_previews {
                drawn = true;
            }
        }
        // One rain step a tick.
        drawn |= self.raining();
        self.since_poll += 1;
        // The widget column fills in on the first tick, not before the first
        // frame, then moves with the poll.
        let home_due = !self.readings.sampled() || self.since_poll >= POLL_TICKS;
        if self.home() && home_due {
            self.readings.sample();
            let playing = self.readings.media.as_ref().is_some_and(|m| m.playing);
            if playing && !self.was_playing {
                if let Some(at) = self.widgets.iter().position(|w| *w == Widget::Media) {
                    self.widget = at;
                }
            }
            self.was_playing = playing;
            drawn = true;
        }
        if self.since_poll < POLL_TICKS {
            return drawn;
        }
        self.since_poll = 0;
        // Devices turn up one by one during a scan: re-read the list each poll
        // until it ends, then once more for the last of them.
        if let Some(until) = self.scan_until {
            let on_bluetooth =
                self.model.dynamic_builtin() == Some(pt35_common::menu::Builtin::Bluetooth);
            if !on_bluetooth || std::time::Instant::now() > until {
                self.scan_until = None;
            }
            if on_bluetooth && self.loading.is_none() {
                self.resync();
            }
        }
        let status = crate::live::status();
        if status == self.status {
            return drawn;
        }
        self.status = status;
        self.windows = self.status.as_ref().map_or(0, |s| s.windows.len());
        self.desktop = self.status.as_ref().is_some_and(|s| s.workspace_empty());
        // pt35d finds the firmware a moment after login, and the desktop menu
        // may have been up since before that.
        if let Some(status) = &self.status {
            pt35_ui::keys::set_letters_are_buttons(!status.pt35_firmware);
        }
        // A dynamic screen's rows were built by its provider and are frozen, so
        // a fresh Status is not enough on its own.
        if let Some(builtin) = self.model.dynamic_builtin() {
            if matches!(builtin, pt35_common::menu::Builtin::Windows) {
                self.model.replace_dynamic(providers::items(builtin));
            }
        }
        true
    }

    fn key(&mut self, key: Key) -> bool {
        use pt35_ui::keys::{navigate, Navigation};
        self.pointer_driven = false;
        if let Some(popup) = self.popup.as_mut() {
            let count = popup.items.len();
            match navigate(&key, Mode::Nav) {
                Navigation::Up => popup.hot = (popup.hot + count - 1) % count,
                Navigation::Down => popup.hot = (popup.hot + 1) % count,
                Navigation::Activate => {
                    let button = popup.items[popup.hot].1;
                    self.popup = None;
                    return self.press(button);
                }
                Navigation::Back | Navigation::Cancel => self.popup = None,
                // Anything else closes it and then does what it does.
                _ => {
                    self.popup = None;
                    return self.key(key);
                }
            }
            return true;
        }
        if self.moving {
            if let Some(step) = self.move_tile(&key) {
                return step;
            }
        }
        // Typing on the launcher is a search: the first letter opens it.
        if self.on_launcher()
            && self.side.is_none()
            && self.theme.menu.type_to_search
            && !pt35_ui::keys::letters_are_buttons()
            && self.model.screen().list.mode() == Mode::Nav
            && key.text.is_some_and(|c| c.is_alphanumeric())
            && pt35_ui::keys::button(&key, Mode::Filter).is_none()
        {
            if self.home() {
                self.open_here(pt35_common::menu::Builtin::Apps);
            }
            self.model.handle(&Key::new(pt35_ui::keys::sym::BUTTON_X));
            let step = self.model.handle(&key);
            return self.apply(step);
        }
        // The shoulders keep their meaning with the menu up: sway hands them
        // over while it is open, so the menu does what the binding would.
        // Except on the home, where they flip the widget pile: the bar's mode
        // icon is the mode switch there.
        let mode = self.model.screen().list.mode();
        if self.home() && mode == Mode::Nav {
            match pt35_ui::keys::button(&key, mode) {
                Some(pt35_ui::keys::Button::R) => return self.flip_widget(true),
                Some(pt35_ui::keys::Button::L) => return self.flip_widget(false),
                // B has nothing to go back to on the home: it picks the tile up.
                Some(pt35_ui::keys::Button::B) if self.side.is_none() => {
                    self.moving = self.model.selected_payload().is_some();
                    return true;
                }
                _ => {}
            }
        }
        match pt35_ui::keys::button(&key, mode) {
            // Select is the switcher's key: it opens it, and on it, closes it.
            // A search keeps Select for clearing itself.
            Some(pt35_ui::keys::Button::Select) if mode == Mode::Nav => {
                let on_switcher =
                    self.model.dynamic_builtin() == Some(pt35_common::menu::Builtin::Windows);
                if !on_switcher {
                    self.open_here(pt35_common::menu::Builtin::Windows);
                    return true;
                }
                let step = self.model.handle(&Key::new(pt35_ui::keys::sym::BACKSPACE));
                return self.apply(step);
            }
            Some(pt35_ui::keys::Button::R) => {
                if let Err(message) = self.ctl(&["mode", "toggle"]) {
                    self.error = Some(message);
                }
                // Rebuilding the rows would drop a search in progress, and
                // only the quick panel shows the mode anyway.
                if mode == Mode::Nav {
                    self.resync();
                } else {
                    self.status = crate::live::status();
                }
                return true;
            }
            // On the switcher, the card under the cursor. Anywhere else the
            // window is hidden behind the menu: closing it unseen is a trap.
            Some(pt35_ui::keys::Button::L) => {
                return match self.model.selected_payload() {
                    Some(payload)
                        if self.model.dynamic_builtin()
                            == Some(pt35_common::menu::Builtin::Windows)
                            && payload.starts_with("con:") =>
                    {
                        let payload = payload.to_string();
                        self.apply(Step::Close(payload))
                    }
                    _ => true,
                };
            }
            _ => {}
        }
        if self.home() {
            let mode = self.model.screen().list.mode();
            match navigate(&key, mode) {
                // Y is the way to every app, where pins are made.
                Navigation::Secondary => {
                    self.open_here(pt35_common::menu::Builtin::Apps);
                    return true;
                }
                // X is the way to every widget, where they are pinned. Typing
                // still searches every app.
                Navigation::StartFilter => {
                    self.open_here(pt35_common::menu::Builtin::Widgets);
                    return true;
                }
                // No side column: the grid's edge is the screen's.
                Navigation::Right if !self.launcher_moves_right() => return true,
                _ => {}
            }
        }
        if self.on_launcher() {
            let mode = self.model.screen().list.mode();
            match (self.side, navigate(&key, mode)) {
                // Right leaves the app list for the side buttons, left comes
                // back. In a grid, only from the last tile of a line.
                (None, Navigation::Right) if !self.launcher_moves_right() => {
                    self.side = Some(0);
                    return true;
                }
                (Some(_), Navigation::Left) | (Some(_), Navigation::Back) => {
                    self.side = None;
                    return true;
                }
                (Some(index), Navigation::Down) => {
                    self.side = Some((index + 1) % SIDE.len());
                    return true;
                }
                (Some(index), Navigation::Up) => {
                    self.side = Some((index + SIDE.len() - 1) % SIDE.len());
                    return true;
                }
                (Some(index), Navigation::Activate) => return self.open_side(index),
                // A search is of the apps: the cursor goes back to them.
                (Some(_), Navigation::StartFilter) => self.side = None,
                _ => {}
            }
        }
        let step = self.model.handle(&key);
        self.apply(step)
    }

    fn hover(&mut self, x: f64, y: f64) -> bool {
        // Pointing at a row selects it, the way a mouse does anywhere else, in
        // either mode: an external mouse is a mouse in Buttons mode too. Only
        // motion comes here, so a menu opening under a resting cursor keeps
        // its selection.
        let (x, y) = (x as i32, y as i32);
        if let Some(popup) = self.popup.as_mut() {
            let hot = popup
                .rects
                .iter()
                .position(|&(l, t, r, b)| x >= l && x < r && y >= t && y < b);
            return match hot {
                Some(hot) if hot != popup.hot => {
                    popup.hot = hot;
                    true
                }
                _ => false,
            };
        }
        // The picked-up tile follows the D-pad, not the pointer.
        if self.moving {
            return false;
        }
        self.pointer_driven = true;
        let hit = self
            .row_hits
            .iter()
            .find(|(l, t, r, b, _)| x >= *l && x < *r && y >= *t && y < *b)
            .map(|hit| hit.4);
        match hit {
            Some(index) if index != self.model.screen().list.cursor_index() => {
                self.side = None;
                self.model.screen_mut().list.focus_window(index)
            }
            _ => false,
        }
    }

    /// A drag on the scrollbar moves the list. Anywhere else it is a pointer
    /// moving with a button held, which selects like any other.
    fn drag(&mut self, x: f64, y: f64) -> bool {
        // Once it has the scrollbar the drag keeps it, wherever it wanders.
        if self.scrolling || self.on_scrollbar(x as i32, y as i32) {
            self.scrolling = true;
            return self.scroll_to(y as i32);
        }
        self.hover(x, y)
    }

    fn touch_end(&mut self) {
        self.scrolling = false;
    }

    /// The finger on the scrollbar takes it at once, as a click does. Anywhere
    /// else it only lights the row under it: the tap waits for the lift.
    fn touch_down(&mut self, x: f64, y: f64) -> bool {
        self.pan = 0.0;
        if self.popup.is_none() && !self.moving && self.on_scrollbar(x as i32, y as i32) {
            self.scrolling = true;
            return self.scroll_to(y as i32);
        }
        self.hover(x, y)
    }

    /// A list follows the finger a line at a time. The window switcher's
    /// strip follows it a card at a time, so a swipe left brings the next one.
    fn pan(&mut self, _x: f64, y: f64, dx: f64, dy: f64) -> bool {
        if self.scrolling {
            return self.scroll_to(y as i32);
        }
        if self.popup.is_some() || self.moving {
            return false;
        }
        let picker = self.model.dynamic_builtin() == Some(pt35_common::menu::Builtin::Windows);
        let Some(first) = self.row_hits.first().copied() else {
            return false;
        };
        // One step is the distance from one row (or card) to the next, as drawn.
        let step = if picker {
            self.row_hits.get(1).map(|hit| hit.0 - first.0)
        } else {
            self.row_hits
                .iter()
                .map(|hit| hit.1 - first.1)
                .find(|d| *d > 0)
        };
        let Some(step) = step.filter(|s| *s > 0).map(f64::from) else {
            return false;
        };
        self.pan += if picker { dx } else { dy };
        let lines = (self.pan / step).trunc();
        if lines == 0.0 {
            return false;
        }
        self.pan -= lines * step;
        if picker {
            let sym = if lines < 0.0 {
                pt35_ui::keys::sym::RIGHT
            } else {
                pt35_ui::keys::sym::LEFT
            };
            for _ in 0..lines.abs() as usize {
                self.key(Key::new(sym));
            }
            return true;
        }
        self.side = None;
        // The content moves with the finger: up shows what is further down.
        self.model.screen_mut().list.scroll_by(-lines as isize)
    }

    /// A right click opens the row's menu, the way it does on any desktop.
    /// Back is the breadcrumbs, B, or Backspace.
    fn back_click(&mut self, x: f64, y: f64) -> bool {
        let (x, y) = (x as i32, y as i32);
        self.popup = None;
        self.moving = false;
        let Some(index) = self
            .row_hits
            .iter()
            .find(|(l, t, r, b, _)| x >= *l && x < *r && y >= *t && y < *b)
            .map(|hit| hit.4)
        else {
            return true;
        };
        self.side = None;
        self.model.screen_mut().list.focus_window(index);
        let items = self.row_options();
        if !items.is_empty() {
            self.popup = Some(Popup {
                x,
                y,
                items,
                hot: 0,
                rects: Vec::new(),
            });
        }
        true
    }

    fn scroll(&mut self, down: bool) -> bool {
        self.popup = None;
        // The D-pad wraps from the last row to the first. A wheel stops at the
        // end, as a wheel does anywhere else.
        let list = &self.model.screen().list;
        let at_end = if down {
            list.selected_position() + 1 >= list.len()
        } else {
            list.selected_position() == 0
        };
        // `true`: to the layer, false from here means quit.
        if at_end && self.side.is_none() {
            return true;
        }
        let sym = if down {
            pt35_ui::keys::sym::DOWN
        } else {
            pt35_ui::keys::sym::UP
        };
        self.key(Key::new(sym))
    }

    fn touch(&mut self, x: f64, y: f64) -> bool {
        self.moving = false;
        // The release that ends a scrollbar drag. Whatever is under it now
        // was not what the drag was for.
        if std::mem::take(&mut self.scrolling) {
            return true;
        }
        let (x, y) = (x as i32, y as i32);
        // A click with the right-click menu up is for the menu: on a row it
        // does that row, anywhere else it only closes the menu.
        if let Some(popup) = self.popup.take() {
            let hit = popup
                .rects
                .iter()
                .position(|&(l, t, r, b)| x >= l && x < r && y >= t && y < b);
            return match hit {
                Some(i) => self.press(popup.items[i].1),
                None => true,
            };
        }
        for (left, right, depth) in self.crumb_hits.clone() {
            // The gap under the header counts: a 32px header is then a 40px target.
            if y < self.theme.menu.header_height as i32 + 8 && x >= left && x < right {
                let step = self.model.back_to(depth);
                return self.apply(step);
            }
        }
        if self.on_scrollbar(x, y) {
            self.scroll_to(y);
            return true;
        }
        for (left, top, right, button) in self.hint_hits.clone() {
            if y >= top && x >= left && x < right {
                return self.press(button);
            }
        }
        for (left, top, right, bottom, index) in self.side_hits.clone() {
            if x >= left && x < right && y >= top && y < bottom {
                return self.open_side(index);
            }
        }
        if self.home() {
            let hit = self
                .widget_hits
                .iter()
                .find(|(l, t, r, b, _)| x >= *l && x < *r && y >= *t && y < *b)
                .map(|hit| hit.4.clone());
            match hit {
                Some(WidgetHit::Next) => return self.flip_widget(true),
                Some(WidgetHit::Run(command)) => return self.apply(Step::Run(command)),
                Some(WidgetHit::Player(command)) => {
                    self.readings.control(command);
                    return true;
                }
                None => {}
            }
        }
        for (left, top, right, bottom, index) in self.row_hits.clone() {
            if x < left || x >= right || y < top || y >= bottom {
                continue;
            }
            // A slider is dragged with the D-pad, so a tap on one has to mean
            // something too: the left half turns it down and the right half up.
            let item = self.model.screen().list.item_in_window(index);
            if let Some(adjust) = item.and_then(|item| self.model.adjust_at(item)) {
                let up = x > (left + right) / 2;
                return self.apply(Step::Adjust(adjust, up));
            }
            let step = self.model.activate_window(index);
            return self.apply(step);
        }
        true
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        self.scrollbar = None;
        if self.on_launcher() {
            if let Some(wallpaper) = &self.wallpaper {
                wallpaper.draw(canvas);
            }
        }
        if self.raining() {
            pt35_ui::rain::draw(
                canvas,
                &mut self.mono,
                rain_step(),
                self.theme.effects.rain,
                self.theme.color.foreground,
                self.theme.color.background,
            );
        }
        let hint_top = canvas.height as i32 - self.theme.menu.hint_height as i32;
        // Where the screen's own content stops: above the hints, and above an
        // error when there is one. The hints stay where they are, or they
        // would be drawn over the error.
        let mut body_bottom = hint_top;
        if let Some(message) = self.error.clone() {
            let size = self.theme.font.size_hint;
            let height = 26;
            let y = hint_top - height;
            canvas.rect(0, y, canvas.width, height as u32, self.theme.color.critical);
            let text =
                self.font
                    .elide(&message, size, canvas.width - 2 * self.theme.menu.padding_x);
            self.font.draw(
                canvas,
                &text,
                self.theme.menu.padding_x as i32,
                y + 18,
                size,
                self.theme.color.background,
            );
            body_bottom = y;
        }
        let screen = (self.model.depth(), self.model.screen().title.clone());
        self.motion.screen(screen);
        // The home is the desktop, not a screen with a name: no header.
        let header = if self.home() {
            self.crumb_hits.clear();
            0
        } else {
            self.draw_header(canvas)
        };
        let body_top = header + 8 + self.motion.rise();
        let quick = matches!(
            self.model.screen().source,
            crate::model::Source::Dynamic {
                builtin: pt35_common::menu::Builtin::Quick
                    | pt35_common::menu::Builtin::System
                    | pt35_common::menu::Builtin::About,
                ..
            }
        );
        if !self.on_launcher() {
            self.side_hits.clear();
        }
        let picker = self.model.dynamic_builtin() == Some(pt35_common::menu::Builtin::Windows);
        let launcher_active = !self.model.over_app();
        let page = self.model.page_name().map(str::to_string);
        let screen = (launcher_active, page.clone());
        if self.launcher_sent.as_ref() != Some(&screen) {
            self.launcher_sent = Some(screen);
            let _ = crate::live::request(&pt35_common::ipc::Request::MenuScreen {
                launcher_active,
                page,
            });
        }
        if !picker {
            self.strip_scroll = None;
        }
        if !picker && self.hover_sent.is_some() {
            self.hover_sent = None;
            let _ = crate::live::request(&pt35_common::ipc::Request::SwitcherHover { id: None });
        }
        if quick {
            self.draw_quick(canvas, body_top, body_bottom - 4);
        } else if picker {
            self.draw_windows(canvas, body_top, body_bottom - 4);
        } else if self.on_launcher() {
            self.draw_launcher(canvas, body_top, body_bottom - 4);
        } else {
            match self.model.layout() {
                Layout::Grid => self.draw_tiles(canvas, body_top, body_bottom - 4),
                Layout::List => self.draw_rows(canvas, body_top, body_bottom),
            }
        }
        self.draw_hints(canvas, hint_top);
        self.draw_popup(canvas);
    }
}

fn rain_step() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        / TICK.as_millis() as u64
}

pub fn run(page: Option<String>) -> Result<()> {
    let (theme, theme_problems) = pt35_common::load_theme_report();
    let tree = pt35_common::load_config("pt35/menu.toml");
    // Each face takes ~50ms to parse on a Pi, and the menu is started on every
    // open. In parallel, the four cost one.
    let load = |faces: Vec<String>| {
        std::thread::spawn(move || {
            let mut last = Err(anyhow::anyhow!("no font"));
            for face in faces {
                last = Font::load(&face);
                if last.is_ok() {
                    break;
                }
            }
            last
        })
    };
    // Parsed and drawn beside the fonts, so the first frame does not wait on it.
    let wallpaper = std::thread::spawn(|| {
        pt35_ui::wallpaper::Wallpaper::load(&pt35_common::paths::config_dir().join("wallpaper.svg"))
    });
    let f = &theme.font;
    let font = load(vec![f.family.clone()]);
    let mono = load(vec![f.family_mono.clone(), f.family.clone()]);
    // A missing bold face is not worth failing over: the regular one reads.
    let bold = load(vec![f.family_bold.clone(), f.family.clone()]);
    let mono_bold = load(vec![
        f.family_mono_bold.clone(),
        f.family_mono.clone(),
        f.family.clone(),
    ]);
    let join = |handle: std::thread::JoinHandle<Result<Font>>| {
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("font loader panicked"))?
    };
    let display = (!f.family_display.is_empty()).then(|| load(vec![f.family_display.clone()]));
    let (font, mono, bold, mono_bold) = (join(font)?, join(mono)?, join(bold)?, join(mono_bold)?);
    let body = 480u32
        .saturating_sub(theme.bar.height)
        .saturating_sub(theme.menu.header_height)
        .saturating_sub(theme.menu.hint_height);
    // How many rows actually fit, not how many the theme asks for: a list that
    // hands out one row more than the body can draw hides the cursor on it.
    let rows = (body.saturating_sub(12) / theme.menu.row_height.max(1))
        .clamp(1, theme.menu.rows_visible.max(1)) as usize;
    let grid_rows = (body / (theme.menu.tile_height + theme.menu.gap).max(1)).max(1) as usize;
    // A screen name may be a builtin rather than a page in menu.toml, and that
    // includes the root: the launcher is assembled, not written down.
    let tree: pt35_common::menu::MenuTree = tree;
    let root = pt35_common::menu::Builtin::from_name(&tree.root);
    let mut model = Model::sized(tree, None, rows, grid_rows, theme.menu.columns as usize);
    // A launcher tile is the icon, a line of name and a margin round both.
    let launcher_tile = theme.icons.size_launcher + LAUNCHER_LABEL + theme.menu.gap;
    // The home has no header, so its grid gets that height too.
    let launcher_body = if theme.menu.launcher_home && theme.menu.launcher_columns > 1 {
        body + theme.menu.header_height
    } else {
        body
    };
    model.launcher_grid(
        ((launcher_body + theme.menu.gap).saturating_sub(12) / launcher_tile.max(1)) as usize,
        theme.menu.launcher_columns as usize,
    );
    model.quick_scrolls(theme.menu.row_frames);
    if let Some(builtin) = root {
        model.set_root_dynamic(
            builtin,
            providers::title(builtin),
            providers::items(builtin),
        );
    }
    let start = page
        .as_deref()
        .map(|name| (name, pt35_common::menu::Builtin::from_name(name)));
    if let Some((name, None)) = start {
        model.open_page(name);
        model.mark_entry();
    }
    let mut menu = Menu::new(theme, font, mono, bold, mono_bold, model);
    menu.wallpaper = wallpaper.join().ok().flatten();
    // A display face that will not load is the mono one, not a dead menu.
    menu.display = display.and_then(|handle| join(handle).ok());
    // The banner is the only place a broken theme file can say so.
    if let Some(problem) = theme_problems.first() {
        menu.error = Some(format!("Left out {problem}"));
    }
    // Through `open`, so a slow screen shows its placeholder at once instead of
    // holding the whole menu back until nmcli answers.
    if let Some((_, Some(builtin))) = start.filter(|(_, b)| b.is_some() && *b != root) {
        menu.open(builtin);
        menu.model.mark_entry();
    }
    layer::run(menu, SurfaceSpec::overlay("pt35-menu"))
}
