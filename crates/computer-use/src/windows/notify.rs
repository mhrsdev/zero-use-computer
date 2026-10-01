//! Notifications on Windows through `UserNotificationListener`, the API
//! Windows provides for reading the user's notifications. Windows asks the
//! user once whether this program may read them (Settings > Privacy >
//! Notifications); without that permission nothing is read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use crate::error::{Error, Result};
use crate::types::Notification;

fn wine(e: windows::core::Error) -> Error {
    Error::Platform(format!("Windows notifications: {e}"))
}

/// Windows file time (100 ns since 1601) → Unix seconds.
fn unix(t: i64) -> Option<u64> {
    const EPOCH_DIFF: i64 = 11_644_473_600;
    u64::try_from(t / 10_000_000 - EPOCH_DIFF).ok()
}

/// How long to wait for Windows to answer one request (the access prompt,
/// the notification list).
const ASYNC_WAIT: Duration = Duration::from_secs(10);
/// How long `recent` waits for the reading thread as a whole.
const READ_WAIT: Duration = Duration::from_secs(25);

/// Wait for an asynchronous operation to finish, polling its status (an
/// IAsyncInfo status: 0 is "started"), up to `limit`; cancelled if it
/// doesn't. Whether it finished.
fn finished(
    status: impl Fn() -> windows::core::Result<i32>,
    cancel: impl Fn(),
    limit: Duration,
) -> bool {
    let start = Instant::now();
    loop {
        // Done, failed or cancelled (GetResults then reports the error).
        if !matches!(status(), Ok(0)) {
            return true;
        }
        if start.elapsed() >= limit {
            cancel();
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read() -> Result<Vec<Notification>> {
    // SAFETY: joins (or creates) this thread's multithreaded apartment.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let listener = UserNotificationListener::Current().map_err(|e| {
        Error::Unsupported(format!(
            "Windows doesn't offer notification access to this program ({e})"
        ))
    })?;
    let denied = || {
        Error::Permission(
            "reading notifications isn't allowed; turn on notification access for this program in Settings > Privacy & security > Notifications".into(),
        )
    };
    let mut status = listener.GetAccessStatus().map_err(wine)?;
    if status == UserNotificationListenerAccessStatus::Unspecified {
        // Ask once; the prompt may never show (no window to show it on),
        // so the wait is bounded.
        let op = listener.RequestAccessAsync().map_err(wine)?;
        if !finished(
            || op.Status().map(|s| s.0),
            || {
                let _ = op.Cancel();
            },
            ASYNC_WAIT,
        ) {
            return Err(denied());
        }
        status = op.GetResults().map_err(wine)?;
    }
    if status != UserNotificationListenerAccessStatus::Allowed {
        return Err(denied());
    }
    let op = listener
        .GetNotificationsAsync(NotificationKinds::Toast)
        .map_err(wine)?;
    if !finished(
        || op.Status().map(|s| s.0),
        || {
            let _ = op.Cancel();
        },
        ASYNC_WAIT,
    ) {
        return Err(Error::Platform(
            "Windows didn't list the notifications in time".into(),
        ));
    }
    let list = op.GetResults().map_err(wine)?;
    let generic = KnownNotificationBindings::ToastGeneric().map_err(wine)?;
    let mut out = Vec::new();
    for i in 0..list.Size().map_err(wine)? {
        let Ok(n) = list.GetAt(i) else { continue };
        let app = n
            .AppInfo()
            .and_then(|a| a.DisplayInfo())
            .and_then(|d| d.DisplayName())
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let time = n.CreationTime().ok().and_then(|t| unix(t.UniversalTime));
        let Ok(binding) = n
            .Notification()
            .and_then(|x| x.Visual())
            .and_then(|v| v.GetBinding(&generic))
        else {
            continue;
        };
        let Ok(texts) = binding.GetTextElements() else {
            continue;
        };
        let mut lines = Vec::new();
        for j in 0..texts.Size().unwrap_or(0) {
            if let Ok(t) = texts.GetAt(j).and_then(|t| t.Text()) {
                lines.push(t.to_string_lossy());
            }
        }
        let mut lines = lines.into_iter();
        out.push(Notification {
            app,
            title: lines.next().unwrap_or_default(),
            body: lines.collect::<Vec<_>>().join("\n"),
            time,
        });
    }
    out.sort_by_key(|n| n.time);
    Ok(out)
}

/// A reading thread that hasn't finished (stuck in a Windows call): no
/// second one is started while it is.
static READING: AtomicBool = AtomicBool::new(false);

/// Marks the reading thread as done when dropped (even if it panics).
struct Reading;

impl Drop for Reading {
    fn drop(&mut self) {
        READING.store(false, Ordering::Release);
    }
}

pub fn recent() -> Result<Vec<Notification>> {
    if READING.swap(true, Ordering::AcqRel) {
        return Err(Error::Platform(
            "Windows hasn't answered an earlier request for the notifications yet; try again later"
                .into(),
        ));
    }
    let reading = Reading;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("notifications".into())
        .spawn(move || {
            let _reading = reading;
            let _ = tx.send(read());
        })
        .map_err(|e| Error::Platform(format!("notification thread: {e}")))?;
    match rx.recv_timeout(READ_WAIT) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::Platform(
            "Windows didn't answer the request for the notifications in time".into(),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(Error::Platform("the notification thread failed".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_time_to_unix() {
        // 2024-01-01T00:00:00Z
        assert_eq!(super::unix(133_485_408_000_000_000), Some(1_704_067_200));
    }
}
