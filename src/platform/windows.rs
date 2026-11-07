use super::Platform;
use std::io;
use std::path::Path;
use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
use windows_permissions::constants::{SeObjectType, SecurityInformation};
use windows_permissions::utilities::current_process_sid;
use windows_permissions::wrappers::{ConvertSidToStringSid, SetNamedSecurityInfo};
use windows_permissions::{LocalBox, SecurityDescriptor};

pub(crate) use windows_native_keyring_store::Store as NativeStore;

pub(crate) struct Native;

impl Platform for Native {
    fn restrict(path: &Path, directory: bool) -> io::Result<()> {
        let sid = current_process_sid()?;
        let sid = ConvertSidToStringSid(&sid)?;
        let sid = sid
            .to_str()
            .ok_or_else(|| io::Error::other("Invalid Windows SID"))?;
        let inheritance = if directory { "OICI" } else { "" };
        let descriptor: LocalBox<SecurityDescriptor> =
            format!("D:P(A;{inheritance};FA;;;{sid})").parse()?;
        SetNamedSecurityInfo(
            path.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
            None,
            None,
            descriptor.dacl(),
            None,
        )
    }

    fn cancellation() -> io::Result<impl std::future::Future<Output = ()> + Send> {
        let mut interrupt = ctrl_c()?;
        let mut interrupt_break = ctrl_break()?;
        let mut close = ctrl_close()?;
        let mut logoff = ctrl_logoff()?;
        let mut shutdown = ctrl_shutdown()?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => (),
                _ = interrupt_break.recv() => (),
                _ = close.recv() => (),
                _ = logoff.recv() => (),
                _ = shutdown.recv() => (),
            }
        })
    }
}
