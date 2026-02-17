//! Async filesystem operation backed by `std::fs`.
//! 
//! Under DesCartes' single-threaded simulation runtime, synchronous I/O is safe
//! (no deadlock risk) and these wrappers simple execute `std::fs` calls inline
//! before returning
//! 
//! Simulated I/O delays can be configured via [`set_fs_delays`] to model realistic
//! storage performance characteristics.

use crate::time;
use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ------------------------------------------------------------------
// Delay configuration
// ------------------------------------------------------------------

/// Distribution for simulating I/O operation delays.
#[derive(Clone, Debug)]
pub enum DelayDistribution {
    /// No delay (instant completion)
    None,

    /// Constant delay for all operations.
    Constant(Duration),

    /// Delay proportional to data size: base * per_byte).
    /// For operations without a size (metadata, sync), usees only base.
    Proportional {
        base: Duration,
        per_byte: Duration,
    },

    /// Uniform random delay between min and max.
    /// Uses the simulation's deterministic RNG.
    Uniform { min: Duration, max: Duration },
}

impl DelayDistribution {
    /// Calculate the delay for an operation with the given byte count.
    pub fn delay(&self, bytes: usize) -> Duration {
        match self {
            DelayDistribution::None => Duration::ZERO,
            DelayDistribution::Constant(d) => *d,
            DelayDistribution::Proportional { base, per_byte } => {
                *base + per_byte.saturating_mul(bytes as u32)
            }
            DelayDistribution::Uniform { min: _, max: _ } => {
                todo!()
            }
        }
    }
}

impl Default for DelayDistribution {
    fn default() -> Self {
        Self::None
    }
}

// Configuration for filesystem operation delays
#[derive(Clone, Debug, Default)]
pub struct FsDelayConfig {
    /// Delay for read operations (per byte)
    pub read: DelayDistribution,

    /// Delay for write operations (per byte)
    pub write: DelayDistribution,

    /// Delay for sync/fsync operations
    pub sync: DelayDistribution,

    /// Delay for metadata operations (stat, rename, etc.)
    pub metadata: DelayDistribution,

    /// Delay for directory operations (readdir, mkdir, etc.)
    pub directory: DelayDistribution,
}

impl FsDelayConfig {
    /// Realistic SSD delays (rough approximations)
    /// - Sequential read: ~500 MB/s -> 2 ns/byte + 100 micro s base
    /// - Sequential write: ~300 MB/s -> 3.3 ns/byte + 100 micro s base
    /// - Fsync: 1-5 ms
    /// - Metadata: 50-100 micro s
    pub fn realistic_ssd() -> Self {
        Self {
            read: DelayDistribution::Proportional { 
                base: Duration::from_micros(100), 
                per_byte: Duration::from_nanos(2) 
            },
            write: DelayDistribution::Proportional { 
                base: Duration::from_micros(100), 
                per_byte: Duration::from_nanos(3) 
            },
            sync: DelayDistribution::Constant(Duration::from_millis(3)),
            metadata: DelayDistribution::Constant(Duration::from_micros(75)),
            directory: DelayDistribution::Constant(Duration::from_millis(50)),            
        }
    }

    /// Realistic HDD delays (conservative estimates).
    /// - Sequential read: ~150 MB/s -> 6.6 ns/byte + 5 ms seek
    /// - Sequential write: ~120 MB/s -> 8.3 ns/byte + 5 ms seek
    /// - Fsync 5-15 ms
    /// - Metadata: 5-10 ms (seek time dominates)
    pub fn realistic_hdd() -> Self {
Self {
            read: DelayDistribution::Proportional { 
                base: Duration::from_millis(5), 
                per_byte: Duration::from_nanos(7) 
            },
            write: DelayDistribution::Proportional { 
                base: Duration::from_millis(5), 
                per_byte: Duration::from_nanos(8) 
            },
            sync: DelayDistribution::Constant(Duration::from_millis(10)),
            metadata: DelayDistribution::Constant(Duration::from_millis(7)),
            directory: DelayDistribution::Constant(Duration::from_millis(7)),            
        }
    }
}

thread_local! {
    static FS_DELAYS: RefCell<FsDelayConfig> = RefCell::new(FsDelayConfig::default())
}

/// Set the filesystem delay configuration for the current simulation thread.
/// 
/// This must be called before any filesystem operaitons if you want to simulate
/// I/O delays. Call it after `runtime::install()` in your test setup.
/// 
/// # Example
/// ```no_run
/// use descartes_tokio::fs::{set_fs_delays, FsDelayConfig};
/// 
/// set_fs_delays(FsDelayConfig::realistic_ssd());
/// ```
pub fn set_fs_delays(config: FsDelayConfig) {
    FS_DELAYS.with(|delays| {
        *delays.borrow_mut() = config;
    });
}

