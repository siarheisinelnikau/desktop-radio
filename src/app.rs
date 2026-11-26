#![allow(unsafe_op_in_unsafe_fn)]

use crate::{
    audio::{self, Engine, METER_SIZE, Signal, Status},
    canvas::{Canvas, GdiPlus, Surface},
    http::wide,
    intro::{Animation, END_MS, FRAME_MS, Frame},
    media::{MediaKey, MediaSession},
    model::{Catalog, GROUPS, Preferences, UnavailableAt, data_dir, label, now},
    ui::{
        self, Action, COMPACT_WIDTH, EDIT, HEIGHT, Hit, LIST, MANAGE_LIST, Page, Palette, Toggle,
        UpdateDisplay, View, WIDTH,
    },
    updates::{self, Check, Release},
};
use anyhow::{Context, Result, bail, ensure};
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::{
    cell::RefCell,
    ffi::{OsString, c_void},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{
        Dwm::{
            DWMNCRP_DISABLED, DWMNCRP_USEWINDOWSTYLE, DWMWA_BORDER_COLOR, DWMWA_CLOAK,
            DWMWA_COLOR_DEFAULT, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_ENABLED,
            DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmFlush,
            DwmGetWindowAttribute, DwmSetWindowAttribute,
        },
        Gdi::*,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Registry::{
            HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
            RegSetKeyValueW,
        },
        Threading::{CreateMutexW, GetCurrentProcess, GetProcessTimes},
    },
    UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

const CLASS: &str = "DesktopRadio.TrayWindow";
const WAKE: u32 = WM_APP + 1;
const TRAY: u32 = WM_APP + 2;
const ACTIVATE: u32 = WM_APP + 3;
const EDIT_KEY: u32 = WM_APP + 4;
const METER_TIMER: usize = 1;
const METER_FRAME_MS: u32 = 15;
const INTRO_TIMER: usize = 2;
const SAVE_TIMER: usize = 3;
const DIAG_TIMER: usize = 4;
const PANEL_TIMER: usize = 5;
const PANEL_TRANSITION_MS: f32 = 245.;
const EDIT_ID: usize = 101;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const APPCOMMAND_MEDIA_NEXTTRACK: u32 = 11;
const APPCOMMAND_MEDIA_PREVIOUSTRACK: u32 = 12;
const APPCOMMAND_MEDIA_PLAY_PAUSE: u32 = 14;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

enum Event {
    Audio(u64, Status),
    Media(MediaKey),
    Checked(bool, Result<Check, String>),
    Progress(u8),
    Downloaded(Result<(), String>),
}

fn publish(sender: &Sender<Event>, window: usize, value: Event) {
    if sender.send(value).is_ok() {
        unsafe {
            PostMessageW(window as HWND, WAKE, 0, 0);
        }
    }
}

pub fn run() -> Result<()> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let action = arguments.first().and_then(|arg| arg.to_str()).unwrap_or("");
    match (action, arguments.len()) {
        ("--capture", 3..=6) => capture(&arguments[1..]),
        ("--diagnose-audio", 3) => {
            audio::diagnose(Path::new(&arguments[1]), Path::new(&arguments[2]))
        }
        ("--apply-update", 5) => updates::apply(
            Path::new(&arguments[1]),
            unicode(&arguments[2])?,
            unicode(&arguments[3])?.parse()?,
            unicode(&arguments[4])?,
        ),
        ("--diagnose-ui", 2) => run_window(
            Some(Diagnostics::new(PathBuf::from(&arguments[1]))),
            false,
            false,
        ),
        ("--diagnose-layout", 2) => {
            let mut report = Diagnostics::new(PathBuf::from(&arguments[1]));
            report.measure_layout = true;
            run_window(Some(report), false, false)
        }
        ("--diagnose-meter", 4) => {
            let mode = unicode(&arguments[3])?;
            ensure!(
                matches!(mode, "spectrum" | "waveform"),
                "Unknown visualization mode"
            );
            let path = Path::new(&arguments[2]).canonicalize()?;
            let source = url::Url::from_file_path(&path)
                .map_err(|_| anyhow::anyhow!("Invalid audio test file path"))?;
            let mut report = Diagnostics::new(PathBuf::from(&arguments[1]));
            report.meter_source = Some((source.to_string(), mode.to_owned()));
            run_window(Some(report), false, false)
        }
        ("--cleanup-update" | "--update-failed", 2) => {
            updates::cleanup(unicode(&arguments[1])?);
            run_window(None, action == "--update-failed", false)
        }
        ("", 0) => run_window(None, false, false),
        ("--autostart", 1) => run_window(None, false, true),
        _ => bail!("Unknown arguments"),
    }
}

fn unicode(string: &OsString) -> Result<&str> {
    string.to_str().context("Invalid Unicode argument")
}

fn run_window(report: Option<Diagnostics>, update_failed: bool, autostart: bool) -> Result<()> {
    let _gdi = GdiPlus::start()?;
    let _instance_guard = if report.is_none() {
        let mutex = unsafe { CreateMutexW(null(), 0, wide("Local\\DesktopRadio.Rust").as_ptr()) };
        ensure!(!mutex.is_null(), "Create the application instance mutex");
        let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let guard = InstanceGuard(mutex);
        if exists {
            if autostart {
                return Ok(());
            }
            for _ in 0..20 {
                let window = unsafe { FindWindowW(wide(CLASS).as_ptr(), null()) };
                if !window.is_null() {
                    unsafe {
                        PostMessageW(window, ACTIVATE, 0, 0);
                    }
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(35));
            }
            bail!("Desktop Radio is starting");
        }
        Some(guard)
    } else {
        None
    };
    let diagnostic = report.is_some();
    let window = create_window(diagnostic)?;
    let mut app = App::new(window, report)?;
    if autostart && !app.prefs.start_with_windows {
        return Ok(());
    }
    if update_failed {
        app.update = UpdateDisplay::InstallFailed;
        app.set_page(Page::Settings);
    }
    APP.with(|state| *state.borrow_mut() = Some(app));
    with_app(|app| {
        if diagnostic {
            let skip_initial_intro = app
                .report
                .as_ref()
                .is_some_and(|report| report.measure_layout || report.meter_source.is_some());
            app.show(!skip_initial_intro);
            unsafe {
                SetTimer(
                    window,
                    DIAG_TIMER,
                    if skip_initial_intro { 100 } else { 3100 },
                    None,
                );
            }
        } else {
            if !autostart {
                app.startup_failed = set_windows_startup(app.prefs.start_with_windows).is_err();
            }
            if app.prefs.play_on_startup {
                if let Some(index) = app.current {
                    app.tune(index);
                }
            }
            if app.prefs.check_updates_on_startup && !update_failed {
                app.check_update(false);
            }
            if update_failed || (!autostart && !app.tray.registered) {
                app.show(!update_failed);
            }
        }
    });
    let mut message = MSG::default();
    let mut result;
    while {
        result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        result > 0
    } {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    APP.with(|state| state.borrow_mut().take());
    ensure!(result != -1, "Windows message loop failed");
    if message.wParam != 0 {
        bail!("Desktop Radio diagnostics failed");
    }
    Ok(())
}

fn set_windows_startup(enabled: bool) -> Result<()> {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide("DesktopRadio");
    if !enabled {
        let result = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        ensure!(
            matches!(
                result,
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND
            ),
            "Remove the Desktop Radio startup entry (Windows error {result})"
        );
        return Ok(());
    }

    let path = std::env::current_exe().context("Locate Desktop Radio")?;
    let command = std::iter::once(b'"' as u16)
        .chain(path.as_os_str().encode_wide())
        .chain(wide("\" --autostart"))
        .collect::<Vec<_>>();
    ensure!(
        command.len() <= 261,
        "Desktop Radio startup command is too long"
    );
    let size = size_of_val(command.as_slice()) as u32;
    let mut previous = vec![0_u16; command.len()];
    let mut previous_size = size;
    let found = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            previous.as_mut_ptr().cast(),
            &mut previous_size,
        )
    };
    if found == ERROR_SUCCESS && previous_size == size && previous == command {
        return Ok(());
    }
    let result = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            command.as_ptr().cast(),
            size,
        )
    };
    ensure!(
        result == ERROR_SUCCESS,
        "Register Desktop Radio at startup (Windows error {result})"
    );
    Ok(())
}

struct InstanceGuard(HANDLE);
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn create_window(diagnostic: bool) -> Result<HWND> {
    let instance = unsafe { GetModuleHandleW(null()) };
    let class = wide(if diagnostic {
        "DesktopRadio.DiagnosticWindow"
    } else {
        CLASS
    });
    let title = wide("Desktop Radio");
    unsafe {
        let icon = LoadImageW(instance, 1 as _, IMAGE_ICON, 32, 32, LR_SHARED) as HICON;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: icon,
            ..Default::default()
        };
        ensure!(
            RegisterClassW(&wc) != 0,
            "Register the Desktop Radio window"
        );
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            WIDTH,
            HEIGHT,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        ensure!(!hwnd.is_null(), "Create the Desktop Radio window");
        let corner = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const corner).cast::<c_void>(),
            size_of_val(&corner) as u32,
        );
        Ok(hwnd)
    }
}

fn with_app<R>(handle: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        state.as_mut().map(handle)
    })
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_DESTROY {
        PostQuitMessage(w as i32);
        return 0;
    }
    if message == WM_ERASEBKGND {
        return 1;
    }
    if let Some(result) = with_app(|app| app.message(message, w, l)).flatten() {
        return result;
    }
    DefWindowProcW(hwnd, message, w, l)
}

