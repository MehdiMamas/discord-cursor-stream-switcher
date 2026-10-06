//! The tray icon, its menu, global hotkeys and background jobs (driver setup, updates).

use crate::config::Config;
use crate::driver::{self, RESOLUTIONS};
use crate::hotkey;
use crate::monitors::{self, Monitor};
use crate::paths;
use crate::shared::{Command, Shared, WM_STATUS_CHANGED};
use crate::system;
use crate::update::{self, Version};
use crate::{error, info, warn};
use std::cell::RefCell;
use std::sync::Arc;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const WM_TRAY: u32 = WM_APP + 1;
/// NIN_SELECT | NINF_KEY: the icon was activated with the keyboard.
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const WM_JOB_DONE: u32 = WM_APP + 4;
const HOTKEY_PAUSE: i32 = 1;
const HOTKEY_LOCK: i32 = 2;
const TIMER_FIRST_CHECK: usize = 1;
const TIMER_DAILY_CHECK: usize = 2;

const ID_PAUSE: u32 = 1;
const ID_LOCK: u32 = 2;
const ID_SHOW_CURSOR: u32 = 3;
const ID_GUARD: u32 = 4;
const ID_AUTOSTART: u32 = 5;
const ID_AUTO_UPDATES: u32 = 6;
const ID_CHECK_UPDATE: u32 = 7;
const ID_INSTALL_UPDATE: u32 = 8;
const ID_OPEN_CONFIG: u32 = 9;
const ID_RELOAD: u32 = 10;
const ID_OPEN_LOGS: u32 = 11;
const ID_ABOUT: u32 = 12;
const ID_QUIT: u32 = 13;
const ID_DRIVER_INSTALL: u32 = 14;
const ID_DRIVER_REMOVE: u32 = 15;
const ID_DISPLAY_SETTINGS: u32 = 16;
const ID_RES_BASE: u32 = 100;
const ID_HIDE_BASE: u32 = 200;

