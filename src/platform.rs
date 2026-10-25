//! Platform focus observation is best-effort and deliberately fails closed.
use anyhow::{Result, bail};
#[derive(Debug, Clone)]
pub struct Foreground {
    pub app: String,
    pub token: u64,
}

#[cfg(target_os = "linux")]
pub fn foreground() -> Option<Foreground> {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{AtomEnum, ConnectionExt},
    };
    if std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland") {
        return None;
    }
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| {
        conn.intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    };
    let active = conn
        .get_property(
            false,
            root,
            atom(b"_NET_ACTIVE_WINDOW")?,
            AtomEnum::WINDOW,
            0,
            1,
        )
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()?;
    if active == 0 {
        return None;
    }
    let pid = conn
        .get_property(
            false,
            active,
            atom(b"_NET_WM_PID")?,
            AtomEnum::CARDINAL,
            0,
            1,
        )
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next();
    let app = pid
        .and_then(|p| std::fs::read_to_string(format!("/proc/{p}/comm")).ok())
        .or_else(|| {
            let r = conn
                .get_property(false, active, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
                .ok()?
                .reply()
                .ok()?;
            Some(String::from_utf8_lossy(&r.value).replace('\0', " "))
        })?;
    if app.trim().is_empty() {
        return None;
    }
    Some(Foreground {
        app: app.trim().to_owned(),
        token: active as u64,
    })
}
#[cfg(target_os = "linux")]
pub fn activate(target: &Foreground) -> Result<()> {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask},
    };
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
        .reply()?
        .atom;
    let event = ClientMessageEvent::new(32, target.token as u32, atom, [2, 0, 0, 0, 0]);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )?;
    conn.flush()?;
    Ok(())
}
#[cfg(target_os = "windows")]
pub fn foreground() -> Option<Foreground> {
    use windows::{
        Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
            UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
        },
        core::PWSTR,
    };
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let app = String::from_utf16_lossy(&buffer[..size as usize]);
        Some(Foreground {
            app,
            token: hwnd.0 as u64,
        })
    }
}
#[cfg(target_os = "windows")]
pub fn activate(target: &Foreground) -> Result<()> {
    use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::SetForegroundWindow};
    if unsafe { SetForegroundWindow(HWND(target.token as *mut _)) }.as_bool() {
        Ok(())
    } else {
        bail!("Windows refused foreground focus; press Ctrl+V in the target app")
    }
}
/// Show our own window natively. Windows never repaints a hidden window, so the app's update
/// loop can't react to the shortcut until the window is visible again.
#[cfg(target_os = "windows")]
pub fn reveal(token: u64) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{SW_SHOWNA, SetForegroundWindow, ShowWindow},
    };
    if token == 0 {
        return;
    }
    let hwnd = HWND(token as *mut _);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let _ = SetForegroundWindow(hwnd);
    }
}
/// macOS and X11 keep delivering redraws to hidden windows, so the update loop handles it.
#[cfg(not(target_os = "windows"))]
pub fn reveal(_token: u64) {}
#[cfg(target_os = "macos")]
pub fn foreground() -> Option<Foreground> {
    let out=std::process::Command::new("osascript").args(["-e","tell application \"System Events\" to get bundle identifier of first application process whose frontmost is true"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let app = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    if app.is_empty() {
        return None;
    }
    Some(Foreground { app, token: 0 })
}
#[cfg(target_os = "macos")]
pub fn activate(target: &Foreground) -> Result<()> {
    if !target
        .app
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        bail!("Invalid application identifier");
    }
    let status = std::process::Command::new("osascript")
        .args([
            "-e",
            &format!("tell application id \"{}\" to activate", target.app),
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        bail!("macOS denied focus restoration; check Automation permissions")
    }
}
/// Usable desktop rectangle (x, y, w, h) of the monitor under the pointer, excluding taskbars/docks.
/// Physical pixels on Windows/Linux, points on macOS (see `WORK_AREA_IN_POINTS`).
pub type WorkArea = (f32, f32, f32, f32);
pub const WORK_AREA_IN_POINTS: bool = cfg!(target_os = "macos");
/// Dock rectangle in egui points: full work-area width, bottom-aligned, ~28% of the height.
pub fn dock_rect(wa: WorkArea, ppp: f32, in_points: bool) -> (f32, f32, f32, f32) {
    let k = if in_points { 1.0 } else { 1.0 / ppp.max(0.1) };
    let (x, y, w, h) = (wa.0 * k, wa.1 * k, wa.2 * k, wa.3 * k);
    let dh = (h * 0.28).clamp(260.0, 420.0).min(h);
    (x, y + h - dh, w, dh)
}
#[cfg(target_os = "windows")]
pub fn work_area() -> Option<WorkArea> {
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
        UI::WindowsAndMessaging::GetCursorPos,
    };
    unsafe {
        let mut p = POINT::default();
        GetCursorPos(&mut p).ok()?;
        let m = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(m, &mut mi).as_bool() {
            return None;
        }
        let r = mi.rcWork;
        Some((
            r.left as f32,
            r.top as f32,
            (r.right - r.left) as f32,
            (r.bottom - r.top) as f32,
        ))
    }
}
#[cfg(target_os = "linux")]
pub fn work_area() -> Option<WorkArea> {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{AtomEnum, ConnectionExt},
    };
    if std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland") {
        return None;
    }
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| {
        conn.intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    };
    let desktop = atom(b"_NET_CURRENT_DESKTOP")
        .and_then(|a| {
            conn.get_property(false, root, a, AtomEnum::CARDINAL, 0, 1)
                .ok()?
                .reply()
                .ok()?
                .value32()?
                .next()
        })
        .unwrap_or(0) as usize;
    let vals: Vec<u32> = conn
        .get_property(
            false,
            root,
            atom(b"_NET_WORKAREA")?,
            AtomEnum::CARDINAL,
            0,
            4 * 64,
        )
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .collect();
    let v = vals
        .get(desktop * 4..desktop * 4 + 4)
        .or_else(|| vals.get(0..4))?;
    (v[2] > 0 && v[3] > 0).then(|| (v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32))
}
#[cfg(target_os = "macos")]
pub fn work_area() -> Option<WorkArea> {
    let js = "ObjC.import('AppKit');var s=$.NSScreen.mainScreen,f=s.frame,v=s.visibleFrame,p=$.NSScreen.screens.objectAtIndex(0).frame;[f.origin.x,f.origin.y,f.size.width,f.size.height,v.origin.x,v.origin.y,v.size.width,v.size.height,p.size.height].join(',')";
    let out = std::process::Command::new("osascript")
        .args(["-l", "JavaScript", "-e", js])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let n: Vec<f32> = String::from_utf8(out.stdout)
        .ok()?
        .trim()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if n.len() != 9 {
        return None;
    }
    // Cocoa origin is bottom-left of the primary screen; winit uses top-left.
    Some((n[4], n[8] - (n[5] + n[7]), n[6], n[7]))
}
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub fn work_area() -> Option<WorkArea> {
    None
}
pub fn paste(target: Option<&Foreground>) -> Result<()> {
    use enigo::{
        Direction::{Click, Press, Release},
        Enigo, Key, Keyboard, Settings,
    };
    let Some(target) = target else {
        bail!("No target app is known; content was copied. Paste manually.");
    };
    activate(target)?;
    // Window-manager focus requests are asynchronous. Never relax the identity check.
    let mut verified = false;
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if foreground()
            .as_ref()
            .is_some_and(|f| f.app == target.app && f.token == target.token)
        {
            verified = true;
            break;
        }
    }
    if !verified {
        bail!("Target focus could not be verified; paste manually");
    }
    let mut enigo = Enigo::new(&Settings::default())?;
    let modifier = if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    };
    enigo.key(modifier, Press)?;
    let result = enigo.key(Key::Unicode('v'), Click);
    let release = enigo.key(modifier, Release);
    result?;
    release?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dock_rect_is_bottom_aligned_in_points() {
        let (x, y, w, h) = dock_rect((0.0, 0.0, 1920.0, 1040.0), 1.0, false);
        assert_eq!((x, w), (0.0, 1920.0));
        assert!((h - 291.2).abs() < 0.01 && (y + h - 1040.0).abs() < 0.01);
        let (_, y, w, h) = dock_rect((0.0, 0.0, 2560.0, 1400.0), 1.5, false);
        assert!((w - 1706.67).abs() < 0.01 && (y + h - 933.33).abs() < 0.01);
        assert_eq!(dock_rect((10.0, 20.0, 1000.0, 500.0), 1.0, true).2, 1000.0);
    }
}
