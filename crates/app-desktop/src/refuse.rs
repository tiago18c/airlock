//! Saying why the app will not start, before any window exists.

/// Tell the person why this launch stops: a message box on Windows, where a
/// bundled build has no console, and the log and stderr elsewhere.
pub fn refuse_to_start(title: &str, message: &str) {
    tracing::error!(target: "airlock::app", "{message}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
        let (title, message) = (wide(title), wide(message));
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the
        // call, and a null owner window is allowed.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        eprintln!("{message}");
    }
}