enum Job {
    Driver { action: &'static str, result: Result<u32, String> },
    UpdateCheck { manual: bool, result: Result<Option<Version>, String> },
    UpdateInstall(Result<(), String>),
}

#[derive(Clone, Copy, PartialEq)]
enum BalloonAction {
    None,
    InstallDriver,
    DisplaySettings,
    InstallUpdate,
}

struct App {
    hwnd: HWND,
    shared: Arc<Shared>,
    cfg: Config,
    icon_on: HICON,
    icon_paused: HICON,
    taskbar_created: u32,
    update: Option<Version>,
    busy: bool,
    balloon: BalloonAction,
    menu_monitors: Vec<Monitor>,
    quitting: bool,
    layout_fixes_seen: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app state. Never call modal UI (menus, message boxes) inside `f`: their
/// message loops re-enter the window procedure.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let max = dst.len() - 1;
    let mut n = 0;
    for (i, c) in s.encode_utf16().take(max).enumerate() {
        dst[i] = c;
        n = i + 1;
    }
    dst[n] = 0;
}

fn load_icon(id: u16) -> HICON {
    // SAFETY: loads an icon resource embedded by build.rs.
    unsafe {
        let instance = GetModuleHandleW(None).ok();
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        LoadImageW(
            instance.map(Into::into),
            PCWSTR(id as usize as *const u16),
            IMAGE_ICON,
            cx,
            cy,
            LR_DEFAULTCOLOR,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_default()
    }
}

/// Creates the tray, runs the message loop until Quit, then shuts down the other threads.
pub fn run(shared: Arc<Shared>, cfg: Config, startup_note: Option<String>) -> i32 {
    // SAFETY: standard window creation on this (UI) thread.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).expect("module handle");
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("CursorStreamSwitcher.Tray"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        // A hidden top-level window (not message-only) so it receives TaskbarCreated and
        // WM_DISPLAYCHANGE broadcasts.
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("CursorStreamSwitcher.Tray"),
            w!("Cursor Stream Switcher"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("tray window")
    };
    shared.set_tray_hwnd(hwnd);
    // SAFETY: registers a well-known message name.
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    let app = App {
        hwnd,
        shared: shared.clone(),
        cfg,
        icon_on: load_icon(1),
        icon_paused: load_icon(2),
        taskbar_created,
        update: None,
        busy: false,
        balloon: BalloonAction::None,
        menu_monitors: Vec::new(),
        quitting: false,
        layout_fixes_seen: 0,
    };
    APP.with(|a| *a.borrow_mut() = Some(app));
    with_app(|app| {
        app.add_icon();
        app.register_hotkeys();
        app.schedule_update_checks();
        if let Some(note) = &startup_note {
            app.balloon(BalloonAction::None, "Settings", note, true);
        } else {
            app.check_stream_display();
        }
    });

    let mut msg = MSG::default();
    // SAFETY: standard message loop.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    APP.with(|a| a.borrow_mut().take());
    0
}

impl App {
    fn notify_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        }
    }

    fn add_icon(&mut self) {
        let mut nid = self.notify_data();
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = self.icon_on;
        copy_wide(&mut nid.szTip, "Cursor Stream Switcher");
        // SAFETY: nid is fully initialized for these calls.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
        self.refresh_icon();
    }

    fn refresh_icon(&mut self) {
        let status = self.shared.status();
        if status.layout_fixes != self.layout_fixes_seen {
            self.layout_fixes_seen = status.layout_fixes;
            self.balloon(
                BalloonAction::None,
                "Stream screen moved",
                "The \"Stream Cursor\" screen sat between your monitors and blocked the mouse, so it was moved to the right edge.",
                false,
            );
        }
        let mut tip = String::from("Cursor Stream Switcher\n");
        tip += &if status.stream_display.is_none() {
            "No virtual display - open this menu to set it up".to_owned()
        } else if status.paused {
            "Paused".to_owned()
        } else if let Some(p) = &status.problem {
            p.clone()
        } else if let Some(src) = &status.source {
            format!(
                "{}{}{}",
                if status.source_hidden { "Hidden: " } else { "Showing " },
                src,
                if status.locked { " (locked)" } else { "" }
            )
        } else {
            "Waiting".to_owned()
        };
        let mut nid = self.notify_data();
        nid.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.hIcon =
            if status.paused || status.stream_display.is_none() { self.icon_paused } else { self.icon_on };
        copy_wide(&mut nid.szTip, &tip);
        // SAFETY: nid is fully initialized.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    fn balloon(&mut self, action: BalloonAction, title: &str, text: &str, warning: bool) {
        self.balloon = action;
        let mut nid = self.notify_data();
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
        copy_wide(&mut nid.szInfoTitle, title);
        copy_wide(&mut nid.szInfo, text);
        // SAFETY: nid is fully initialized.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    fn register_hotkeys(&mut self) {
        let mut problems = Vec::new();
        for (id, text, what) in [
            (HOTKEY_PAUSE, self.cfg.hotkey_pause.clone(), "pause"),
            (HOTKEY_LOCK, self.cfg.hotkey_lock.clone(), "lock"),
        ] {
            // SAFETY: (un)registering hotkeys for our own window.
            unsafe {
                let _ = UnregisterHotKey(Some(self.hwnd), id);
            }
            match hotkey::parse(&text) {
                Ok(Some(hk)) => {
                    // SAFETY: as above.
                    let r = unsafe {
                        RegisterHotKey(
                            Some(self.hwnd),
                            id,
                            HOT_KEY_MODIFIERS(hk.modifiers) | MOD_NOREPEAT,
                            hk.vk,
                        )
                    };
                    if r.is_err() {
                        problems.push(format!("{text} ({what}) is already used by another app"));
                    }
                }
                Ok(None) => {}
                Err(e) => problems.push(e),
            }
        }
        if !problems.is_empty() {
            warn!("hotkeys: {}", problems.join("; "));
            self.balloon(BalloonAction::None, "Hotkey not available", &problems.join("\n"), true);
        }
    }

    fn schedule_update_checks(&mut self) {
        // SAFETY: timers on our own window.
        unsafe {
            if self.cfg.check_for_updates {
                SetTimer(Some(self.hwnd), TIMER_FIRST_CHECK, 20_000, None);
                SetTimer(Some(self.hwnd), TIMER_DAILY_CHECK, 24 * 60 * 60 * 1000, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_FIRST_CHECK);
                let _ = KillTimer(Some(self.hwnd), TIMER_DAILY_CHECK);
            }
        }
    }

    /// Explains what to do if the virtual display is missing.
    fn check_stream_display(&mut self) {
        let all = monitors::enumerate(&self.cfg.stream_display);
        if let Some(stream) = all.iter().find(|m| m.is_stream) {
            if stream.primary && all.len() > 1 {
                self.balloon(
                    BalloonAction::DisplaySettings,
                    "Stream screen is your main display",
                    "Click here and make one of your real monitors the main display, or the taskbar will be out of reach.",
                    true,
                );
            }
            return;
        }
        if self.cfg.stream_display != "auto" {
            self.balloon(
                BalloonAction::None,
                "Stream display not found",
                &format!(
                    "No display named {} is active. Check stream_display in the settings.",
                    self.cfg.stream_display
                ),
                true,
            );
        } else if driver::is_installed() {
            self.balloon(
                BalloonAction::DisplaySettings,
                "Virtual display is switched off",
                "Click here, then set the \"Stream Cursor\" display to \"Extend desktop to this display\".",
                true,
            );
        } else {
            self.balloon(
                BalloonAction::InstallDriver,
                "One-time setup",
                "Click here to add the virtual \"Stream Cursor\" screen that Discord will share (needs admin).",
                false,
            );
        }
    }

    fn save_config(&mut self) {
        if let Err(e) = self.cfg.save(&paths::config_file()) {
            error!("saving settings failed: {e}");
            self.balloon(BalloonAction::None, "Couldn't save settings", &e, true);
        }
        self.shared.send(Command::Reload(self.cfg.clone()));
    }

    fn reload_config(&mut self) {
        let (cfg, problem) = Config::load(&paths::config_file());
        if let Some(p) = problem {
            self.balloon(BalloonAction::None, "Settings", &p, true);
            return;
        }
        self.cfg = cfg;
        self.register_hotkeys();
        self.schedule_update_checks();
        self.shared.send(Command::Reload(self.cfg.clone()));
        info!("settings reloaded");
    }

    fn build_menu(&mut self) -> HMENU {
        let status = self.shared.status();
        self.menu_monitors =
            monitors::enumerate(&self.cfg.stream_display).into_iter().filter(|m| !m.is_stream).collect();
        let installed = driver::is_installed();
        let current_mode = driver::current_mode();
        let stream_size = status.stream_display.is_some().then(|| {
            monitors::enumerate(&self.cfg.stream_display)
                .into_iter()
                .find(|m| m.is_stream)
                .map(|m| (m.rect.width() as u32, m.rect.height() as u32))
        });

        // SAFETY: menu handles are created here and destroyed by the caller.
        unsafe {
            let menu = CreatePopupMenu().unwrap_or_default();
            let add = |m: HMENU, id: u32, text: &str, checked: bool, enabled: bool| {
                let mut flags = MF_STRING;
                if checked {
                    flags |= MF_CHECKED;
                }
                if !enabled {
                    flags |= MF_GRAYED;
                }
                let _ = AppendMenuW(m, flags, id as usize, &HSTRING::from(text));
            };
            let sep = |m: HMENU| {
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            };
            let with_key = |label: &str, key: &str| {
                if key.is_empty() { label.to_owned() } else { format!("{label}\t{key}") }
            };

            add(menu, 0, &format!("Cursor Stream Switcher {}", env!("CARGO_PKG_VERSION")), false, false);
            let state = match (&status.stream_display, &status.source) {
                (None, _) => "No virtual display yet".to_owned(),
                (_, _) if status.paused => "Paused".to_owned(),
                (_, Some(src)) => {
                    format!("{} {src}", if status.source_hidden { "Hiding" } else { "Showing" })
                }
                _ => "Waiting for the mouse".to_owned(),
            };
            add(menu, 0, &state, false, false);
            sep(menu);
            add(menu, ID_PAUSE, &with_key("Pause stream", &self.cfg.hotkey_pause), status.paused, true);
            add(
                menu,
                ID_LOCK,
                &with_key("Lock to current screen", &self.cfg.hotkey_lock),
                status.locked,
                true,
            );

            let hide = CreatePopupMenu().unwrap_or_default();
            for (i, m) in self.menu_monitors.iter().enumerate() {
                add(hide, ID_HIDE_BASE + i as u32, &m.label(), self.cfg.is_hidden(&m.id), true);
            }
            let _ = AppendMenuW(menu, MF_POPUP, hide.0 as usize, w!("Hide from stream"));
            add(menu, ID_SHOW_CURSOR, "Show mouse cursor", self.cfg.show_cursor, true);
            add(
                menu,
                ID_GUARD,
                "Keep mouse off the stream screen",
                self.cfg.keep_mouse_off_stream_display,
                true,
            );
            sep(menu);

            let vd = CreatePopupMenu().unwrap_or_default();
            for (i, (w, h)) in RESOLUTIONS.iter().enumerate() {
                let current = current_mode.map(|(cw, ch, _)| (cw, ch)) == Some((*w, *h))
                    || (current_mode.is_none() && stream_size.flatten() == Some((*w, *h)));
                add(vd, ID_RES_BASE + i as u32, &format!("{w} x {h}"), current, installed && !self.busy);
            }
            sep(vd);
            add(
                vd,
                ID_DRIVER_INSTALL,
                if installed { "Reinstall driver..." } else { "Install driver..." },
                false,
                !self.busy,
            );
            add(vd, ID_DRIVER_REMOVE, "Remove driver...", false, installed && !self.busy);
            add(vd, ID_DISPLAY_SETTINGS, "Open Display settings", false, true);
            let _ = AppendMenuW(menu, MF_POPUP, vd.0 as usize, w!("Virtual display"));
            sep(menu);

            add(menu, ID_AUTOSTART, "Start with Windows", system::autostart_enabled(), true);
            add(menu, ID_AUTO_UPDATES, "Check for updates daily", self.cfg.check_for_updates, true);
            match self.update {
                Some(v) => add(menu, ID_INSTALL_UPDATE, &format!("Install update {v}"), false, !self.busy),
                None => add(menu, ID_CHECK_UPDATE, "Check for updates now", false, !self.busy),
            }
            sep(menu);
            add(menu, ID_OPEN_CONFIG, "Edit settings file", false, true);
            add(menu, ID_RELOAD, "Reload settings", false, true);
            add(menu, ID_OPEN_LOGS, "Open log folder", false, true);
            add(menu, ID_ABOUT, "About / help", false, true);
            sep(menu);
            add(menu, ID_QUIT, "Quit", false, true);
            menu
        }
    }

    /// Handles a menu command. Returns true if the user asked to remove the driver (the
    /// confirmation dialog is shown outside the app borrow).
    fn on_command(&mut self, id: u32) -> bool {
        let status = self.shared.status();
        match id {
            ID_PAUSE => self.shared.send(Command::SetPaused(!status.paused)),
            ID_LOCK => self.shared.send(Command::SetLocked(!status.locked)),
            ID_SHOW_CURSOR => {
                self.cfg.show_cursor = !self.cfg.show_cursor;
                self.save_config();
            }
            ID_GUARD => {
                self.cfg.keep_mouse_off_stream_display = !self.cfg.keep_mouse_off_stream_display;
                self.save_config();
            }
            ID_AUTOSTART => {
                if let Err(e) = system::set_autostart(!system::autostart_enabled()) {
                    self.balloon(BalloonAction::None, "Start with Windows", &e, true);
                }
            }
            ID_AUTO_UPDATES => {
                self.cfg.check_for_updates = !self.cfg.check_for_updates;
                self.save_config();
                self.schedule_update_checks();
            }
            ID_CHECK_UPDATE => self.start_update_check(true),
            ID_INSTALL_UPDATE => self.start_update_install(),
            ID_OPEN_CONFIG => system::open_with_notepad(&paths::config_file().display().to_string()),
            ID_RELOAD => self.reload_config(),
            ID_OPEN_LOGS => system::open(&paths::data_dir().display().to_string()),
            ID_ABOUT => system::open(&format!("https://github.com/{}#readme", update::REPO)),
            ID_QUIT => self.quit(),
            ID_DRIVER_INSTALL => {
                let (w, h, r) = driver::current_mode().unwrap_or(driver::DEFAULT_MODE);
                self.start_driver_job(
                    "install",
                    format!("driver-install --width {w} --height {h} --refresh {r}"),
                );
            }
            ID_DRIVER_REMOVE => return true,
            ID_DISPLAY_SETTINGS => system::open("ms-settings:display"),
            _ if (ID_RES_BASE..ID_RES_BASE + RESOLUTIONS.len() as u32).contains(&id) => {
                let (w, h) = RESOLUTIONS[(id - ID_RES_BASE) as usize];
                self.start_driver_job(
                    "resolution change",
                    format!("driver-configure --width {w} --height {h} --refresh 60"),
                );
            }
            _ if (ID_HIDE_BASE..ID_HIDE_BASE + self.menu_monitors.len() as u32).contains(&id) => {
                let m = self.menu_monitors[(id - ID_HIDE_BASE) as usize].clone();
                let hidden = !self.cfg.is_hidden(&m.id);
                self.cfg.set_hidden(&m.id, hidden);
                info!("{} {}", if hidden { "hiding" } else { "showing" }, m.label());
                self.save_config();
            }
            _ => {}
        }
        false
    }

    fn start_driver_job(&mut self, action: &'static str, args: String) {
        if self.busy {
            return;
        }
        self.busy = true;
        let hwnd = self.hwnd.0 as isize;
        info!("starting elevated driver {action}");
        std::thread::spawn(move || {
            let result = system::run_elevated(&args);
            post_job(hwnd, Job::Driver { action, result });
        });
    }

    fn start_update_check(&mut self, manual: bool) {
        if self.busy && manual {
            return;
        }
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || post_job(hwnd, Job::UpdateCheck { manual, result: update::check() }));
    }

