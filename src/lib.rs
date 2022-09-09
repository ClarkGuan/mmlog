use log::{Level, Log, Metadata, Record};
use std::ffi::{CStr, CString, NulError};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use std::{mem, ptr, slice};

#[macro_export]
macro_rules! dbg {
    ($val:expr $(,)?) => {
        match $val {
            tmp => {
                ::log::debug!("{} = {:#?}", stringify!($val), &tmp);
                tmp
            }
        }
    };
    ($($val:expr),+ $(,)?) => {
        ($($crate::dbg!($val)),+,)
    };
}

pub const KB: usize = 1024;
pub const MB: usize = KB * 1024;

// 不可能用到吧？
// pub const GB: usize = MB * 1024;
// pub const TB: usize = GB * 1024;

fn level_info(l: Level) -> &'static str {
    match l {
        Level::Error => "E",
        Level::Warn => "W",
        Level::Info => "I",
        Level::Debug => "D",
        Level::Trace => "T",
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("C style string nul error: {0}")]
    Nul(#[from] NulError),

    #[error("error: {0}")]
    Any(String),
}

impl Error {
    unsafe fn from_errno() -> Error {
        let errno = *libc::__errno_location();
        let s = CStr::from_ptr(libc::strerror(errno))
            .to_str()
            .expect("CStr::to_str()");
        Error::Any(format!("errno: {}, msg: {}", errno as isize, s))
    }
}

macro_rules! errno_try {
    ($actual:expr, $expect:expr, $bk:block) => {{
        let ret = $actual;
        if ret == $expect {
            $bk
            return Err($crate::Error::from_errno());
        }
        ret
    }};
    ($actual:expr, $expect:expr) => {
        errno_try!($actual, $expect, {})
    };
}

#[derive(Debug)]
pub struct Builder {
    size: usize,
    level: Level,
    sync: bool,
}

impl Builder {
    const MIN_SIZE: usize = 512 * KB;

    pub fn new() -> Builder {
        Builder {
            size: Self::MIN_SIZE,
            level: Level::Info,
            sync: false,
        }
    }

    pub fn size(mut self, s: usize) -> Self {
        self.size = s;
        self
    }

    pub fn level(mut self, l: Level) -> Self {
        self.level = l;
        self
    }

    pub fn sync(mut self, enable: bool) -> Self {
        self.sync = enable;
        self
    }

    fn make_sense(&mut self) {
        if self.size < Self::MIN_SIZE {
            self.size = Self::MIN_SIZE;
        }
    }

    pub fn build<P: AsRef<Path>>(mut self, name: P) -> Result<Logger> {
        self.make_sense();
        Logger::new(name, self.size, self.level, self.sync)
    }

    pub fn open<P: AsRef<Path>>(mut self, name: P) -> Result<Logger> {
        self.make_sense();
        Logger::open(name, self.size, self.level, self.sync)
    }
}

#[derive(Debug)]
pub struct Logger {
    addr: *mut libc::c_void,
    size: usize,
    level: Level,
    spin: SpinLock,
    sync: bool,
}

impl Logger {
    const HEADER_SIZE: usize = mem::size_of::<usize>();
    const EMPTY_STRING: String = String::new();

    fn new<P: AsRef<Path>>(name: P, size: usize, level: Level, sync: bool) -> Result<Logger> {
        let logger = Self::open_inner(
            name,
            size,
            level,
            sync,
            libc::O_CREAT | libc::O_RDWR | libc::O_TRUNC,
        )?;
        logger.set_offset(0);
        Ok(logger)
    }

    fn open<P: AsRef<Path>>(name: P, size: usize, level: Level, sync: bool) -> Result<Logger> {
        Self::open_inner(name, size, level, sync, libc::O_RDWR)
    }