struct App {
    window: HWND,
    edit: HWND,
    edit_font: HFONT,
    edit_brush: HBRUSH,
    surface: Option<Surface>,
    tray: Tray,
    media: Option<MediaSession>,
    engine: Engine,
    sender: Sender<Event>,
    events: Receiver<Event>,
    prefs: Preferences,
    settings: PathBuf,
    catalog: Catalog,
    page: Page,
    player_width: f32,
    panel_resize: Option<PanelResize>,
    search_after_resize: bool,
    popup_extent: (i32, i32),
    popup_clipped: bool,
    manage_group: &'static str,
    list: Vec<usize>,
    top: usize,
    query: String,
    current: Option<usize>,
    revision: u64,
    status: Status,
    hits: Vec<Hit>,
    hot: Option<Action>,
    focus: Option<Action>,
    drag: Option<Action>,
    scale: f32,
    dirty: bool,
    meter_dirty: bool,
    visible: bool,
    meter: Meter,
    intro: Option<Animation>,
    save_failed: bool,
    startup_failed: bool,
    release: Option<Release>,
    update: UpdateDisplay,
    checking: bool,
    report: Option<Diagnostics>,
    renders: u64,
    presents: u64,
    tray_tip: String,
}

impl App {
    fn new(window: HWND, report: Option<Diagnostics>) -> Result<Self> {
        let diagnostic = report.is_some();
        let settings = data_dir().join("settings.json");
        let mut prefs = if diagnostic {
            Preferences::default()
        } else {
            Preferences::load(&settings)
        };
        let catalog = Catalog::load();
        if diagnostic {
            apply_fixture(&mut prefs);
        }
        if let Some((_, mode)) = report
            .as_ref()
            .and_then(|report| report.meter_source.as_ref())
        {
            prefs.visualization = mode.clone();
            prefs.volume = 0;
        }
        let current = prefs.station_id.as_deref().and_then(|id| catalog.get(id));
        let (sender, events) = mpsc::channel();
        let address = window as usize;
        let audio_sender = sender.clone();
        let engine = Engine::start(&prefs, move |revision, status| {
            publish(&audio_sender, address, Event::Audio(revision, status))
        });
        let media_sender = sender.clone();
        let media = MediaSession::attach(
            windows::Win32::Foundation::HWND(window.cast()),
            move |key| publish(&media_sender, address, Event::Media(key)),
        );
        let instance = unsafe { GetModuleHandleW(null()) };
        let edit = unsafe {
            CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                null(),
                WS_CHILD | ES_AUTOHSCROLL as u32,
                0,
                0,
                20,
                20,
                window,
                EDIT_ID as _,
                instance,
                null(),
            )
        };
        ensure!(!edit.is_null(), "Create the station search field");
        unsafe {
            SetWindowSubclass(edit, Some(search_proc), 1, address);
        }
        let tray = Tray::new(window, instance);
        let player_width = player_width(prefs.station_list_collapsed);
        let mut app = Self {
            window,
            edit,
            edit_font: null_mut(),
            edit_brush: null_mut(),
            surface: None,
            tray,
            media,
            engine,
            sender,
            events,
            prefs,
            settings,
            catalog,
            page: Page::Player,
            player_width,
            panel_resize: None,
            search_after_resize: false,
            popup_extent: (WIDTH, HEIGHT),
            popup_clipped: false,
            manage_group: GROUPS[0],
            list: Vec::new(),
            top: 0,
            query: String::new(),
            current,
            revision: 0,
            status: Status::Ready,
            hits: Vec::new(),
            hot: None,
            focus: None,
            drag: None,
            scale: 1.,
            dirty: true,
            meter_dirty: false,
            visible: false,
            meter: Meter::new(),
            intro: None,
            save_failed: false,
            startup_failed: false,
            release: None,
            update: UpdateDisplay::Blank,
            checking: false,
            report,
            renders: 0,
            presents: 0,
            tray_tip: String::new(),
        };
        app.update_edit();
        app.relist(true);
        app.update_system_media();
        Ok(app)
    }

    fn message(&mut self, message: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        if self.report.is_some()
            && matches!(
                message,
                TRAY | WM_CLOSE
                    | WM_MOUSEMOVE
                    | WM_MOUSELEAVE
                    | WM_LBUTTONDOWN
                    | WM_LBUTTONUP
                    | WM_MOUSEWHEEL
                    | WM_KEYDOWN
                    | EDIT_KEY
                    | WM_APPCOMMAND
            )
        {
            return Some(0);
        }
        let handled = match message {
            WM_NCHITTEST if self.report.is_some() => return Some(HTTRANSPARENT as LRESULT),
            WM_PAINT => {
                self.paint();
                true
            }
            WM_CLOSE => {
                self.hide();
                true
            }
            WM_CONTEXTMENU if !self.tray.registered => {
                self.menu();
                true
            }
            ACTIVATE => {
                self.show(true);
                true
            }
            TRAY => {
                self.tray_event(l as u32 & 0xffff);
                true
            }
            WAKE => {
                self.receive();
                true
            }
            WM_TIMER => {
                self.timer(w);
                true
            }
            WM_SIZE => {
                self.surface = None;
                self.mark_dirty();
                true
            }
            WM_DPICHANGED => {
                if self.visible {
                    self.place();
                    self.mark_dirty();
                }
                true
            }
            WM_CTLCOLOREDIT if l as HWND == self.edit => {
                let theme = Palette::for_theme(&self.prefs.theme);
                unsafe {
                    SetTextColor(w as HDC, color_ref(theme.text));
                    SetBkColor(w as HDC, color_ref(theme.panel));
                }
                return Some(self.edit_brush as LRESULT);
            }
            WM_COMMAND if (w & 0xffff) == EDIT_ID && (w >> 16) as u32 == EN_CHANGE => {
                let mut buffer = [0u16; 251];
                let count =
                    unsafe { GetWindowTextW(self.edit, buffer.as_mut_ptr(), buffer.len() as i32) };
                if count >= 0 {
                    let text = String::from_utf16_lossy(&buffer[..count as usize]);
                    if text != self.query {
                        self.query = text;
                        self.relist(true);
                        self.mark_dirty();
                    }
                }
                true
            }
            WM_MOUSEMOVE => {
                let (x, y) = self.logical(l);
                if let Some(drag) = self.drag {
                    self.perform(drag, x, y);
                }
                let hot = self.at(x, y);
                if self.hot != hot {
                    self.hot = hot;
                    self.mark_dirty();
                    let mut tracking = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.window,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        TrackMouseEvent(&mut tracking);
                    }
                }
                true
            }
            WM_MOUSELEAVE => {
                if self.hot.take().is_some() {
                    self.mark_dirty();
                }
                true
            }
            WM_LBUTTONDOWN => {
                if self.intro.is_some() {
                    self.dismiss_intro();
                    return Some(0);
                }
                let (x, y) = self.logical(l);
                if let Some(action) = self.at(x, y) {
                    self.focus = None;
                    if action != Action::Search {
                        unsafe {
                            SetFocus(self.window);
                        }
                    }
                    if action.is_slider() {
                        self.drag = Some(action);
                        unsafe {
                            SetCapture(self.window);
                        }
                    }
                    self.perform(action, x, y);
                }
                true
            }
            WM_LBUTTONUP => {
                if self.drag.take().is_some() {
                    unsafe {
                        ReleaseCapture();
                    }
                }
                true
            }
            WM_CAPTURECHANGED => {
                self.drag = None;
                true
            }
            WM_MOUSEWHEEL => {
                let mut point = POINT {
                    x: (l as u16 as i16) as i32,
                    y: ((l >> 16) as u16 as i16) as i32,
                };
                unsafe {
                    ScreenToClient(self.window, &mut point);
                }
                let area = if self.page == Page::Stations {
                    MANAGE_LIST
                } else {
                    LIST
                };
                if self.can_scroll_stations()
                    && area.contains(
                        (point.x - self.popup_left()) as f32 / self.scale,
                        point.y as f32 / self.scale,
                    )
                {
                    self.scroll(if ((w >> 16) as u16 as i16) > 0 { -3 } else { 3 });
                }
                true
            }
            WM_KEYDOWN | EDIT_KEY => self.key(w as u16, message == EDIT_KEY),
            WM_APPCOMMAND => {
                let code = ((l >> 16) & 0x7ff) as u32;
                match code {
                    APPCOMMAND_MEDIA_NEXTTRACK => self.navigate(1),
                    APPCOMMAND_MEDIA_PREVIOUSTRACK => self.navigate(-1),
                    APPCOMMAND_MEDIA_PLAY_PAUSE => self.toggle(),
                    _ => return None,
                }
                return Some(1);
            }
            other if other == self.tray.taskbar_created => {
                self.tray.add();
                self.tray_tip.clear();
                self.update_system_media();
                true
            }
            _ => false,
        };
        handled.then_some(0)
    }

    fn paint(&mut self) {
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(self.window, &mut paint);
            let meter_only = self.meter_dirty && !self.dirty;
            if self.visible
                && !dc.is_null()
                && self.render_client()
                && self
                    .surface
                    .as_ref()
                    .is_some_and(|surface| surface.blit(dc))
            {
                self.presents += 1;
                if meter_only {
                    if let Some(report) = &mut self.report {
                        report
                            .meter_presents
                            .push(report.start.elapsed().as_secs_f64() * 1000.);
                    }
                }
            }
            EndPaint(self.window, &paint);
        }
    }

    fn render_client(&mut self) -> bool {
        let mut rectangle = RECT::default();
        if unsafe { GetClientRect(self.window, &mut rectangle) } == 0 {
            return false;
        }
        self.render_surface(rectangle.right, rectangle.bottom)
    }

    fn render_surface(&mut self, width: i32, height: i32) -> bool {
        if width <= 0 || height <= 0 {
            return false;
        }
        if self
            .surface
            .as_ref()
            .is_none_or(|surface| surface.width != width || surface.height != height)
        {
            self.surface = Surface::new(width, height).ok();
            self.dirty = true;
        }
        let Some(surface) = &self.surface else {
            return false;
        };
        if self.dirty || self.meter_dirty {
            let Ok(mut canvas) = Canvas::open(surface.dc, self.scale) else {
                return false;
            };
            canvas.translate_x(self.popup_left() as f32 / self.scale);
            if self.dirty {
                self.hits = ui::render(&mut canvas, &self.view(true));
            } else {
                ui::redraw_meter(&mut canvas, &self.view(true));
            }
            self.renders += 1;
            self.dirty = false;
            self.meter_dirty = false;
        }
        true
    }

    fn set_cloak(&self, cloaked: bool) -> bool {
        let cloaked = i32::from(cloaked);
        unsafe {
            DwmSetWindowAttribute(
                self.window,
                DWMWA_CLOAK as u32,
                (&raw const cloaked).cast(),
                size_of_val(&cloaked) as u32,
            ) >= 0
        }
    }

    fn present_opening_cover(
        &mut self,
        prepared: bool,
        background: COLORREF,
    ) -> (bool, Option<bool>) {
        unsafe {
            let dc = GetDC(self.window);
            if dc.is_null() {
                return (false, None);
            }
            let presented = if prepared {
                self.surface
                    .as_ref()
                    .is_some_and(|surface| surface.blit(dc))
            } else {
                let mut area = RECT::default();
                GetClientRect(self.window, &mut area);
                let brush = CreateSolidBrush(background);
                let painted = !brush.is_null() && FillRect(dc, &area, brush) != 0;
                if !brush.is_null() {
                    DeleteObject(brush);
                }
                painted
            };
            GdiFlush();
            let client_covered = self.report.is_some().then(|| {
                let mut area = RECT::default();
                let left = self.popup_left();
                GetClientRect(self.window, &mut area) != 0
                    && GetPixel(dc, left + (area.right - left) / 4, area.bottom / 4) == background
                    && GetPixel(dc, left + (area.right - left) * 3 / 4, area.bottom / 2)
                        == background
            });
            ReleaseDC(self.window, dc);
            if presented {
                self.presents += 1;
            }
            (presented, client_covered)
        }
    }

    fn view(&self, native_search: bool) -> View<'_> {
        View {
            page: self.page,
            width: (self.popup_extent.0 - self.popup_left()) as f32 / self.scale,
            station_list_ready: self.station_list_ready(),
            prefs: &self.prefs,
            catalog: &self.catalog,
            current: self.current,
            status: &self.status,
            list: &self.list,
            top: self.top,
            query: &self.query,
            native_search: native_search && self.station_list_ready() && self.intro.is_none(),
            group: if self.page == Page::Stations {
                self.manage_group
            } else {
                &self.prefs.group
            },
            hot: self.hot,
            focus: self.focus,
            spectrum: self.meter.spectrum(),
            waveform: self.meter.waveform(),
            update: &self.update,
            intro: self.intro.as_ref().map(Animation::frame),
            save_failed: self.save_failed,
            startup_failed: self.startup_failed,
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        if self.visible {
            unsafe {
                InvalidateRect(self.window, null(), 0);
            }
        }
    }

    fn place(&mut self) {
        unsafe {
            let rect = self.tray.rect();
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor);
            let monitor = rect.map_or_else(
                || MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST),
                |rect| MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST),
            );
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return;
            }
            let work = info.rcWork;
            let mut dpi_x = 96_u32;
            let mut dpi_y = 96_u32;
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) != 0 {
                dpi_x = GetDpiForWindow(self.window).max(96);
            }
            let width = work.right - work.left;
            let height = work.bottom - work.top;
            let scale = (dpi_x as f32 / 96.)
                .min((width - 20) as f32 / WIDTH as f32)
                .min((height - 20) as f32 / HEIGHT as f32)
                .max(0.55);
            if (self.scale - scale).abs() > 0.002 {
                self.scale = scale;
                self.update_edit();
                self.surface = None;
            }
            let width = self.pixel(WIDTH as f32).min(width);
            let height = self.pixel(HEIGHT as f32).min(height);
            let margin = self.pixel(10.);
            if SetWindowPos(
                self.window,
                HWND_TOPMOST,
                work.right - width - margin,
                work.bottom - height - margin,
                width,
                height,
                SWP_NOACTIVATE,
            ) != 0
            {
                self.popup_extent = (width, height);
                self.apply_popup_region();
            }
        }
    }

    fn popup_width(&self) -> f32 {
        if self.page == Page::Player {
            self.player_width
        } else {
            WIDTH as f32
        }
    }

    fn popup_left(&self) -> i32 {
        (self.popup_extent.0 - self.pixel(self.popup_width())).max(0)
    }

    fn apply_popup_region(&mut self) -> bool {
        let (width, height) = self.popup_extent;
        let left = self.popup_left();
        let redraw = i32::from(self.visible);
        unsafe {
            if left == 0 {
                if self.popup_clipped {
                    if SetWindowRgn(self.window, null_mut(), redraw) == 0 {
                        return false;
                    }
                    self.set_clipped_frame(false);
                    self.popup_clipped = false;
                }
                return true;
            }
            let diameter = self.pixel(16.).max(2);
            let region = CreateRoundRectRgn(left, 0, width + 1, height + 1, diameter, diameter);
            if region.is_null() {
                return false;
            }
            if !self.popup_clipped {
                self.set_clipped_frame(true);
            }
            if SetWindowRgn(self.window, region, redraw) != 0 {
                self.popup_clipped = true;
                return true;
            }
            DeleteObject(region);
            if !self.popup_clipped {
                self.set_clipped_frame(false);
            }
            false
        }
    }

    fn set_clipped_frame(&mut self, clipped: bool) {
        let (policy, border) = if clipped {
            (DWMNCRP_DISABLED, DWMWA_COLOR_NONE)
        } else {
            (DWMNCRP_USEWINDOWSTYLE, DWMWA_COLOR_DEFAULT)
        };
        let (policy_result, border_result) = unsafe {
            // DWM can draw the frame around the whole HWND, outside its region.
            let policy = DwmSetWindowAttribute(
                self.window,
                DWMWA_NCRENDERING_POLICY as u32,
                (&raw const policy).cast(),
                size_of_val(&policy) as u32,
            );
            let border = DwmSetWindowAttribute(
                self.window,
                DWMWA_BORDER_COLOR as u32,
                (&raw const border).cast(),
                size_of_val(&border) as u32,
            );
            (policy, border)
        };
        if let Some(report) = self.report.as_mut().filter(|report| report.measure_layout) {
            report.layout_frame_updates.push(serde_json::json!({
                "clipped": clipped, "policy_result": policy_result, "border_result": border_result,
            }));
        }
    }

    fn present_panel_resize(&mut self) {
        // Keep the native window and backing bitmap at their full size so DWM
        // never has to stretch the toolbar while changing the visible width.
        self.dirty = true;
        let (width, height) = self.popup_extent;
        let prepared = self.render_surface(width, height);
        if self.apply_popup_region() && prepared {
            unsafe {
                let dc = GetDC(self.window);
                if !dc.is_null() {
                    let surface = self.surface.as_ref().unwrap();
                    let presented = surface.blit(dc);
                    GdiFlush();
                    let toolbar = RECT {
                        left: width - self.pixel(212.),
                        right: width - self.pixel(18.),
                        top: self.pixel(14.),
                        bottom: self.pixel(61.),
                    };
                    if let Some(report) =
                        self.report.as_mut().filter(|report| report.measure_layout)
                    {
                        report
                            .layout_frames
                            .push(report.start.elapsed().as_secs_f64() * 1000.);
                        report.layout_toolbar.push(
                            presented
                                .then(|| surface.matching_client_pixels(dc, toolbar))
                                .flatten(),
                        );
                    }
                    ReleaseDC(self.window, dc);
                    if presented {
                        self.presents += 1;
                        ValidateRect(self.window, null());
                        return;
                    }
                }
            }
        }
        self.mark_dirty();
    }

    fn show(&mut self, intro: bool) {
        if self.visible {
            if self.report.is_none() {
                unsafe {
                    SetForegroundWindow(self.window);
                }
            }
            return;
        }
        self.place();
        let play_intro = intro
            && self.page == Page::Player
            && self.prefs.show_intro
            && (self.report.is_some() || client_animations_enabled())
            && self.set_cloak(true);
        let background = color_ref(Palette::for_theme(&self.prefs.theme).window);
        self.dirty = true;
        if play_intro {
            self.intro = Some(Animation::new(Instant::now()));
            if let Some(report) = &mut self.report {
                report.intro_start = Some((cpu_milliseconds(), self.renders));
            }
        }
        let prepared = play_intro && self.render_client();
        // A hidden tray window can still have its previous pixels in the compositor.
        // Keep it cloaked until the covered frame has reached the window DC.
        self.visible = true;
        self.show_edit();
        unsafe {
            ShowWindow(
                self.window,
                if play_intro || self.report.is_some() {
                    SW_SHOWNOACTIVATE
                } else {
                    SW_SHOW
                },
            );
        }
        if play_intro {
            let (presented, client_covered) = self.present_opening_cover(prepared, background);
            let compositor_ready = presented && unsafe { DwmFlush() } >= 0;
            let uncloaked = self.set_cloak(false);
            if let Some(report) = &mut self.report {
                let covered = prepared
                    && self.surface.as_ref().is_some_and(|surface| {
                        [(0.12, 0.12), (0.84, 0.15), (0.50, 0.51), (0.80, 0.86)]
                            .into_iter()
                            .all(|(x, y)| unsafe {
                                GetPixel(
                                    surface.dc,
                                    (surface.width as f32 * x) as i32,
                                    (surface.height as f32 * y) as i32,
                                ) == background
                            })
                    });
                report.opening_frames.push(serde_json::json!({
                    "covered": covered, "client_covered": client_covered, "presented": presented,
                    "compositor_ready": compositor_ready, "uncloaked": uncloaked,
                    "edit_visible": unsafe { IsWindowVisible(self.edit) } != 0,
                }));
            }
            self.intro = Some(Animation::new(Instant::now()));
            if !presented {
                self.mark_dirty();
            }
            if unsafe { SetTimer(self.window, INTRO_TIMER, FRAME_MS, None) } == 0 {
                self.end_intro();
            }
        }
        if self.report.is_none() {
            unsafe {
                SetForegroundWindow(self.window);
                SetFocus(self.window);
            }
        }
        self.meter_schedule();
        if !prepared {
            self.mark_dirty();
        }
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        if !self.tray.registered && self.report.is_none() {
            self.quit();
            return;
        }
        self.visible = false;
        self.reset_panel_resize();
        if self.intro.take().is_some() {
            if let Some(report) = &mut self.report {
                report.intro_start = None;
            }
        }
        self.hot = None;
        self.focus = None;
        self.drag = None;
        self.engine.watch(false);
        unsafe {
            KillTimer(self.window, METER_TIMER);
            KillTimer(self.window, INTRO_TIMER);
            ShowWindow(self.window, SW_HIDE);
        }
    }

    fn show_edit(&self) {
        unsafe {
            ShowWindow(
                self.edit,
                if self.visible && self.station_list_ready() && self.intro.is_none() {
                    SW_SHOWNOACTIVATE
                } else {
                    SW_HIDE
                },
            );
        }
    }

    fn update_edit(&mut self) {
        let palette = Palette::for_theme(&self.prefs.theme);
        let family = wide("Segoe UI");
        let text = wide(label("Search by name or frequency", &self.prefs.language));
        unsafe {
            if !self.edit_brush.is_null() {
                DeleteObject(self.edit_brush);
            }
            self.edit_brush = CreateSolidBrush(color_ref(palette.panel));
            let font = CreateFontW(
                -self.pixel(15.),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                0,
                family.as_ptr(),
            );
            SendMessageW(self.edit, WM_SETFONT, font as usize, 1);
            if !self.edit_font.is_null() {
                DeleteObject(self.edit_font);
            }
            self.edit_font = font;
            SendMessageW(self.edit, EM_SETCUEBANNER, 0, text.as_ptr() as isize);
            SendMessageW(self.edit, EM_LIMITTEXT, 220, 0);
            MoveWindow(
                self.edit,
                self.pixel(EDIT.x),
                self.pixel(EDIT.y + 2.),
                self.pixel(EDIT.w - 17.),
                self.pixel(EDIT.h),
                0,
            );
        }
    }

    fn pixel(&self, logical: f32) -> i32 {
        (logical * self.scale).round() as i32
    }

    fn logical(&self, value: LPARAM) -> (f32, f32) {
        (
            ((value as u16 as i16) as i32 - self.popup_left()) as f32 / self.scale,
            ((value >> 16) as u16 as i16) as f32 / self.scale,
        )
    }

    fn at(&self, x: f32, y: f32) -> Option<Action> {
        self.hits
            .iter()
            .rev()
            .find(|hit| hit.area.contains(x, y))
            .map(|hit| hit.action)
    }

    fn set_page(&mut self, page: Page) {
        if page == self.page {
            return;
        }
        self.page = page;
        self.reset_panel_resize();
        if self.visible {
            self.apply_popup_region();
        }
        if self.intro.is_some() {
            self.end_intro();
        }
        self.hot = None;
        self.focus = None;
        self.top = 0;
        self.relist(page == Page::Player);
        self.show_edit();
        self.meter_schedule();
        self.mark_dirty();
    }

    fn station_list_ready(&self) -> bool {
        self.page == Page::Player
            && !self.prefs.station_list_collapsed
            && self.panel_resize.is_none()
    }

    fn can_scroll_stations(&self) -> bool {
        self.page == Page::Stations || self.station_list_ready()
    }

    fn change_station_list(&mut self, collapsed: bool) {
        if self.prefs.station_list_collapsed == collapsed {
            if self.panel_resize.is_none() && self.search_after_resize {
                self.focus_search();
            }
            return;
        }
        self.prefs.station_list_collapsed = collapsed;
        self.save_later();
        self.hot = None;
        if !matches!(self.focus, Some(Action::ToggleStationList)) {
            self.focus = None;
        }
        self.hits
            .retain(|hit| hit.area.x < LIST.x || hit.area.y < 60.);
        if collapsed {
            self.search_after_resize = false;
            if unsafe { GetFocus() == self.edit } {
                unsafe { SetFocus(self.window) };
            }
        }
        let target = player_width(collapsed);
        let animate = self.visible
            && self.page == Page::Player
            && (self.report.is_some() || client_animations_enabled())
            && self.pixel(self.player_width) != self.pixel(target);
        if animate && unsafe { SetTimer(self.window, PANEL_TIMER, FRAME_MS, None) } != 0 {
            self.panel_resize = Some(PanelResize {
                from: self.player_width,
                target,
                started: Instant::now(),
            });
            self.show_edit();
            self.mark_dirty();
        } else {
            self.complete_panel_resize();
        }
    }

    fn reset_panel_resize(&mut self) {
        unsafe { KillTimer(self.window, PANEL_TIMER) };
        self.panel_resize = None;
        self.player_width = player_width(self.prefs.station_list_collapsed);
        self.search_after_resize = false;
    }

    fn complete_panel_resize(&mut self) {
        let focus_search = self.search_after_resize;
        self.reset_panel_resize();
        if self.visible {
            self.present_panel_resize();
            self.show_edit();
            if focus_search {
                self.focus_search();
            }
        } else {
            self.mark_dirty();
        }
    }

    fn focus_search(&mut self) {
        self.search_after_resize = false;
        self.focus = Some(Action::Search);
        unsafe {
            SetFocus(self.edit);
            SendMessageW(self.edit, EM_SETSEL, 0, -1);
        }
    }

    fn relist(&mut self, reset: bool) {
        let manage = self.page == Page::Stations;
        let (group, query) = if manage {
            (self.manage_group, "")
        } else {
            (self.prefs.group.as_str(), self.query.as_str())
        };
        self.list = self.catalog.list(&self.prefs, group, query, manage);
        if reset {
            self.top = 0;
        }
        self.top = self.top.min(self.list.len().saturating_sub(6));
    }

    fn reveal_current(&mut self) {
        if let Some(at) = self
            .current
            .and_then(|current| self.list.iter().position(|station| *station == current))
        {
            if at < self.top {
                self.top = at;
            }
            if at >= self.top + 6 {
                self.top = at.saturating_sub(3).min(self.list.len().saturating_sub(6));
            }
        }
    }

    fn scroll(&mut self, delta: i32) {
        let top = self
            .top
            .saturating_add_signed(delta as isize)
            .min(self.list.len().saturating_sub(6));
        if self.top != top {
            self.top = top;
            self.hot = None;
            if matches!(
                self.focus,
                Some(
                    Action::Tune(_) | Action::Star(_) | Action::Visibility(_) | Action::Move(_, _)
                )
            ) {
                self.focus = None;
            }
            self.mark_dirty();
        }
    }

    fn clear_search(&mut self) {
        if self.query.is_empty() {
            return;
        }
        self.query.clear();
        unsafe {
            SetWindowTextW(self.edit, wide("").as_ptr());
        }
        self.relist(true);
    }

    fn perform(&mut self, action: Action, x: f32, y: f32) {
        match action {
            Action::Close => self.hide(),
            Action::ToggleStationList => {
                self.change_station_list(!self.prefs.station_list_collapsed)
            }
            Action::Page(page) => self.set_page(page),
            Action::Group(group) => {
                if self.page == Page::Stations {
                    self.manage_group = group;
                } else {
                    self.prefs.group = group.into();
                    self.clear_search();
                    self.save_later();
                }
                self.relist(true);
                if self.page == Page::Player {
                    self.reveal_current();
                }
            }
            Action::GroupVisible(group) => {
                if !self.prefs.hidden_groups.remove(group)
                    && self.prefs.hidden_groups.len() < GROUPS.len() - 1
                {
                    self.prefs.hidden_groups.insert(group.into());
                    if self.prefs.group == group {
                        self.prefs.group = GROUPS
                            .iter()
                            .find(|group| !self.prefs.hidden_groups.contains(**group))
                            .unwrap()
                            .to_string();
                    }
                }
                self.save_later();
            }
            Action::Search => unsafe {
                SetFocus(self.edit);
            },
            Action::ClearSearch => {
                self.clear_search();
                unsafe {
                    SetFocus(self.edit);
                }
            }
            Action::Tune(station) => self.tune(station),
            Action::Star(station) => {
                let id = &self.catalog.0[station].id;
                if !self.prefs.favorites.remove(id) {
                    self.prefs.favorites.insert(id.clone());
                }
                self.relist(false);
                self.save_later();
            }
            Action::Play => self.toggle(),
            Action::Step(direction) => self.navigate(direction),
            Action::Volume => self.set_volume(Action::volume(x)),
            Action::Band(index) => self.set_band(index, Action::gain(y)),
            Action::ResetBands => {
                self.prefs.equalizer = [0; 10];
                self.mixer_changed();
            }
            Action::Language(language) => {
                self.prefs.language = language.into();
                self.update_edit();
                self.save_later();
            }
            Action::Theme(theme) => {
                self.prefs.theme = theme.into();
                self.update_edit();
                self.save_later();
            }
            Action::Meter(mode) => {
                self.prefs.visualization = mode.into();
                self.meter.reset();
                self.meter_schedule();
                self.save_later();
            }
            Action::Toggle(flag) => {
                match flag {
                    Toggle::Smooth => self.prefs.smooth_visualization ^= true,
                    Toggle::Intro => self.prefs.show_intro ^= true,
                    Toggle::WindowsStartup => {
                        let enabled = !self.prefs.start_with_windows;
                        self.startup_failed = set_windows_startup(enabled).is_err();
                        if !self.startup_failed {
                            self.prefs.start_with_windows = enabled;
                            self.save();
                            if self.save_failed {
                                self.prefs.start_with_windows = !enabled;
                                self.startup_failed = set_windows_startup(!enabled).is_err();
                            }
                        }
                        self.mark_dirty();
                        return;
                    }
                    Toggle::Autoplay => self.prefs.play_on_startup ^= true,
                    Toggle::StartupUpdates => self.prefs.check_updates_on_startup ^= true,
                    Toggle::Unavailable => self.prefs.hide_unavailable ^= true,
                    Toggle::Equalizer => {
                        self.prefs.equalizer_enabled ^= true;
                        self.engine.mix(&self.prefs);
                    }
                }
                self.save_later();
            }
            Action::Visibility(station) => {
                let id = &self.catalog.0[station].id;
                if !self.prefs.hidden_stations.remove(id) {
                    self.prefs.hidden_stations.insert(id.clone());
                }
                self.save_later();
            }
            Action::Move(station, direction) => {
                if let Some(index) = self.list.iter().position(|candidate| *candidate == station) {
                    let next = index
                        .checked_add_signed(direction as isize)
                        .filter(|next| *next < self.list.len());
                    if let Some(next) = next {
                        self.list.swap(index, next);
                        self.prefs.order.insert(
                            self.manage_group.to_owned(),
                            self.list
                                .iter()
                                .map(|index| self.catalog.0[*index].id.clone())
                                .collect(),
                        );
                        if next < self.top {
                            self.top = next;
                        }
                        if next >= self.top + 6 {
                            self.top = next.saturating_sub(5);
                        }
                        self.save_later();
                    }
                }
            }
            Action::CheckUpdate => self.check_update(true),
            Action::InstallUpdate => self.install_update(),
            Action::Scroll(delta) => self.scroll(delta),
        }
        self.mark_dirty();
    }

    fn key(&mut self, key: u16, from_edit: bool) -> bool {
        let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
        let control = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
        if key == b'F' as u16 && control {
            self.end_intro();
            self.set_page(Page::Player);
            self.search_after_resize = true;
            self.change_station_list(false);
            return true;
        }
        match key {
            VK_ESCAPE => {
                if self.intro.is_some() {
                    self.dismiss_intro();
                } else if self.page != Page::Player {
                    self.set_page(self.page.back());
                } else if from_edit && !self.query.is_empty() {
                    self.clear_search();
                    self.mark_dirty();
                } else {
                    self.hide();
                }
            }
            VK_TAB => {
                if self.intro.is_some() {
                    self.end_intro();
                    return true;
                }
                if self.hits.is_empty() {
                    return true;
                }
                let previous = if from_edit {
                    Some(Action::Search)
                } else {
                    self.focus
                };
                let index = self
                    .hits
                    .iter()
                    .position(|hit| Some(hit.action) == previous);
                let direction = if shift { -1 } else { 1 };
                let next = index.map_or(if shift { self.hits.len() - 1 } else { 0 }, |index| {
                    (index as isize + direction).rem_euclid(self.hits.len() as isize) as usize
                });
                let action = self.hits[next].action;
                self.focus = Some(action);
                unsafe {
                    SetFocus(if action == Action::Search {
                        self.edit
                    } else {
                        self.window
                    });
                }
                self.mark_dirty();
            }
            VK_RETURN if from_edit => {
                if let Some(index) = self.list.first() {
                    self.tune(*index);
                }
                unsafe {
                    SetFocus(self.window);
                }
            }
            VK_RETURN | VK_SPACE if !from_edit => {
                if self.intro.is_some() {
                    self.dismiss_intro();
                } else if let Some(action) = self.focus.filter(|action| !action.is_slider()) {
                    self.perform(action, 0., 0.);
                } else if key == VK_SPACE && self.focus.is_none() {
                    self.toggle();
                }
            }
            VK_LEFT | VK_RIGHT | VK_DOWN | VK_UP if !from_edit => {
                let forward = key == VK_RIGHT || key == VK_UP;
                let delta = if forward { 1 } else { -1 };
                match self.focus {
                    Some(Action::Volume) => {
                        self.set_volume((self.prefs.volume as i32 + delta * 4).clamp(0, 100) as u8)
                    }
                    Some(Action::Band(band)) => self.set_band(
                        band,
                        (self.prefs.equalizer[band] + delta as i8).clamp(-12, 12),
                    ),
                    _ if self.can_scroll_stations() => {
                        self.scroll(if key == VK_UP || key == VK_LEFT {
                            -1
                        } else {
                            1
                        })
                    }
                    _ => return false,
                }
                self.mark_dirty();
            }
            VK_MEDIA_PLAY_PAUSE => self.toggle(),
            VK_MEDIA_NEXT_TRACK => self.navigate(1),
            VK_MEDIA_PREV_TRACK => self.navigate(-1),
            VK_PRIOR | VK_NEXT if !from_edit && self.can_scroll_stations() => {
                self.scroll(if key == VK_PRIOR { -6 } else { 6 })
            }
            _ => return false,
        }
        true
    }

    fn set_volume(&mut self, volume: u8) {
        if self.prefs.volume != volume {
            self.prefs.volume = volume;
            self.mixer_changed();
        }
    }

    fn set_band(&mut self, band: usize, gain: i8) {
        if self.prefs.equalizer[band] != gain {
            self.prefs.equalizer[band] = gain;
            self.mixer_changed();
        }
    }

    fn mixer_changed(&mut self) {
        self.engine.mix(&self.prefs);
        self.save_later();
        self.mark_dirty();
    }

    fn available_stations(&self) -> Vec<usize> {
        self.catalog
            .list(&self.prefs, &self.prefs.group, &self.query, false)
    }

    fn navigate(&mut self, direction: i32) {
        let list = self.available_stations();
        if list.is_empty() {
            return;
        }
        let current = self
            .current
            .and_then(|station| list.iter().position(|item| *item == station))
            .map(|current| current as i32)
            .unwrap_or(if direction > 0 { -1 } else { 0 });
        self.tune(list[(current + direction).rem_euclid(list.len() as i32) as usize]);
    }

    fn tune(&mut self, index: usize) {
        if self.current == Some(index) && self.is_active() {
            return;
        }
        let mut station = self.catalog.0[index].clone();
        if let Some(report) = &self.report {
            let Some((source, _)) = &report.meter_source else {
                return;
            };
            station.streams = vec![source.clone()];
            station.track_source_id = None;
        }
        self.current = Some(index);
        self.prefs.station_id = Some(station.id.clone());
        self.revision = self.engine.play(station);
        self.status = Status::Connecting;
        self.meter.reset();
        self.save_later();
        if self.page == Page::Player {
            self.reveal_current();
        }
        self.update_system_media();
        self.meter_schedule();
        self.mark_dirty();
    }

    fn is_active(&self) -> bool {
        matches!(self.status, Status::Playing(_) | Status::Connecting)
    }

    fn stop(&mut self) {
        self.revision = self.engine.stop();
        self.status = Status::Ready;
        self.meter.reset();
        self.update_system_media();
        self.meter_schedule();
        self.mark_dirty();
    }

    fn toggle(&mut self) {
        if self.is_active() {
            self.stop();
        } else if let Some(index) = self
            .current
            .or_else(|| self.available_stations().first().copied())
        {
            self.tune(index);
        }
    }

    fn update_system_media(&mut self) {
        let station = self.current.map(|current| &self.catalog.0[current]);
        if let Some(media) = &self.media {
            let _ = media.update(station, &self.status);
        }
        let tip = station.map_or_else(
            || "Desktop Radio".into(),
            |station| format!("Desktop Radio · {}", station.name),
        );
        if tip != self.tray_tip {
            self.tray.tip(&tip);
            self.tray_tip = tip;
        }
    }

    fn receive(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Audio(revision, status)
                    if revision == self.revision && self.status != status =>
                {
                    self.status = status;
                    if let Some(index) = self.current {
                        let id = self.catalog.0[index].id.clone();
                        let change = match self.status {
                            Status::Unavailable => {
                                self.prefs
                                    .unavailable
                                    .insert(id, UnavailableAt::Epoch(now()));
                                true
                            }
                            Status::Playing(_) => self.prefs.unavailable.remove(&id).is_some(),
                            _ => false,
                        };
                        if change {
                            self.relist(false);
                            self.save_later();
                        }
                    }
                    if !self.is_active() {
                        self.meter.reset();
                    }
                    self.update_system_media();
                    self.meter_schedule();
                    self.mark_dirty();
                }
                Event::Media(key) => match key {
                    MediaKey::Play if !self.is_active() => self.toggle(),
                    MediaKey::Pause if self.is_active() => self.stop(),
                    MediaKey::Previous => self.navigate(-1),
                    MediaKey::Next => self.navigate(1),
                    _ => (),
                },
                Event::Checked(manual, result) => {
                    self.checking = false;
                    self.release = None;
                    self.update = match result {
                        Ok(Check::New(release)) => {
                            let version = release.version.to_string();
                            self.release = Some(release);
                            if !manual {
                                self.tray.notify(
                                    label("Update available", &self.prefs.language),
                                    &format!("Desktop Radio v{version}"),
                                );
                            }
                            UpdateDisplay::Available(version)
                        }
                        Ok(Check::Current) if manual => UpdateDisplay::Current,
                        Ok(Check::NoRelease) if manual => UpdateDisplay::Unpublished,
                        Err(_) if manual => UpdateDisplay::Failed,
                        _ => UpdateDisplay::Blank,
                    };
                    if self.page == Page::Settings {
                        self.mark_dirty();
                    }
                }
                Event::Progress(value) => {
                    self.update = UpdateDisplay::Downloading(value);
                    if self.page == Page::Settings {
                        self.mark_dirty();
                    }
                }
                Event::Downloaded(result) => {
                    self.update = if result.is_ok() {
                        UpdateDisplay::Restarting
                    } else {
                        UpdateDisplay::InstallFailed
                    };
                    self.mark_dirty();
                    if result.is_ok() {
                        self.quit();
                    }
                }
                _ => (),
            }
        }
    }

    fn check_update(&mut self, manual: bool) {
        if self.checking
            || matches!(
                self.update,
                UpdateDisplay::Downloading(_) | UpdateDisplay::Restarting
            )
        {
            return;
        }
        self.checking = true;
        if manual {
            self.update = UpdateDisplay::Checking;
        }
        let sender = self.sender.clone();
        let window = self.window as usize;
        std::thread::spawn(move || {
            publish(
                &sender,
                window,
                Event::Checked(
                    manual,
                    updates::check().map_err(|error| format!("{error:#}")),
                ),
            );
        });
    }

    fn install_update(&mut self) {
        let Some(release) = self.release.clone() else {
            return;
        };
        if matches!(
            self.update,
            UpdateDisplay::Downloading(_) | UpdateDisplay::Restarting
        ) {
            return;
        }
        self.update = UpdateDisplay::Downloading(0);
        let sender = self.sender.clone();
        let window = self.window as usize;
        std::thread::spawn(move || {
            let result = updates::download(&release, |progress| {
                publish(&sender, window, Event::Progress(progress))
            });
            publish(
                &sender,
                window,
                Event::Downloaded(result.map_err(|error| format!("{error:#}"))),
            );
        });
    }

    fn meter_schedule(&self) {
        let run = self.visible
            && self.page == Page::Player
            && self.intro.is_none()
            && self.prefs.visualization != "off"
            && matches!(self.status, Status::Playing(_));
        self.engine.watch(run);
        unsafe {
            if run {
                SetTimer(self.window, METER_TIMER, METER_FRAME_MS, None);
            } else {
                KillTimer(self.window, METER_TIMER);
            }
        }
    }

    fn timer(&mut self, id: usize) {
        match id {
            METER_TIMER
                if self.visible
                    && self.page == Page::Player
                    && self.intro.is_none()
                    && self.prefs.visualization != "off"
                    && matches!(self.status, Status::Playing(_)) =>
            {
                let signal = self.engine.signal();
                let waveform = self.prefs.visualization == "waveform";
                if self
                    .meter
                    .update(signal, self.prefs.smooth_visualization, waveform)
                {
                    self.meter_dirty = true;
                    let area = ui::METER_REDRAW;
                    let scale = self.scale;
                    let left = self.popup_left();
                    let bounds = RECT {
                        left: left + (area.x * scale).floor() as i32,
                        top: (area.y * scale).floor() as i32,
                        right: left + (area.right() * scale).ceil() as i32,
                        bottom: (area.bottom() * scale).ceil() as i32,
                    };
                    unsafe {
                        InvalidateRect(self.window, &bounds, 0);
                    }
                }
            }
            INTRO_TIMER => {
                if !self.visible
                    || self
                        .intro
                        .as_mut()
                        .is_none_or(|intro| intro.advance(Instant::now()))
                {
                    self.end_intro();
                } else {
                    self.mark_dirty();
                }
            }
            PANEL_TIMER => {
                if let Some(resize) = &self.panel_resize {
                    let (width, finished) = resize.frame(Instant::now());
                    if finished {
                        self.complete_panel_resize();
                    } else if self.pixel(width) != self.pixel(self.player_width) {
                        self.player_width = width;
                        self.present_panel_resize();
                    }
                }
            }
            DIAG_TIMER => self.diagnose(),
            SAVE_TIMER => self.save(),
            _ => (),
        }
    }

    fn dismiss_intro(&mut self) {
        if let Some(intro) = &mut self.intro {
            intro.dismiss(Instant::now());
        }
    }

    fn end_intro(&mut self) {
        unsafe {
            KillTimer(self.window, INTRO_TIMER);
        }
        let Some(intro) = self.intro.take() else {
            return;
        };
        if let Some(report) = &mut self.report {
            if let Some((cpu, renders)) = report.intro_start.take() {
                report.intros.push(serde_json::json!({
                    "wall_ms": intro.elapsed().as_millis(), "cpu_ms": cpu_milliseconds() - cpu,
                    "render": self.renders - renders,
                }));
            }
        }
        self.show_edit();
        self.meter_schedule();
        self.mark_dirty();
    }

    fn save_later(&self) {
        if self.report.is_none() {
            unsafe {
                SetTimer(self.window, SAVE_TIMER, 330, None);
            }
        }
    }

    fn save(&mut self) {
        unsafe {
            KillTimer(self.window, SAVE_TIMER);
        }
        if self.report.is_some() {
            return;
        }
        let failed = self.prefs.save(&self.settings).is_err();
        if self.save_failed != failed {
            self.save_failed = failed;
            self.mark_dirty();
        }
    }

    fn tray_event(&mut self, code: u32) {
        match code {
            NIN_SELECT | NIN_KEYSELECT => {
                if self.visible {
                    self.hide();
                } else {
                    self.show(true);
                }
            }
            WM_LBUTTONUP if !self.tray.version_4 => {
                if self.visible {
                    self.hide();
                } else {
                    self.show(true);
                }
            }
            WM_CONTEXTMENU => self.menu(),
            WM_RBUTTONUP if !self.tray.version_4 => self.menu(),
            NIN_BALLOONUSERCLICK => {
                self.set_page(Page::Settings);
                self.show(false);
            }
            _ => (),
        }
    }

    fn menu(&mut self) {
        let items = [
            (1, "Open radio"),
            (2, "Play / Pause"),
            (3, "Previous station"),
            (4, "Next station"),
            (0, ""),
            (5, "Settings"),
            (6, "Quit"),
        ];
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return;
        }
        let mut point = POINT::default();
        unsafe {
            for (id, title) in items {
                AppendMenuW(
                    menu,
                    if id == 0 { MF_SEPARATOR } else { MF_STRING },
                    id,
                    wide(label(title, &self.prefs.language)).as_ptr(),
                );
            }
            GetCursorPos(&mut point);
            SetForegroundWindow(self.window);
            let action = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                self.window,
                null(),
            );
            DestroyMenu(menu);
            PostMessageW(self.window, WM_NULL, 0, 0);
            if action != 6 {
                self.receive();
            }
            match action {
                1 => self.show(true),
                2 => self.toggle(),
                3 => self.navigate(-1),
                4 => self.navigate(1),
                5 => {
                    self.set_page(Page::Settings);
                    self.show(false);
                }
                6 => self.quit(),
                _ => (),
            }
        }
    }

    fn quit(&mut self) {
        self.engine.stop();
        self.save();
        self.tray.remove();
        unsafe {
            PostQuitMessage(0);
        }
    }

    fn diagnose(&mut self) {
        if self
            .report
            .as_ref()
            .is_some_and(|report| report.measure_layout)
        {
            self.diagnose_layout();
            return;
        }
        if self
            .report
            .as_ref()
            .is_some_and(|report| report.meter_source.is_some())
        {
            self.diagnose_meter();
            return;
        }
        let Some(diagnostic) = &mut self.report else {
            return;
        };
        let phase = diagnostic.phase;
        let visible = unsafe { IsWindowVisible(self.window) } != 0;
        let state = [
            "visible start",
            "visible end",
            "hidden start",
            "hidden end",
            "dismiss requested",
            "reopened",
            "hiding during intro",
            "hidden after interruption",
            "hidden settled",
        ][phase];
        diagnostic.samples.push(serde_json::json!({
            "state": state,
            "ms": diagnostic.start.elapsed().as_millis(), "cpu_ms": cpu_milliseconds(),
            "render": self.renders, "present": self.presents, "visible": visible, "intro_active": self.intro.is_some(),
        }));
        diagnostic.phase += 1;
        match phase {
            0 => unsafe {
                SetTimer(self.window, DIAG_TIMER, 7000, None);
            },
            1 => {
                self.tray_event(NIN_SELECT);
                unsafe {
                    SetTimer(self.window, DIAG_TIMER, 500, None);
                }
            }
            2 => unsafe {
                SetTimer(self.window, DIAG_TIMER, 7000, None);
            },
            3 => {
                self.tray_event(NIN_SELECT);
                unsafe {
                    SetTimer(self.window, DIAG_TIMER, 510, None);
                }
            }
            4 => {
                self.dismiss_intro();
                unsafe {
                    SetTimer(self.window, DIAG_TIMER, 600, None);
                }
            }
            5 => {
                self.tray_event(NIN_SELECT);
                self.tray_event(NIN_SELECT);
                unsafe {
                    SetTimer(self.window, DIAG_TIMER, 200, None);
                }
            }
            6 => {
                self.tray_event(NIN_SELECT);
                unsafe {
                    SetTimer(self.window, DIAG_TIMER, 400, None);
                }
            }
            7 => unsafe {
                SetTimer(self.window, DIAG_TIMER, 750, None);
            },
            _ => {
                let diagnostic = self.report.take().unwrap();
                let result = serde_json::json!({"pid": std::process::id(), "backend": "GDI+ into memory DIB; Win32 BitBlt",
                    "native_tray_registered": self.tray.registered, "media_session_attached": self.media.is_some(),
                    "native_icon_loaded": !self.tray.icon.is_null(), "tray_icon_rect": self.tray.rect().is_some(),
                    "samples": diagnostic.samples, "intros": diagnostic.intros, "opening_frames": diagnostic.opening_frames});
                let saved =
                    std::fs::write(diagnostic.path, serde_json::to_vec_pretty(&result).unwrap())
                        .is_ok();
                unsafe {
                    KillTimer(self.window, DIAG_TIMER);
                    PostQuitMessage(if saved { 0 } else { 1 });
                }
                return;
            }
        }
    }

    fn diagnose_meter(&mut self) {
        let diagnostic = self.report.as_mut().unwrap();
        let phase = diagnostic.phase;
        diagnostic.phase += 1;
        diagnostic.samples.push(serde_json::json!({
            "ms": diagnostic.start.elapsed().as_secs_f64() * 1000.,
            "cpu_ms": cpu_milliseconds(), "render": self.renders, "present": self.presents,
            "visible": self.visible, "playing": matches!(self.status, Status::Playing(_)),
            "meter_presents": diagnostic.meter_presents.len(),
        }));
        let interval = match phase {
            0 => {
                self.tune(self.current.unwrap_or_default());
                1500
            }
            1 => {
                let revision = self.revision;
                self.perform(Action::Tune(self.current.unwrap()), 0., 0.);
                if self.revision != revision || !matches!(self.status, Status::Playing(_)) {
                    unsafe { PostQuitMessage(1) };
                    return;
                }
                5200
            }
            2 => {
                self.hide();
                1500
            }
            3 => {
                self.show(false);
                1800
            }
            _ => {
                let diagnostic = self.report.take().unwrap();
                let result = serde_json::json!({
                    "mode": diagnostic.meter_source.as_ref().unwrap().1,
                    "samples": diagnostic.samples, "meter_presents_ms": diagnostic.meter_presents,
                });
                let saved =
                    std::fs::write(diagnostic.path, serde_json::to_vec_pretty(&result).unwrap())
                        .is_ok();
                unsafe {
                    KillTimer(self.window, DIAG_TIMER);
                    PostQuitMessage(if saved { 0 } else { 1 });
                }
                return;
            }
        };
        unsafe {
            SetTimer(self.window, DIAG_TIMER, interval, None);
        }
    }

    fn diagnose_layout(&mut self) {
        let mut rect = RECT::default();
        let native_width = (unsafe { GetWindowRect(self.window, &mut rect) } != 0)
            .then_some(rect.right - rect.left);
        let mut region = RECT::default();
        let has_region = unsafe { GetWindowRgnBox(self.window, &mut region) } != 0;
        let width = if has_region {
            Some(region.right - region.left)
        } else {
            native_width
        };
        let expected_width = self.popup_extent.0 - self.popup_left();
        let mut frame_rendering = 0;
        let frame_rendering = unsafe {
            let frame = DwmGetWindowAttribute(
                self.window,
                DWMWA_NCRENDERING_ENABLED as u32,
                (&raw mut frame_rendering).cast(),
                size_of_val(&frame_rendering) as u32,
            );
            (frame >= 0).then_some(frame_rendering != 0)
        };
        let diagnostic = self.report.as_mut().unwrap();
        let phase = diagnostic.phase;
        diagnostic.phase += 1;
        diagnostic.samples.push(serde_json::json!({
            "ms": diagnostic.start.elapsed().as_secs_f64() * 1000.,
            "cpu_ms": cpu_milliseconds(), "expected_width": expected_width,
            "width": width, "native_width": native_width, "right": rect.right,
            "page": format!("{:?}", self.page),
            "visible": self.visible, "moving": self.panel_resize.is_some(),
            "search_visible": unsafe { IsWindowVisible(self.edit) } != 0,
            "search_hit": self.hits.iter().any(|hit| hit.action == Action::Search),
            "frame_rendering": frame_rendering,
            "render": self.renders, "present": self.presents,
        }));
        let interval = match phase {
            0 | 5 | 7 => {
                self.perform(Action::ToggleStationList, 0., 0.);
                100
            }
            1 | 6 => 260,
            2 => 5200,
            3 => {
                self.perform(Action::Page(Page::Equalizer), 0., 0.);
                160
            }
            4 => {
                self.perform(Action::Page(Page::Player), 0., 0.);
                160
            }
            8 => {
                self.hide();
                130
            }
            9 => 5000,
            10 => {
                self.show(true);
                3100
            }
            _ => {
                let diagnostic = self.report.take().unwrap();
                let result = serde_json::json!({
                    "samples": diagnostic.samples, "present_ms": diagnostic.layout_frames,
                    "opening_frames": diagnostic.opening_frames, "intros": diagnostic.intros,
                    "toolbar_checksums": diagnostic.layout_toolbar,
                    "frame_updates": diagnostic.layout_frame_updates,
                });
                let saved =
                    std::fs::write(diagnostic.path, serde_json::to_vec_pretty(&result).unwrap())
                        .is_ok();
                unsafe {
                    KillTimer(self.window, DIAG_TIMER);
                    PostQuitMessage(if saved { 0 } else { 1 });
                }
                return;
            }
        };
        unsafe {
            SetTimer(self.window, DIAG_TIMER, interval, None);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.tray.remove();
        self.media.take();
        unsafe {
            DestroyWindow(self.window);
            if !self.edit_brush.is_null() {
                DeleteObject(self.edit_brush);
            }
            if !self.edit_font.is_null() {
                DeleteObject(self.edit_font);
            }
        }
    }
}