    fn start_update_install(&mut self) {
        let Some(v) = self.update else { return };
        if self.busy {
            return;
        }
        self.busy = true;
        self.balloon(
            BalloonAction::None,
            "Updating",
            &format!("Downloading version {v}. Windows will ask to allow the installer."),
            false,
        );
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || post_job(hwnd, Job::UpdateInstall(update::install(v))));
    }

    fn on_job(&mut self, job: Job) {
        match job {
            Job::Driver { action, result } => {
                self.busy = false;
                match result {
                    Ok(code) if code == driver::EXIT_OK as u32 => {
                        let text = if action == "removal" {
                            "The virtual display was removed."
                        } else {
                            "Done. In Discord: Share Screen > Screens, and pick the screen that shows your active monitor."
                        };
                        self.balloon(BalloonAction::None, "Virtual display", text, false);
                    }
                    Ok(code) if code == driver::EXIT_REBOOT as u32 => {
                        self.balloon(
                            BalloonAction::None,
                            "Virtual display",
                            "Restart Windows to finish.",
                            true,
                        );
                    }
                    Ok(_) => self.balloon(
                        BalloonAction::None,
                        "Virtual display",
                        &format!("The driver {action} failed. Details are in the log folder (driver.log)."),
                        true,
                    ),
                    Err(e) if e == "cancelled" => info!("driver {action} cancelled at the UAC prompt"),
                    Err(e) => self.balloon(
                        BalloonAction::None,
                        "Virtual display",
                        &format!("Couldn't start the {action}: {e}"),
                        true,
                    ),
                }
                self.shared.send(Command::DisplaysChanged);
            }
            Job::UpdateCheck { manual, result } => match result {
                Ok(Some(v)) => {
                    let first = self.update != Some(v);
                    self.update = Some(v);
                    if first || manual {
                        self.balloon(
                            BalloonAction::InstallUpdate,
                            "Update available",
                            &format!("Version {v} is out. Click here to install it."),
                            false,
                        );
                    }
                }
                Ok(None) => {
                    if manual {
                        self.balloon(BalloonAction::None, "No update", "You have the latest version.", false);
                    }
                }
                Err(e) => {
                    warn!("update check failed: {e}");
                    if manual {
                        self.balloon(BalloonAction::None, "Update check failed", &e, true);
                    }
                }
            },
            Job::UpdateInstall(result) => {
                self.busy = false;
                match result {
                    // The installer normally closes this process itself and starts the new one.
                    Ok(()) => {
                        info!("update installed; exiting");
                        self.quit();
                    }
                    Err(e) if e == "cancelled" => {
                        info!("update cancelled");
                        self.balloon(BalloonAction::None, "Update cancelled", "Nothing was changed.", false);
                    }
                    Err(e) => {
                        error!("update failed: {e}");
                        self.balloon(BalloonAction::None, "Update failed", &e, true);
                    }
                }
            }
        }
    }

    fn quit(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.shared.send(Command::Quit);
        let nid = self.notify_data();
        // SAFETY: removing our own icon and ending our own message loop.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            PostQuitMessage(0);
        }
    }
}