/// Get the current filesystem delay configuration
pub fn get_fs_delays() -> FsDelayConfig {
    FS_DELAYS.with(|delays| delays.borrow().clone())
}

/// Apply a delay based on the distribution and operation size
async fn apply_delay(dist: &DelayDistribution, bytes: usize) {
    let delay = dist.delay(bytes);
    if !delay.is_zero() {
        time::sleep(delay).await;
    }
}

// -------------------------------------------------------------
// Free functions
// -------------------------------------------------------------


/// Rename a file or directory
pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let delay = get_fs_delays().metadata;
    apply_delay(&delay, 0).await;
    std::fs::rename(from, to)
}

/// Read the entire contents of a file into a `Vec<u8>`
pub async fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let result = std::fs::read(path)?;
    let delay = get_fs_delays().read;
    apply_delay(&delay, result.len()).await;
    Ok(result)
}

/// Write a slice as the entire contents of a file.
pub async fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let contents = contents.as_ref();
    let delay = get_fs_delays().write;
    apply_delay(&delay, contents.len()).await;
    std::fs::write(path, contents)
}

/// Read the entries of a directory
pub async fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    let delay = get_fs_delays().directory;
    apply_delay(&delay, 0).await;
    std::fs::read_dir(path).map(ReadDir)
}

/// Remove a file
pub async fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let delay = get_fs_delays().metadata;
    apply_delay(&delay, 0).await;
    std::fs::remove_file(path)
}

/// Remove a directory
pub async fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let delay = get_fs_delays().directory;
    apply_delay(&delay, 0).await;
    std::fs::remove_dir(path)
}

/// Remove a directory and all its contents
pub async fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let delay = get_fs_delays().directory;
    apply_delay(&delay, 0).await;
    std::fs::remove_dir_all(path)
}

/// Create a directory and all of its parent directories
pub async fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let delay = get_fs_delays().directory;
    apply_delay(&delay, 0).await;
    std::fs::create_dir_all(path)
}

/// Create a directory and all of its parent directories
pub async fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let delay = get_fs_delays().directory;
    apply_delay(&delay, 0).await;
    std::fs::create_dir(path)
}

/// Query the metadata for a path
pub async fn metadata(path: impl AsRef<Path>) -> io::Result<std::fs::Metadata> {
    let delay = get_fs_delays().metadata;
    apply_delay(&delay, 0).await;
    std::fs::metadata(path)
}

/// Set the permissions on a path
pub async fn set_permissions(
    path: impl AsRef<Path>,
    perm: std::fs::Permissions,
) -> io::Result<()> {
    let delay = get_fs_delays().metadata;
    apply_delay(&delay, 0).await;
    std::fs::set_permissions(path, perm)
}

// -------------------------------------------------------------
// File
// -------------------------------------------------------------

/// An async file handle wrapping `std::fs::File`
#[derive(Debug)]
pub struct File {
    inner: std::fs::File,
}

impl File {
    pub async fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        let delay = get_fs_delays().metadata;
        apply_delay(&delay, 0).await;
        std::fs::File::create(path).map(|f| Self { inner: f})
    }

    pub async fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let delay = get_fs_delays().metadata;
        apply_delay(&delay, 0).await;
        std::fs::File::open(path).map(|f| Self { inner: f})
    }

    pub async fn sync_all(&self) -> io::Result<()> {
        let delay = get_fs_delays().sync;
        apply_delay(&delay, 0).await;
        self.inner.sync_all()
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let delay = get_fs_delays().write;
        apply_delay(&delay, buf.len()).await;
        self.inner.write_all(buf)
    }

    /// Write a `u128` in big-endian byte order
    pub async fn write_u128(&mut self, n: u128) -> io::Result<()> {
        use std::io::Write;
        let delay = get_fs_delays().write;
        apply_delay(&delay, 16).await;
        self.inner.write_all(&n.to_be_bytes())
    }
}

// -------------------------------------------------------------
// ReadDir / DirEntry
// -------------------------------------------------------------

/// Iterator over directory entries.
pub struct ReadDir(std::fs::ReadDir);

impl ReadDir {
    /// Return the next entry in the directory
    pub async fn next_entry(&mut self) -> io::Result<Option<DirEntry>> {
        match self.0.next() {
            Some(Ok(entry)) => Ok(Some(DirEntry(entry))),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

/// A single directory entry.
#[derive(Debug)]
pub struct DirEntry(std::fs::DirEntry);

impl DirEntry {
    /// Return the file name of the entry.
    pub fn file_name(&self) -> OsString {
        self.0.file_name()
    }

    /// Return the full path of the entry.
    pub fn path(&self) -> PathBuf {
        self.0.path()
    }

    /// Return the file type of the entry.
    pub async fn file_type(&self) -> io::Result<std::fs::FileType> {
        self.0.file_type()
    }
}