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
pub fn paste(target: Option<&Foreground>) -> Result<()> {
    use enigo::{
        Direction::{Click, Press, Release},
        Enigo, Key, Keyboard, Settings,
    };
    let Some(target) = target else {
        bail!("No target app is known; content was copied. Paste manually.");
    };
    activate(target)?;
    std::thread::sleep(std::time::Duration::from_millis(180));
    // Verify focus before injecting, rather than pasting into an unexpected window.
    let current = foreground();
    if !current
        .as_ref()
        .is_some_and(|f| f.app == target.app && f.token == target.token)
    {
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