fn post_job(hwnd: isize, job: Job) {
    let ptr = Box::into_raw(Box::new(job));
    // SAFETY: the window procedure takes ownership of the box; if posting fails we free it.
    unsafe {
        if PostMessageW(Some(HWND(hwnd as *mut _)), WM_JOB_DONE, WPARAM(0), LPARAM(ptr as isize)).is_err() {
            drop(Box::from_raw(ptr));
        }
    }
}

fn show_menu(hwnd: HWND) {
    let Some(menu) = with_app(|a| a.build_menu()) else {
        return;
    };
    let mut pt = POINT::default();
    // SAFETY: standard tray menu sequence; the menu is destroyed afterwards.
    let cmd = unsafe {
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    };
    if cmd == 0 {
        return;
    }
    if with_app(|a| a.on_command(cmd)) == Some(true) {
        // SAFETY: modal confirmation outside the app borrow.
        let answer = unsafe {
            MessageBoxW(
                Some(hwnd),
                w!(
                    "Remove the virtual \"Stream Cursor\" display and its driver?\n\nYou can add it again later from this menu."
                ),
                w!("Cursor Stream Switcher"),
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        if answer == IDYES {
            with_app(|a| a.start_driver_job("removal", "driver-uninstall".into()));
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let event = (lp.0 as u32) & 0xffff;
            match event {
                WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT => show_menu(hwnd),
                NIN_BALLOONUSERCLICK => {
                    let action = with_app(|a| std::mem::replace(&mut a.balloon, BalloonAction::None));
                    match action {
                        Some(BalloonAction::InstallDriver) => {
                            with_app(|a| a.on_command(ID_DRIVER_INSTALL));
                        }
                        Some(BalloonAction::DisplaySettings) => system::open("ms-settings:display"),
                        Some(BalloonAction::InstallUpdate) => {
                            with_app(|a| a.start_update_install());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_STATUS_CHANGED => {
            with_app(|a| a.refresh_icon());
            LRESULT(0)
        }
        WM_JOB_DONE => {
            // SAFETY: lParam is the Box<Job> leaked by post_job.
            let job = unsafe { Box::from_raw(lp.0 as *mut Job) };
            with_app(|a| a.on_job(*job));
            LRESULT(0)
        }
        WM_HOTKEY => {
            with_app(|a| {
                let status = a.shared.status();
                match wp.0 as i32 {
                    HOTKEY_PAUSE => a.shared.send(Command::SetPaused(!status.paused)),
                    HOTKEY_LOCK => a.shared.send(Command::SetLocked(!status.locked)),
                    _ => {}
                }
            });
            LRESULT(0)
        }
        WM_TIMER => {
            with_app(|a| {
                if wp.0 == TIMER_FIRST_CHECK {
                    // SAFETY: one-shot timer on our own window.
                    unsafe {
                        let _ = KillTimer(Some(a.hwnd), TIMER_FIRST_CHECK);
                    }
                }
                if a.cfg.check_for_updates {
                    a.start_update_check(false);
                }
            });
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            with_app(|a| a.shared.send(Command::DisplaysChanged));
            LRESULT(0)
        }
        // Sent by the installer and uninstaller (and Restart Manager) to close the app.
        WM_CLOSE => {
            with_app(|a| a.quit());
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                with_app(|a| a.quit());
            }
            LRESULT(0)
        }
        _ => {
            if with_app(|a| msg == a.taskbar_created) == Some(true) {
                // Explorer restarted: put the icon back.
                with_app(|a| a.add_icon());
                return LRESULT(0);
            }
            // SAFETY: default handling.
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }
}