fn player_width(collapsed: bool) -> f32 {
    (if collapsed { COMPACT_WIDTH } else { WIDTH }) as f32
}

struct PanelResize {
    from: f32,
    target: f32,
    started: Instant,
}

impl PanelResize {
    fn frame(&self, now: Instant) -> (f32, bool) {
        let progress = now.duration_since(self.started).as_secs_f32() * 1000. / PANEL_TRANSITION_MS;
        if progress >= 1. {
            return (self.target, true);
        }
        let eased = progress * progress * (3. - 2. * progress);
        (self.from + (self.target - self.from) * eased, false)
    }
}

unsafe extern "system" fn search_proc(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    parent: usize,
) -> LRESULT {
    if message == WM_KEYDOWN && matches!(w as u16, VK_ESCAPE | VK_TAB | VK_RETURN) {
        PostMessageW(parent as HWND, EDIT_KEY, w, l);
        return 0;
    }
    if message == WM_CHAR && matches!(w, 9 | 13 | 27) {
        return 0;
    }
    DefSubclassProc(window, message, w, l)
}

fn color_ref(color: u32) -> u32 {
    ((color & 255) << 16) | (color & 0x00ff00) | ((color >> 16) & 255)
}

struct Tray {
    window: HWND,
    icon: HICON,
    registered: bool,
    version_4: bool,
    taskbar_created: u32,
}

