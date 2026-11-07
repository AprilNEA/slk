use super::Platform;
use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tokio::signal::unix::{SignalKind, signal};

#[cfg(target_os = "macos")]
pub(crate) use apple_native_keyring_store::keychain::Store as NativeStore;
#[cfg(target_os = "linux")]
pub(crate) use dbus_secret_service_keyring_store::Store as NativeStore;

pub(crate) struct Native;

impl Platform for Native {
    fn restrict(path: &Path, directory: bool) -> io::Result<()> {
        fs::set_permissions(
            path,
            Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
    }

    fn cancellation() -> io::Result<impl std::future::Future<Output = ()> + Send> {
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        let mut hangup = signal(SignalKind::hangup())?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => (),
                _ = terminate.recv() => (),
                _ = hangup.recv() => (),
            }
        })
    }
}