    fn open_inner<P: AsRef<Path>>(
        name: P,
        size: usize,
        level: Level,
        sync: bool,
        mode: libc::c_int,
    ) -> Result<Logger> {
        let size = size + Self::HEADER_SIZE;
        unsafe {
            let path = name.as_ref();
            let cstr = CString::new(
                path.to_str()
                    .ok_or(Error::Any(format!("Path::to_str() -> {:?}", path)))?,
            )?;

            let fd = errno_try!(libc::open(cstr.as_ptr(), mode, 0o666), -1);
            errno_try!(libc::ftruncate(fd, size as _), -1, {
                libc::close(fd);
            });
            let addr = errno_try!(
                libc::mmap(
                    ptr::null_mut::<libc::c_void>(),
                    size as _,
                    libc::PROT_WRITE | libc::PROT_READ,
                    libc::MAP_SHARED,
                    fd,
                    0,
                ),
                libc::MAP_FAILED,
                {
                    libc::close(fd);
                }
            );
            errno_try!(libc::close(fd), -1);
            Ok(Logger {
                addr,
                size,
                level,
                spin: Default::default(),
                sync,
            })
        }
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { &slice::from_raw_parts(self.addr as _, self.size)[Self::HEADER_SIZE..] }
    }

    #[allow(mutable_transmutes)]
    unsafe fn as_mut_slice(&self) -> &mut [u8] {
        mem::transmute(self.as_slice())
    }

    fn offset(&self) -> usize {
        *unsafe { mem::transmute::<_, &usize>(self.addr) }
    }

    fn set_offset(&self, new: usize) {
        assert!(new <= self.size - Self::HEADER_SIZE);
        let offset: &mut usize = unsafe { mem::transmute(self.addr) };
        *offset = new;
    }

    fn size(&self) -> usize {
        self.size - Self::HEADER_SIZE
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        self.flush();
        unsafe {
            debug_assert_ne!(libc::munmap(self.addr, self.size as _), -1);
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        let metadata = record.metadata();
        if self.enabled(metadata) {
            unsafe {
                let mut msg = format!(
                    "[{:?} {} {} {} {}] {}",
                    SystemTime::UNIX_EPOCH
                        .elapsed()
                        .expect("SystemTime::elapsed()"),
                    libc::gettid(),
                    level_info(record.level()),
                    record.file().map_or(Self::EMPTY_STRING, |f| {
                        record
                            .line()
                            .map_or(Self::EMPTY_STRING, |nb| format!("{}:{}", f, nb))
                    }),
                    record.target(),
                    record.args()
                );

                if !msg.ends_with('\n') {
                    msg += "\n";
                }

                // 锁住 offset 的变化
                let _guard = self.spin.lock();

                let offset = self.offset();
                let source = msg.as_bytes();

                if offset + source.len() <= self.size() {
                    let n = (&mut self.as_mut_slice()[offset..])
                        .write(source)
                        .expect("Write::write()");
                    debug_assert_eq!(n, source.len());
                    self.set_offset(offset + n);
                } else {
                    let n = (&mut self.as_mut_slice()[offset..])
                        .write(source)
                        .expect("Write::write()");
                    debug_assert_eq!(n, self.size() - offset);
                    let left = (source.len() - n) % self.size();
                    let n = self
                        .as_mut_slice()
                        .write(&source[source.len() - left..])
                        .expect("Write::write()");
                    debug_assert_eq!(left, n);
                    self.set_offset(left);
                }
            }
        }
    }

    fn flush(&self) {
        unsafe {
            let flags = if self.sync {
                libc::MS_SYNC
            } else {
                libc::MS_ASYNC
            };
            debug_assert_ne!(libc::msync(self.addr, self.size as _, flags), -1);
        }
    }
}

unsafe impl Send for Logger {}
unsafe impl Sync for Logger {}

#[derive(Debug, Default)]
#[repr(transparent)]
struct SpinLock(AtomicBool);

impl SpinLock {
    fn lock(&self) -> LockGuard {
        loop {
            if let Ok(false) =
                self.0
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            {
                break;
            }
        }
        LockGuard(self)
    }

    fn unlock(&self) {
        let result = self
            .0
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst);
        assert!(matches!(result, Ok(true)));
    }
}

#[derive(Debug)]
#[repr(transparent)]
struct LockGuard<'a>(&'a SpinLock);

impl<'a> Drop for LockGuard<'a> {
    fn drop(&mut self) {
        self.0.unlock();
    }
}