impl Tray {
    fn new(window: HWND, instance: HINSTANCE) -> Self {
        let mut icon = null_mut();
        unsafe {
            if LoadIconMetric(instance, 1 as _, LIM_SMALL as i32, &mut icon) != 0 {
                icon = LoadImageW(instance, 1 as _, IMAGE_ICON, 16, 16, 0) as HICON;
            }
        }
        let mut tray = Self {
            window,
            icon,
            registered: false,
            version_4: false,
            taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
        };
        tray.add();
        tray
    }

    fn data(&self, flags: u32) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.window,
            uID: 1,
            uFlags: flags,
            uCallbackMessage: TRAY,
            hIcon: self.icon,
            ..Default::default()
        }
    }

    fn add(&mut self) {
        let mut data = self.data(NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP);
        copy_wide(&mut data.szTip, "Desktop Radio");
        self.registered = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
        if self.registered {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            self.version_4 = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } != 0;
        }
    }

    fn tip(&self, text: &str) {
        if !self.registered {
            return;
        }
        let mut data = self.data(NIF_TIP | NIF_SHOWTIP);
        copy_wide(&mut data.szTip, text);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    fn notify(&self, title: &str, text: &str) {
        if !self.registered {
            return;
        }
        let mut data = self.data(NIF_INFO);
        copy_wide(&mut data.szInfo, text);
        copy_wide(&mut data.szInfoTitle, title);
        data.dwInfoFlags = NIIF_INFO;
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    fn rect(&self) -> Option<RECT> {
        let id = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.window,
            uID: 1,
            ..Default::default()
        };
        let mut result = RECT::default();
        (unsafe { Shell_NotifyIconGetRect(&id, &mut result) } == 0).then_some(result)
    }

    fn remove(&mut self) {
        if self.registered {
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &self.data(0));
            }
            self.registered = false;
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        if !self.icon.is_null() {
            unsafe {
                DestroyIcon(self.icon);
            }
        }
    }
}

