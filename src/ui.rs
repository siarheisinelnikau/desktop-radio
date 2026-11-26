use crate::{
    audio::Status,
    canvas::{Align, Area, Canvas, Color, alpha, blend, rgb},
    model::{Catalog, FAVORITES, FREQUENCIES, GROUPS, Preferences, group_name, label},
};

pub const WIDTH: i32 = 720;
pub const COMPACT_WIDTH: i32 = 310;
pub const HEIGHT: i32 = 570;
pub const EDIT: Area = Area::new(347., 141., 333., 21.);
pub const LIST: Area = Area::new(307., 132., 390., 404.);
pub const MANAGE_LIST: Area = Area::new(22., 195., 675., 340.);
const VOLUME: Area = Area::new(67., 491., 153., 25.);
const EQ_HEIGHT: f32 = 207.;
const EQ_TOP: f32 = 196.;
const SEARCH_SPACE: f32 = 52.;

pub fn player_list_area(search: f32) -> Area {
    let inset = SEARCH_SPACE * search;
    Area::new(LIST.x, LIST.y + inset, LIST.w, LIST.h - inset)
}

pub fn station_rows(manage: bool, search_open: bool) -> usize {
    if manage || search_open { 6 } else { 7 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Player,
    Equalizer,
    Settings,
    Stations,
}

impl Page {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "player" => Self::Player,
            "equalizer" => Self::Equalizer,
            "settings" => Self::Settings,
            "stations" => Self::Stations,
            _ => return None,
        })
    }

    pub fn back(self) -> Self {
        if self == Self::Stations {
            Self::Settings
        } else {
            Self::Player
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Smooth,
    Intro,
    WindowsStartup,
    Autoplay,
    StartupUpdates,
    Unavailable,
    Equalizer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Close,
    ToggleStationList,
    Page(Page),
    Group(&'static str),
    GroupVisible(&'static str),
    ToggleSearch,
    Search,
    ClearSearch,
    Tune(usize),
    Star(usize),
    Play,
    Step(i32),
    Volume,
    Band(usize),
    ResetBands,
    Language(&'static str),
    Theme(&'static str),
    Meter(&'static str),
    Toggle(Toggle),
    Visibility(usize),
    Move(usize, i32),
    CheckUpdate,
    InstallUpdate,
    Scroll(i32),
    ScrollThumb,
}

impl Action {
    pub fn is_slider(self) -> bool {
        matches!(self, Self::Volume | Self::Band(_) | Self::ScrollThumb)
    }

    pub fn volume(x: f32) -> u8 {
        (((x - VOLUME.x) / VOLUME.w) * 100.).round().clamp(0., 100.) as u8
    }

    pub fn gain(y: f32) -> i8 {
        (12. - (y - EQ_TOP) / EQ_HEIGHT * 24.)
            .round()
            .clamp(-12., 12.) as i8
    }
}

pub struct Hit {
    pub area: Area,
    pub action: Action,
}

pub struct Scrollbar {
    pub track: Area,
    pub thumb: Area,
    pub last: usize,
}

impl Scrollbar {
    pub fn new(area: Area, count: usize, visible: usize, top: usize) -> Option<Self> {
        let last = count.saturating_sub(visible);
        if last == 0 {
            return None;
        }
        let track = A::new(area.right() - 4., area.y + 3., 4., area.h - 9.);
        let length = (track.h * visible as f32 / count as f32).max(21.);
        let offset = (track.h - length) * top.min(last) as f32 / last as f32;
        let thumb = A::new(track.x, track.y + offset, track.w, length);
        Some(Self { track, thumb, last })
    }

    pub fn top_for(&self, pointer_y: f32, grab_offset: f32) -> usize {
        let travel = self.track.h - self.thumb.h;
        let fraction = ((pointer_y - grab_offset - self.track.y) / travel).clamp(0., 1.);
        (fraction * self.last as f32).round() as usize
    }
}

#[derive(Clone, Default)]
pub enum UpdateDisplay {
    #[default]
    Blank,
    Checking,
    Current,
    Unpublished,
    Failed,
    Available(String),
    Downloading(u8),
    Restarting,
    InstallFailed,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub window: Color,
    pub panel: Color,
    pub raised: Color,
    pub outline: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_ink: Color,
    pub accent_panel: Color,
    pub hero_top: Color,
}

impl Palette {
    pub fn for_theme(theme: &str) -> Self {
        let mut palette = Self {
            window: rgb(0x10151e),
            panel: rgb(0x182130),
            raised: rgb(0x202c3f),
            outline: rgb(0x2c3b51),
            text: rgb(0xeff4fc),
            muted: rgb(0xb1c1d6),
            accent: rgb(0xadc9fa),
            accent_ink: rgb(0x162b4d),
            accent_panel: rgb(0x253a5a),
            hero_top: rgb(0x24344d),
        };
        if theme == "amber" {
            palette.accent = rgb(0xf6cf80);
            palette.accent_ink = rgb(0x352407);
            palette.accent_panel = rgb(0x463623);
            palette.hero_top = rgb(0x362d28);
        }
        palette
    }

    pub fn popup_border(self) -> Color {
        blend(self.accent, self.outline, 24)
    }
}

pub struct View<'a> {
    pub page: Page,
    pub width: f32,
    pub station_list_ready: bool,
    pub prefs: &'a Preferences,
    pub catalog: &'a Catalog,
    pub current: Option<usize>,
    pub status: &'a Status,
    pub list: &'a [usize],
    pub top: usize,
    pub query: &'a str,
    pub search_open: bool,
    pub search_amount: f32,
    pub native_search: bool,
    pub group: &'a str,
    pub hot: Option<Action>,
    pub focus: Option<Action>,
    pub spectrum: &'a [f32],
    pub waveform: &'a [f32],
    pub update: &'a UpdateDisplay,
    pub intro: Option<crate::intro::Frame>,
    pub save_failed: bool,
    pub startup_failed: bool,
}

type A = Area;
use Align::{Center, Left, Right};

const METER: A = A::new(43., 161., 225., 83.);
pub const METER_REDRAW: A = A::new(40., 158., 231., 89.);

fn hero_background(canvas: &Canvas, p: Palette) {
    canvas.gradient(
        A::new(22., 87., 266., 451.),
        19.,
        p.hero_top,
        p.panel,
        blend(p.accent, p.outline, 80),
    );
}

pub fn redraw_meter(canvas: &mut Canvas, view: &View<'_>) {
    let palette = Palette::for_theme(&view.prefs.theme);
    canvas.clipped(METER_REDRAW, |canvas| {
        hero_background(canvas, palette);
        Draw {
            canvas,
            view,
            palette,
            hits: Vec::new(),
        }
        .visualization(METER);
    });
}

pub fn render(canvas: &mut Canvas, view: &View<'_>) -> Vec<Hit> {
    let palette = Palette::for_theme(&view.prefs.theme);
    if let Some(frame) = view.intro {
        let scene = crate::intro::Scene::new(frame, view.width);
        if let Some((pivot, scale)) = scene.content() {
            canvas.clear(palette.window);
            canvas.transformed(pivot, (scale, scale), pivot, |canvas| {
                draw_page(canvas, view, palette);
            });
        }
        scene.draw(canvas, palette, &view.prefs.language);
        return Vec::new();
    }
    canvas.clear(palette.window);
    draw_page(canvas, view, palette)
}

fn draw_page(canvas: &mut Canvas, view: &View<'_>, palette: Palette) -> Vec<Hit> {
    let mut surface = Draw {
        canvas,
        view,
        palette,
        hits: Vec::new(),
    };
    surface.header();
    match surface.view.page {
        Page::Player => surface.player(),
        Page::Equalizer => surface.equalizer(),
        Page::Settings => surface.settings(),
        Page::Stations => surface.station_settings(),
    }
    if view.save_failed {
        surface.text(
            A::new(23., 540., view.width - 47., 23.),
            surface.tr("Could not save settings."),
            12,
            false,
            surface.palette.accent,
            Left,
        );
    }
    if view.page == Page::Player
        && [view.hot, view.focus].contains(&Some(Action::ToggleStationList))
    {
        surface.station_list_hint();
    }
    surface.hits
}

struct Draw<'a, 'v> {
    canvas: &'a mut Canvas,
    view: &'a View<'v>,
    palette: Palette,
    hits: Vec<Hit>,
}

impl Draw<'_, '_> {
    fn tr<'a>(&self, text: &'a str) -> &'a str {
        label(text, &self.view.prefs.language)
    }

    fn text(&mut self, area: A, text: &str, size: u8, bold: bool, color: Color, align: Align) {
        self.canvas.text(area, text, size, bold, color, align);
    }

    fn hit(&mut self, action: Action, area: A) {
        self.hits.push(Hit { action, area });
    }

    fn surround(&self, area: A, radius: f32, action: Action) {
        if self.view.focus == Some(action) {
            self.canvas
                .panel(area, radius, 0, Some(self.palette.accent));
        }
    }

    fn choice(&mut self, area: A, action: Action, text: &str, active: bool, size: u8) {
        let p = self.palette;
        let (fill, ink, edge) = if active {
            (p.accent, p.accent_ink, p.accent)
        } else {
            let fill = if self.view.hot == Some(action) {
                p.raised
            } else {
                p.panel
            };
            (fill, p.text, p.outline)
        };
        self.canvas.panel(area, 10., fill, Some(edge));
        self.text(area.inset(3., 0.), text, size, true, ink, Center);
        self.surround(area.inset(2., 2.), 8., action);
        self.hit(action, area);
    }

    fn icon_button(&mut self, area: A, action: Action, icon: Icon, selected: bool) {
        let p = self.palette;
        let surface = if selected {
            p.accent
        } else if self.view.hot == Some(action) {
            p.raised
        } else {
            p.panel
        };
        self.canvas.panel(
            area,
            11.,
            surface,
            Some(if selected { p.accent } else { p.outline }),
        );
        self.icon(
            icon,
            area.center(),
            if selected { p.accent_ink } else { p.text },
        );
        self.surround(area.inset(3., 3.), 8., action);
        self.hit(action, area);
    }

    fn card(&self, area: A) {
        self.canvas
            .panel(area, 16., self.palette.panel, Some(self.palette.outline));
    }

    fn header(&mut self) {
        let p = self.palette;
        let back = self.view.page != Page::Player;
        let right = self.view.width - 22.;
        let title = match self.view.page {
            Page::Player => "Desktop Radio",
            Page::Equalizer => self.tr("Equalizer"),
            Page::Settings => self.tr("Settings"),
            Page::Stations => self.tr("Manage stations"),
        };
        let mark = A::new(22., 18., 40., 40.);
        if back {
            self.icon_button(mark, Action::Page(self.view.page.back()), Icon::Back, false);
        } else {
            self.logo(mark);
        }
        let title_opacity = ((self.view.width - 434.) / 42.).clamp(0., 1.);
        if title_opacity > 0. {
            self.text(
                A::new(73., 17., right - if back { 121. } else { 271. }, 41.),
                title,
                24,
                true,
                alpha(p.text, title_opacity),
                Left,
            );
        }
        if !back {
            self.icon_button(
                A::new(right - 188., 18., 40., 40.),
                Action::ToggleStationList,
                Icon::StationList(self.view.prefs.station_list_collapsed),
                false,
            );
            self.icon_button(
                A::new(right - 138., 18., 40., 40.),
                Action::Page(Page::Equalizer),
                Icon::Equalizer,
                self.view.prefs.equalizer_enabled,
            );
            self.icon_button(
                A::new(right - 88., 18., 40., 40.),
                Action::Page(Page::Settings),
                Icon::Gear,
                false,
            );
        }
        self.icon_button(
            A::new(right - 38., 18., 38., 40.),
            Action::Close,
            Icon::Close,
            false,
        );
        self.canvas.line((22., 72.), (right, 72.), p.outline, 1.);
    }

    fn station_list_hint(&mut self) {
        let area = A::new((self.view.width - 276.).max(22.), 62., 216., 26.);
        let hint = self.tr(if self.view.prefs.station_list_collapsed {
            "Show station list"
        } else {
            "Hide station list"
        });
        self.canvas
            .panel(area, 7., self.palette.raised, Some(self.palette.outline));
        self.text(
            area.inset(8., 0.),
            hint,
            12,
            false,
            self.palette.text,
            Center,
        );
    }

    fn logo(&self, area: A) {
        let p = self.palette;
        self.canvas.panel(area, 12., p.accent, None);
        let (x, y) = area.center();
        for (pos, h) in [5., 10., 18., 10., 5.].into_iter().enumerate() {
            let x = x + (pos as f32 - 2.) * 5.;
            self.canvas
                .line((x, y - h * 0.5), (x, y + h * 0.5), p.accent_ink, 2.4);
        }
    }

    fn player(&mut self) {
        self.hero();
        if self.view.station_list_ready {
            self.station_list();
        } else if self.view.width > LIST.x + 22. {
            let area = A::new(LIST.x - 1., 86., self.view.width - LIST.x - 21., 453.);
            self.canvas.clipped(area, |canvas| {
                Draw {
                    canvas,
                    view: self.view,
                    palette: self.palette,
                    hits: Vec::new(),
                }
                .station_list();
            });
        }
    }

    fn station_list(&mut self) {
        self.group_tabs(A::new(307., 87., 345., 36.), false, false);
        self.icon_button(
            A::new(662., 87., 36., 36.),
            Action::ToggleSearch,
            Icon::Search,
            self.view.search_open,
        );
        let amount = self.view.search_amount;
        let list = player_list_area(amount);
        if amount == 0. || amount == 1. {
            if amount == 1. {
                self.search();
            }
            self.rows(list, false);
            return;
        }

        let reveal = A::new(
            LIST.x - 1.,
            LIST.y - 1.,
            LIST.w + 3.,
            SEARCH_SPACE * amount + 1.,
        );
        self.canvas.clipped(reveal, |canvas| {
            Draw {
                canvas,
                view: self.view,
                palette: self.palette,
                hits: Vec::new(),
            }
            .search();
        });
        self.canvas
            .clipped(A::new(list.x - 1., list.y, list.w + 2., list.h), |canvas| {
                let mut rows = Draw {
                    canvas,
                    view: self.view,
                    palette: self.palette,
                    hits: Vec::new(),
                };
                rows.rows(list, false);
                self.hits.extend(
                    rows.hits
                        .into_iter()
                        .filter(|hit| hit.area.bottom() <= list.bottom()),
                );
            });
    }

    fn hero(&mut self) {
        let p = self.palette;
        hero_background(self.canvas, p);
        let station = self.view.current.map(|index| &self.view.catalog.0[index]);
        let (status, is_live) = match self.view.status {
            Status::Playing(_) => ("On air", true),
            Status::Connecting => ("Connecting…", false),
            Status::Unavailable => ("Station unavailable. Try again.", false),
            Status::AudioDevice => ("Audio device unavailable", false),
            Status::Ready => ("Ready to play", false),
        };
        let badge = A::new(39., 108., 232., 31.);
        let compact = matches!(
            self.view.status,
            Status::Ready | Status::Connecting | Status::Playing(_)
        );
        if compact {
            let pill = A::new(89., 108., 132., 29.);
            self.canvas.panel(
                pill,
                14.5,
                if is_live { p.accent_panel } else { p.raised },
                None,
            );
            if is_live {
                self.canvas.circle(116., 122.5, 3.6, p.accent);
            }
            let text = self.tr(status);
            self.text(
                if is_live {
                    A::new(121., 108., 89., 29.)
                } else {
                    pill
                },
                text,
                13,
                true,
                if is_live { p.accent } else { p.text },
                Center,
            );
        } else {
            self.text(badge, self.tr(status), 13, true, p.accent, Center);
        }
        self.visualization(METER);
        self.text(
            A::new(33., 257., 243., 47.),
            station.map_or(self.tr("Choose a station"), |s| s.name.as_str()),
            30,
            true,
            p.text,
            Center,
        );
        if let Some(station) = station {
            self.text(
                A::new(36., 306., 237., 24.),
                &station.detail(&self.view.prefs.language),
                15,
                false,
                p.muted,
                Center,
            );
            if let Status::Playing(Some(track)) = self.view.status {
                let (artist, title) = track
                    .split_once(" — ")
                    .or_else(|| track.split_once(" - "))
                    .unwrap_or(("", track));
                self.text(
                    A::new(37., 345., 234., 27.),
                    title,
                    16,
                    true,
                    p.accent,
                    Center,
                );
                self.text(
                    A::new(37., 374., 234., 21.),
                    artist,
                    13,
                    false,
                    p.muted,
                    Center,
                );
            }
        }
        let active = matches!(self.view.status, Status::Playing(_) | Status::Connecting);
        self.icon_button(
            A::new(44., 418., 44., 44.),
            Action::Step(-1),
            Icon::Previous,
            false,
        );
        let play = A::new(98., 418., 114., 44.);
        self.canvas.panel(
            play,
            12.,
            if self.view.hot == Some(Action::Play) {
                blend(rgb(0xffffff), p.accent, 33)
            } else {
                p.accent
            },
            None,
        );
        self.icon(
            if active { Icon::Pause } else { Icon::Play },
            (120., 440.),
            p.accent_ink,
        );
        self.text(
            A::new(132., 418., 70., 44.),
            self.tr(if active { "Pause" } else { "Play" }),
            14,
            true,
            p.accent_ink,
            Center,
        );
        self.surround(play.inset(3., 3.), 8., Action::Play);
        self.hit(Action::Play, play);
        self.icon_button(
            A::new(222., 418., 44., 44.),
            Action::Step(1),
            Icon::Next,
            false,
        );
        let value = self.view.prefs.volume;
        let line_y = 504.;
        let thumb = VOLUME.x + VOLUME.w * value as f32 / 100.;
        self.icon(Icon::Volume, (49., line_y), p.muted);
        self.canvas
            .line((VOLUME.x, line_y), (VOLUME.right(), line_y), p.outline, 4.);
        if value != 0 {
            self.canvas
                .line((VOLUME.x, line_y), (thumb, line_y), p.accent, 4.);
        }
        self.canvas.circle(thumb, line_y, 5., p.accent);
        self.text(
            A::new(227., 489., 40., 29.),
            &format!("{value}%"),
            13,
            true,
            p.muted,
            Right,
        );
        self.surround(VOLUME, 6., Action::Volume);
        self.hit(Action::Volume, VOLUME);
    }

    fn visualization(&mut self, area: A) {
        let p = self.palette;
        if self.view.prefs.visualization == "off" {
            return;
        }
        let (_, mid) = area.center();
        self.canvas.line(
            (area.x, mid),
            (area.right(), mid),
            blend(p.accent, p.outline, 27),
            1.,
        );
        if self.view.prefs.visualization == "waveform" {
            if self.view.waveform.is_empty() {
                return;
            }
            let n = self.view.waveform.len();
            let points = self
                .view
                .waveform
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    (
                        area.x + index as f32 * area.w / (n - 1) as f32,
                        mid - sample.clamp(-1., 1.) * area.h * 0.43,
                    )
                })
                .collect::<Vec<_>>();
            self.canvas.polyline(&points, p.accent, 2.);
        } else {
            let levels = self.view.spectrum;
            if levels.is_empty() {
                return;
            }
            let spacing = area.w / levels.len() as f32;
            for (i, value) in levels.iter().enumerate() {
                let height = (value * area.h * 0.45).max(1.);
                let width = (spacing - 5.).max(2.);
                let x = area.x + spacing * i as f32 + 2.5;
                self.canvas.panel(
                    A::new(x, mid - height - 2., width, height),
                    2.5,
                    p.accent,
                    None,
                );
                self.canvas.panel(
                    A::new(x, mid + 2., width, height * 0.58),
                    2.5,
                    blend(p.accent, p.panel, 76),
                    None,
                );
            }
        }
    }

    fn search(&mut self) {
        let p = self.palette;
        let area = A::new(307., 132., 391., 42.);
        self.canvas.panel(area, 12., p.panel, Some(p.outline));
        self.icon(Icon::Search, (329., 153.), p.muted);
        let query = self.view.query;
        if !self.view.native_search {
            let placeholder = self.tr("Search by name or frequency");
            self.text(
                EDIT,
                if query.is_empty() { placeholder } else { query },
                15,
                false,
                if query.is_empty() { p.muted } else { p.text },
                Left,
            );
        }
        self.hit(Action::Search, area);
        if !query.is_empty() {
            let clear = A::new(669., 137., 26., 30.);
            self.icon(Icon::Close, clear.center(), p.muted);
            self.hit(Action::ClearSearch, clear);
        }
    }

    fn group_tabs(&mut self, area: A, all: bool, visibility: bool) {
        let p = self.palette;
        let groups: Vec<_> = GROUPS
            .into_iter()
            .filter(|group| all || !self.view.prefs.hidden_groups.contains(*group))
            .map(|group| (group, group_name(group, &self.view.prefs.language)))
            .collect();
        if groups.is_empty() {
            return;
        }
        self.canvas.panel(area, 11., p.panel, Some(p.outline));
        let weight = |title: &str| {
            if all {
                1.
            } else {
                10. + title.chars().count() as f32
            }
        };
        let unit = (area.w - 8.) / groups.iter().map(|(_, title)| weight(title)).sum::<f32>();
        let font = if (area.w - 8.) / (groups.len() as f32) < 90. {
            12
        } else {
            14
        };
        let mut left = area.x + 4.;
        for (group, title) in groups {
            let width = unit * weight(title);
            let action = if visibility {
                Action::GroupVisible(group)
            } else {
                Action::Group(group)
            };
            let button = A::new(left, area.y + 4., width - 2., area.h - 8.);
            left += width;
            let active = if visibility {
                !self.view.prefs.hidden_groups.contains(group)
            } else {
                group == self.view.group
            };
            let background = if active {
                p.accent
            } else if self.view.hot == Some(action) {
                p.raised
            } else {
                p.panel
            };
            self.canvas.panel(button, 8., background, None);
            self.text(
                button,
                title,
                font,
                true,
                if active { p.accent_ink } else { p.muted },
                Center,
            );
            self.surround(button.inset(1., 1.), 7., action);
            self.hit(action, button);
        }
    }

    fn rows(&mut self, area: A, manage: bool) {
        let p = self.palette;
        let visible = station_rows(manage, self.view.search_open);
        let step = if manage {
            (area.h + 3.) / visible as f32
        } else {
            let closed = (LIST.h + 3.) / station_rows(false, false) as f32;
            let open = (LIST.h - SEARCH_SPACE + 3.) / station_rows(false, true) as f32;
            closed + (open - closed) * self.view.search_amount
        };
        let drawn = ((area.h + 3.) / step - 0.001).ceil() as usize;
        let scrollbar = Scrollbar::new(area, self.view.list.len(), visible, self.view.top);
        for (position, &index) in self
            .view
            .list
            .iter()
            .enumerate()
            .skip(self.view.top)
            .take(drawn)
        {
            let station = &self.view.catalog.0[index];
            let current = self.view.current == Some(index);
            let action = if manage {
                Action::Visibility(index)
            } else {
                Action::Tune(index)
            };
            let row = A::new(
                area.x,
                area.y + (position - self.view.top) as f32 * step,
                area.w - if scrollbar.is_some() { 11. } else { 0. },
                step - 6.,
            );
            let color = if current && !manage {
                p.accent_panel
            } else if self.view.hot == Some(action) {
                p.raised
            } else {
                p.panel
            };
            self.canvas.panel(
                row,
                11.,
                color,
                Some(if current && !manage {
                    blend(p.accent, p.outline, 108)
                } else {
                    p.outline
                }),
            );
            let available_width = row.w - if manage { 153. } else { 58. };
            let tone = if current && !manage { p.accent } else { p.text };
            self.text(
                A::new(row.x + 12., row.y + 5., available_width, 23.),
                &station.name,
                16,
                true,
                tone,
                Left,
            );
            self.text(
                A::new(row.x + 12., row.y + 27., available_width, 19.),
                &station.detail(&self.view.prefs.language),
                12,
                false,
                p.muted,
                Left,
            );
            if manage {
                let visible = !self.view.prefs.hidden_stations.contains(&station.id);
                self.switch(
                    A::new(row.right() - 139., row.y + 9., 40., 34.),
                    action,
                    visible,
                    "",
                );
                if position > 0 {
                    self.icon_button(
                        A::new(row.right() - 85., row.y + 8., 33., 35.),
                        Action::Move(index, -1),
                        Icon::Up,
                        false,
                    );
                }
                if position + 1 < self.view.list.len() {
                    self.icon_button(
                        A::new(row.right() - 44., row.y + 8., 33., 35.),
                        Action::Move(index, 1),
                        Icon::Down,
                        false,
                    );
                }
            } else {
                self.surround(row.inset(2., 2.), 10., action);
                self.hit(action, row);
                let star = A::new(row.right() - 43., row.y + 6., 36., row.h - 12.);
                let starred = self.view.prefs.favorites.contains(&station.id);
                if self.view.hot == Some(Action::Star(index)) {
                    self.canvas.panel(star, 8., p.raised, None);
                }
                self.icon(
                    Icon::Star(starred),
                    star.center(),
                    if starred { p.accent } else { p.muted },
                );
                self.surround(star, 8., Action::Star(index));
                self.hit(Action::Star(index), star);
            }
        }
        if self.view.list.is_empty() {
            let text = if !self.view.query.is_empty() && !manage {
                "No matching stations"
            } else if self.view.group == FAVORITES {
                "No favorites yet"
            } else {
                "No visible stations"
            };
            self.card(area);
            self.text(
                area.inset(15., 0.),
                self.tr(text),
                16,
                false,
                p.muted,
                Center,
            );
        } else if let Some(Scrollbar { track, thumb, .. }) = scrollbar {
            self.canvas.panel(track, 2., p.outline, None);
            self.canvas.panel(
                thumb,
                2.,
                if self.view.hot == Some(Action::ScrollThumb)
                    || self.view.focus == Some(Action::ScrollThumb)
                {
                    p.accent
                } else {
                    p.muted
                },
                None,
            );
            let left = track.x - 6.;
            let width = track.w + 10.;
            if thumb.y > track.y {
                self.hit(
                    Action::Scroll(-5),
                    A::new(left, track.y, width, thumb.y - track.y),
                );
            }
            if thumb.bottom() < track.bottom() {
                self.hit(
                    Action::Scroll(5),
                    A::new(left, thumb.bottom(), width, track.bottom() - thumb.bottom()),
                );
            }
            self.hit(Action::ScrollThumb, A::new(left, thumb.y, width, thumb.h));
        }
    }

    fn equalizer(&mut self) {
        let p = self.palette;
        let prefs = self.view.prefs;
        self.card(A::new(22., 87., 676., 393.));
        self.switch(
            A::new(40., 101., 637., 44.),
            Action::Toggle(Toggle::Equalizer),
            prefs.equalizer_enabled,
            self.tr("Enable equalizer"),
        );
        self.canvas.line((39., 150.), (679., 150.), p.outline, 1.);
        let grid_x = 86.;
        let grid_w = 574.;
        for gain in [-12, -6, 0, 6, 12] {
            let y = EQ_TOP + (12 - gain) as f32 * EQ_HEIGHT / 24.;
            self.canvas.line(
                (grid_x, y),
                (grid_x + grid_w, y),
                if gain == 0 { p.muted } else { p.outline },
                1.,
            );
            let text = if gain == 12 {
                "+12 dB".to_owned()
            } else if gain == -12 {
                "−12 dB".to_owned()
            } else if gain == 0 {
                "0".to_owned()
            } else {
                String::new()
            };
            self.text(
                A::new(36., y - 10., 42., 20.),
                &text,
                11,
                false,
                p.muted,
                Right,
            );
        }
        for (band, frequency) in FREQUENCIES.into_iter().enumerate() {
            let x = grid_x + 27. + band as f32 * 57.5;
            let gain = prefs.equalizer[band];
            let y = EQ_TOP + (12 - gain) as f32 * EQ_HEIGHT / 24.;
            let center = EQ_TOP + EQ_HEIGHT * 0.5;
            let ink = if prefs.equalizer_enabled {
                p.accent
            } else {
                p.muted
            };
            self.canvas
                .line((x, EQ_TOP), (x, EQ_TOP + EQ_HEIGHT), p.outline, 4.);
            if gain != 0 {
                self.canvas.line((x, center), (x, y), ink, 4.);
            }
            self.canvas.circle(
                x,
                y,
                if self.view.hot == Some(Action::Band(band)) {
                    9.
                } else {
                    7.
                },
                ink,
            );
            let text = if frequency >= 1000 {
                format!("{}k", frequency / 1000)
            } else {
                frequency.to_string()
            };
            self.text(
                A::new(x - 26., 417., 52., 22.),
                &text,
                14,
                true,
                p.text,
                Center,
            );
            self.text(
                A::new(x - 26., 442., 52., 23.),
                &if gain == 0 {
                    "0 dB".into()
                } else {
                    format!("{gain:+} dB")
                },
                12,
                false,
                ink,
                Center,
            );
            let hit = A::new(x - 17., EQ_TOP - 11., 34., EQ_HEIGHT + 22.);
            self.surround(hit, 7., Action::Band(band));
            self.hit(Action::Band(band), hit);
        }
        self.choice(
            A::new(22., 497., 137., 41.),
            Action::ResetBands,
            self.tr("Reset bands"),
            false,
            14,
        );
        self.choice(
            A::new(582., 497., 116., 41.),
            Action::Page(Page::Player),
            self.tr("Done"),
            true,
            14,
        );
    }

    fn settings(&mut self) {
        let p = self.palette;
        let prefs = self.view.prefs;
        self.card(A::new(22., 87., 326., 451.));
        self.card(A::new(362., 87., 336., 160.));
        self.card(A::new(362., 260., 336., 219.));
        self.text(
            A::new(38., 101., 275., 31.),
            self.tr("Appearance"),
            18,
            true,
            p.text,
            Left,
        );
        self.segment_settings(
            144.,
            self.tr("Language"),
            &[
                ("English", "en", Action::Language("en")),
                ("Русский", "ru", Action::Language("ru")),
                ("Қазақша", "kk", Action::Language("kk")),
            ],
            &prefs.language,
        );
        self.segment_settings(
            225.,
            self.tr("Theme"),
            &[
                (self.tr("Graphite"), "graphite", Action::Theme("graphite")),
                (self.tr("Amber"), "amber", Action::Theme("amber")),
            ],
            &prefs.theme,
        );
        self.segment_settings(
            306.,
            self.tr("Visualization"),
            &[
                (self.tr("Spectrum"), "spectrum", Action::Meter("spectrum")),
                (self.tr("Waveform"), "waveform", Action::Meter("waveform")),
                (self.tr("Off"), "off", Action::Meter("off")),
            ],
            &prefs.visualization,
        );
        self.switch(
            A::new(37., 388., 297., 43.),
            Action::Toggle(Toggle::Smooth),
            prefs.smooth_visualization,
            self.tr("Smooth visualization"),
        );
        self.switch(
            A::new(37., 440., 297., 43.),
            Action::Toggle(Toggle::Intro),
            prefs.show_intro,
            self.tr("Show intro when opening the panel"),
        );
        self.text(
            A::new(378., 101., 302., 31.),
            self.tr("Startup and playback"),
            18,
            true,
            p.text,
            Left,
        );
        self.switch(
            A::new(377., 138., 304., 36.),
            Action::Toggle(Toggle::WindowsStartup),
            prefs.start_with_windows,
            self.tr("Start with Windows"),
        );
        self.switch(
            A::new(377., 179., 304., 36.),
            Action::Toggle(Toggle::Autoplay),
            prefs.play_on_startup,
            self.tr("Resume last station on startup"),
        );
        if self.view.startup_failed {
            self.text(
                A::new(378., 216., 304., 22.),
                self.tr("Could not change Windows startup."),
                13,
                false,
                p.accent,
                Left,
            );
        }
        self.text(
            A::new(378., 273., 189., 32.),
            self.tr("Updates"),
            18,
            true,
            p.text,
            Left,
        );
        self.text(
            A::new(580., 274., 101., 30.),
            concat!("v", env!("CARGO_PKG_VERSION")),
            14,
            false,
            p.muted,
            Right,
        );
        self.switch(
            A::new(377., 314., 305., 41.),
            Action::Toggle(Toggle::StartupUpdates),
            prefs.check_updates_on_startup,
            self.tr("Check for updates on startup"),
        );
        let available = matches!(self.view.update, UpdateDisplay::Available(_));
        let busy = matches!(
            self.view.update,
            UpdateDisplay::Checking | UpdateDisplay::Downloading(_) | UpdateDisplay::Restarting
        );
        if !busy {
            let (action, title) = if available {
                (Action::InstallUpdate, self.tr("Download and install"))
            } else {
                (Action::CheckUpdate, self.tr("Check for updates"))
            };
            self.choice(A::new(378., 367., 304., 43.), action, title, available, 14);
        }
        let message = match self.view.update {
            UpdateDisplay::Blank => String::new(),
            UpdateDisplay::Checking => self.tr("Checking for updates…").to_owned(),
            UpdateDisplay::Current => self.tr("You have the latest version.").to_owned(),
            UpdateDisplay::Unpublished => self
                .tr("No published Windows build is accessible.")
                .to_owned(),
            UpdateDisplay::Failed => self.tr("Could not check for updates.").to_owned(),
            UpdateDisplay::Available(version) => {
                format!("{}  ·  v{version}", self.tr("Update available"))
            }
            UpdateDisplay::Downloading(percent) => {
                format!("{}  {percent}%", self.tr("Downloading…"))
            }
            UpdateDisplay::Restarting => self.tr("Download verified. Restarting…").to_owned(),
            UpdateDisplay::InstallFailed => self.tr("Could not install the update.").to_owned(),
        };
        self.text(
            A::new(377., 422., 305., 30.),
            &message,
            13,
            false,
            p.muted,
            Left,
        );
        if let UpdateDisplay::Downloading(percent) = self.view.update {
            let width = 303. * *percent as f32 / 100.;
            self.canvas
                .panel(A::new(378., 461., 304., 5.), 2., p.outline, None);
            if width > 0. {
                self.canvas
                    .panel(A::new(378., 461., width, 5.), 2., p.accent, None);
            }
        }
        self.choice(
            A::new(362., 492., 336., 46.),
            Action::Page(Page::Stations),
            self.tr("Manage stations"),
            false,
            14,
        );
    }

    fn segment_settings(
        &mut self,
        y: f32,
        label: &str,
        options: &[(&str, &str, Action)],
        selected: &str,
    ) {
        self.text(
            A::new(37., y, 293., 24.),
            label,
            13,
            false,
            self.palette.muted,
            Left,
        );
        let gap = 4.;
        let width = (296. - gap * (options.len() - 1) as f32) / options.len() as f32;
        for (index, (name, value, action)) in options.iter().enumerate() {
            self.choice(
                A::new(37. + (width + gap) * index as f32, y + 29., width, 37.),
                *action,
                name,
                *value == selected,
                13,
            );
        }
    }

    fn station_settings(&mut self) {
        let p = self.palette;
        self.text(
            A::new(22., 84., 306., 28.),
            self.tr("Visible groups"),
            14,
            true,
            p.text,
            Left,
        );
        self.switch(
            A::new(360., 79., 337., 35.),
            Action::Toggle(Toggle::Unavailable),
            self.view.prefs.hide_unavailable,
            self.tr("Hide recently unavailable stations"),
        );
        self.group_tabs(A::new(22., 116., 676., 34.), true, true);
        self.group_tabs(A::new(22., 157., 676., 31.), true, false);
        self.rows(MANAGE_LIST, true);
    }

    fn switch(&mut self, area: A, action: Action, on: bool, label: &str) {
        let p = self.palette;
        let (_, y) = area.center();
        let right = area.right() - 3.;
        let track = A::new(right - 34., y - 9., 34., 18.);
        if !label.is_empty() {
            self.text(
                A::new(area.x, area.y, area.w - 46., area.h),
                label,
                13,
                false,
                p.text,
                Left,
            );
        }
        self.canvas
            .panel(track, 9., if on { p.accent } else { p.outline }, None);
        self.canvas.circle(
            if on { right - 9. } else { right - 25. },
            y,
            6.,
            if on { p.accent_ink } else { p.muted },
        );
        self.surround(area, 9., action);
        self.hit(action, area);
    }

    fn icon(&self, icon: Icon, (x, y): (f32, f32), color: Color) {
        let c = &*self.canvas;
        let line = |a: (f32, f32), b: (f32, f32)| {
            c.line((x + a.0, y + a.1), (x + b.0, y + b.1), color, 1.55)
        };
        match icon {
            Icon::Close => {
                line((-5., -5.), (5., 5.));
                line((-5., 5.), (5., -5.));
            }
            Icon::Back => {
                line((-7., 0.), (7., 0.));
                line((-7., 0.), (-2., -5.));
                line((-7., 0.), (-2., 5.));
            }
            Icon::StationList(collapsed) => {
                c.panel(A::new(x - 8., y - 6., 16., 12.), 2., 0, Some(color));
                line((-2.5, -5.5), (-2.5, 5.5));
                let dir = if collapsed { -1. } else { 1. };
                line((2.6 - 1.5 * dir, -2.1), (2.6 + 1.5 * dir, 0.));
                line((2.6 - 1.5 * dir, 2.1), (2.6 + 1.5 * dir, 0.));
            }
            Icon::Up | Icon::Down => {
                let dir = if matches!(icon, Icon::Up) { 1. } else { -1. };
                line((-5., 2.5 * dir), (0., -2.5 * dir));
                line((0., -2.5 * dir), (5., 2.5 * dir));
            }
            Icon::Play | Icon::Previous | Icon::Next => {
                let dir = if matches!(icon, Icon::Previous) {
                    -1.
                } else {
                    1.
                };
                let dx = if matches!(icon, Icon::Play) {
                    0.
                } else {
                    -1.5 * dir
                };
                c.polygon(
                    &[
                        (x - 4. * dir + dx, y - 6.),
                        (x + 6. * dir + dx, y),
                        (x - 4. * dir + dx, y + 6.),
                    ],
                    color,
                    true,
                );
                if !matches!(icon, Icon::Play) {
                    line((7. * dir, -6.), (7. * dir, 6.));
                }
            }
            Icon::Pause => {
                c.panel(A::new(x - 5., y - 6., 3.5, 12.), 1., color, None);
                c.panel(A::new(x + 1.5, y - 6., 3.5, 12.), 1., color, None);
            }
            Icon::Search => {
                c.panel(A::new(x - 7., y - 7., 10., 10.), 5., 0, Some(color));
                line((2., 2.), (6., 6.));
            }
            Icon::Equalizer => {
                for (i, center) in [-2., 4., -3.].into_iter().enumerate() {
                    let px = (i as f32 - 1.) * 6.;
                    line((px, -7.), (px, 7.));
                    c.circle(x + px, y + center, 2.3, color);
                }
            }
            Icon::Gear => {
                let points = std::array::from_fn::<_, 32, _>(|index| {
                    let angle = std::f32::consts::TAU * (index as f32 + 0.5) / 32.;
                    let radius = if matches!(index % 4, 0 | 3) { 6.3 } else { 8.4 };
                    (x + radius * angle.cos(), y + radius * angle.sin())
                });
                c.polygon(&points, color, false);
                c.panel(A::new(x - 2.9, y - 2.9, 5.8, 5.8), 2.9, 0, Some(color));
            }
            Icon::Volume => {
                c.polygon(
                    &[
                        (x - 7., y - 2.5),
                        (x - 3., y - 2.5),
                        (x + 1., y - 5.5),
                        (x + 1., y + 5.5),
                        (x - 3., y + 2.5),
                        (x - 7., y + 2.5),
                    ],
                    color,
                    true,
                );
                line((3.5, -3.), (5., 0.));
                line((5., 0.), (3.5, 3.));
            }
            Icon::Star(fill) => {
                let points = std::array::from_fn::<_, 10, _>(|i| {
                    let angle =
                        std::f32::consts::TAU * i as f32 / 10. - std::f32::consts::FRAC_PI_2;
                    let radius = if i % 2 == 0 { 8. } else { 3.5 };
                    (x + angle.cos() * radius, y + angle.sin() * radius)
                });
                c.polygon(&points, color, fill);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Back,
    Close,
    StationList(bool),
    Up,
    Down,
    Play,
    Pause,
    Previous,
    Next,
    Search,
    Equalizer,
    Gear,
    Volume,
    Star(bool),
}
