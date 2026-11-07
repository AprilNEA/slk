use std::future::Future;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{Native, NativeStore};
#[cfg(windows)]
pub(crate) use windows::{Native, NativeStore};

pub(crate) trait Platform {
    fn restrict(path: &Path, directory: bool) -> io::Result<()>;
    fn cancellation() -> io::Result<impl Future<Output = ()> + Send>;
}