fn copy_wide(destination: &mut [u16], text: &str) {
    let mut used = 0;
    for character in text.chars() {
        let mut utf16 = [0; 2];
        let units = character.encode_utf16(&mut utf16);
        if used + units.len() >= destination.len() {
            break;
        }
        destination[used..used + units.len()].copy_from_slice(units);
        used += units.len();
    }
    destination[used] = 0;
}

struct Meter {
    fft: Arc<dyn Fft<f32>>,
    window: [f32; METER_SIZE],
    input: [Complex32; METER_SIZE],
    spectrum: [f32; 16],
    waveform: [f32; 100],
    spectrum_target: [f32; 16],
    waveform_target: [f32; 100],
    ready: bool,
    revision: u64,
    updated: Instant,
}

impl Meter {
    fn new() -> Self {
        Self {
            fft: FftPlanner::new().plan_fft_forward(METER_SIZE),
            window: std::array::from_fn(|i| {
                0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (METER_SIZE - 1) as f32).cos()
            }),
            input: [Complex32::new(0., 0.); METER_SIZE],
            spectrum: [0.; 16],
            waveform: [0.; 100],
            spectrum_target: [0.; 16],
            waveform_target: [0.; 100],
            ready: false,
            revision: 0,
            updated: Instant::now(),
        }
    }

    fn reset(&mut self) {
        self.ready = false;
        self.spectrum.fill(0.);
        self.waveform.fill(0.);
        self.spectrum_target.fill(0.);
        self.waveform_target.fill(0.);
    }
    fn spectrum(&self) -> &[f32] {
        if self.ready { &self.spectrum } else { &[] }
    }
    fn waveform(&self) -> &[f32] {
        if self.ready { &self.waveform } else { &[] }
    }

    fn update(&mut self, signal: Signal, smooth: bool, waveform: bool) -> bool {
        let fresh = signal.revision != 0 && signal.revision != self.revision;
        if fresh {
            self.revision = signal.revision;
            if waveform {
                let count = self.waveform_target.len();
                for (index, target) in self.waveform_target.iter_mut().enumerate() {
                    let low = index * METER_SIZE / count;
                    let high = (index + 1) * METER_SIZE / count;
                    *target =
                        signal.values[low..high].iter().sum::<f32>() / (high - low) as f32 * 2.2;
                }
            } else {
                self.analyze_spectrum(&signal);
            }
        }
        if !self.ready && !fresh {
            return false;
        }
        let now = Instant::now();
        let seconds = now.duration_since(self.updated).as_secs_f32();
        self.updated = now;
        let animate = self.ready && smooth;
        let first = !self.ready;
        self.ready = true;
        let changed = if waveform {
            follow_signal(
                &mut self.waveform,
                &self.waveform_target,
                animate,
                seconds,
                58.,
                58.,
            )
        } else {
            follow_signal(
                &mut self.spectrum,
                &self.spectrum_target,
                animate,
                seconds,
                58.,
                8.,
            )
        };
        first || changed
    }

    fn analyze_spectrum(&mut self, signal: &Signal) {
        for (index, buffered) in self.input.iter_mut().enumerate() {
            *buffered = Complex32::new(signal.values[index] * self.window[index], 0.);
        }
        self.fft.process(&mut self.input);
        for (index, target) in self.spectrum_target.iter_mut().enumerate() {
            let bin = |step: usize| {
                (39. * 1.45_f32.powi(step as i32) * METER_SIZE as f32 / signal.rate as f32) as usize
            };
            let start = bin(index).clamp(1, METER_SIZE / 2 - 1);
            let end = bin(index + 1).clamp(start + 1, METER_SIZE / 2);
            let power = self.input[start..end]
                .iter()
                .map(|bin| bin.norm_sqr())
                .fold(0., f32::max);
            let db = 10.
                * (power * 16. / (METER_SIZE * METER_SIZE) as f32)
                    .max(1e-10)
                    .log10();
            *target = ((db + 61.) / 61.).clamp(0., 1.);
        }
    }
}

fn follow_signal(
    levels: &mut [f32],
    targets: &[f32],
    animate: bool,
    seconds: f32,
    attack: f32,
    release: f32,
) -> bool {
    let rising = 1. - (-seconds * attack).exp();
    let falling = 1. - (-seconds * release).exp();
    let mut changed = false;
    for (level, &target) in levels.iter_mut().zip(targets) {
        let delta = target - *level;
        let next = if !animate || delta.abs() < 0.005 {
            target
        } else {
            *level + delta * if delta > 0. { rising } else { falling }
        };
        changed |= next != *level;
        *level = next;
    }
    changed
}

struct Diagnostics {
    path: PathBuf,
    start: Instant,
    phase: usize,
    samples: Vec<serde_json::Value>,
    intro_start: Option<(f64, u64)>,
    intros: Vec<serde_json::Value>,
    opening_frames: Vec<serde_json::Value>,
    meter_source: Option<(String, String)>,
    meter_presents: Vec<f64>,
    measure_layout: bool,
    layout_frames: Vec<f64>,
    layout_toolbar: Vec<Option<String>>,
    layout_frame_updates: Vec<serde_json::Value>,
}

impl Diagnostics {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            start: Instant::now(),
            phase: 0,
            samples: Vec::new(),
            intro_start: None,
            intros: Vec::new(),
            opening_frames: Vec::new(),
            meter_source: None,
            meter_presents: Vec::new(),
            measure_layout: false,
            layout_frames: Vec::new(),
            layout_toolbar: Vec::new(),
            layout_frame_updates: Vec::new(),
        }
    }
}

fn client_animations_enabled() -> bool {
    let mut enabled = 1;
    unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut enabled).cast(), 0);
    }
    enabled != 0
}

fn cpu_milliseconds() -> f64 {
    let (mut creation, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        );
    }
    let duration =
        |time: FILETIME| ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    (duration(kernel) + duration(user)) as f64 / 10_000.
}

fn apply_fixture(prefs: &mut Preferences) {
    prefs.station_id = Some("radio-4you".into());
    prefs
        .favorites
        .extend(["radio-4you".into(), "radius".into()]);
    prefs.equalizer_enabled = true;
    prefs.equalizer = [3, 4, 2, 0, -1, 0, 1, 2, 2, 1];
}

fn capture(arguments: &[OsString]) -> Result<()> {
    let _gdi = GdiPlus::start()?;
    let mut prefs = Preferences::default();
    apply_fixture(&mut prefs);
    let screen = unicode(&arguments[1])?;
    prefs.station_list_collapsed = matches!(screen, "compact" | "compact-intro");
    let (page, frame) = if matches!(screen, "intro" | "intro-return" | "compact-intro") {
        let time = arguments
            .get(4)
            .map(unicode)
            .transpose()?
            .map(str::parse)
            .transpose()?
            .unwrap_or(950.);
        ensure!(
            (0. ..=END_MS).contains(&time),
            "Screenshot time is outside the intro"
        );
        (Page::Player, Some(Frame::at(time)))
    } else {
        ensure!(
            arguments.len() < 5,
            "Screenshot time is only available for the intro"
        );
        (
            Page::parse(if screen == "compact" {
                "player"
            } else {
                screen
            })
            .context("Unknown screenshot view")?,
            None,
        )
    };
    if let Some(theme) = arguments.get(2) {
        let theme = unicode(theme)?;
        ensure!(
            matches!(theme, "graphite" | "amber"),
            "Unknown screenshot theme"
        );
        prefs.theme = theme.to_owned();
        if theme == "amber" {
            prefs.visualization = "waveform".into();
        }
    }
    if let Some(language) = arguments.get(3) {
        let language = unicode(language)?;
        ensure!(
            matches!(language, "en" | "ru" | "kk"),
            "Unknown screenshot language"
        );
        prefs.language = language.to_owned();
    }
    let catalog = Catalog::load();
    let list = catalog.list(&prefs, &prefs.group, "", page == Page::Stations);
    let status = Status::Playing(Some("Driguo — Take Me Higher".into()));
    let mut meter = Meter::new();
    let mut signal = Signal {
        revision: 1,
        ..Default::default()
    };
    for (step, sample) in signal.values.iter_mut().enumerate() {
        let t = step as f32 / signal.rate as f32;
        let sine = |frequency: f32| (std::f32::consts::TAU * frequency * t).sin();
        *sample = sine(72.) * 0.37
            + sine(157.) * 0.16
            + sine(245.) * 0.11
            + sine(606.) * 0.07
            + sine(1852.) * 0.035;
    }
    meter.update(signal, false, prefs.visualization == "waveform");
    let width = player_width(prefs.station_list_collapsed);
    let surface = Surface::new(width as i32, HEIGHT)?;
    {
        let mut canvas = Canvas::open(surface.dc, 1.)?;
        ui::render(
            &mut canvas,
            &View {
                page,
                width,
                station_list_ready: page == Page::Player && !prefs.station_list_collapsed,
                prefs: &prefs,
                catalog: &catalog,
                current: catalog.get("radio-4you"),
                status: &status,
                list: &list,
                top: 0,
                query: "",
                native_search: false,
                group: &prefs.group,
                hot: None,
                focus: None,
                spectrum: meter.spectrum(),
                waveform: meter.waveform(),
                update: &UpdateDisplay::Blank,
                intro: frame,
                save_failed: false,
                startup_failed: false,
            },
        );
    }
    surface.png(Path::new(&arguments[0]))
}
