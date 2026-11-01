use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const WAL_HEADER: &[u8] = b"GRAPHITE-WAL-1\n";
const SNAPSHOT_HEADER_PREFIX: &str = "// graphite-snapshot seq=";
const MAX_RECORD_SIZE: u64 = 64 * 1024 * 1024;

pub const SNAPSHOT_FILE: &str = "snapshot.gql";
pub const WAL_FILE: &str = "wal.log";
pub const LOCK_FILE: &str = "db.lock";

/// Durable storage for one database directory.
///
/// Layout:
/// - `snapshot.gql` — the graph as of `sequence`, written atomically.
/// - `wal.log` — append-only transaction log for commits after the snapshot.
/// - `db.lock` — advisory exclusive lock held for the lifetime of the handle.
///
/// Each WAL record is `[sequence: u64][length: u64][crc32: u32][payload]`.
/// Recovery reads records until the first incomplete or corrupt record, which
/// treats a torn tail as an uncommitted transaction.
#[derive(Debug)]
pub struct Storage {
    directory: PathBuf,
    _lock: File,
    wal: File,
}

impl Storage {
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory).map_err(|error| {
            Error::new(format!(
                "cannot create database directory `{}`: {error}",
                directory.display()
            ))
        })?;

        let lock_path = directory.join(LOCK_FILE);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| {
                Error::new(format!(
                    "cannot open database lock `{}`: {error}",
                    lock_path.display()
                ))
            })?;

        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::new(format!(
                    "database `{}` is already open by another process",
                    directory.display()
                )));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(Error::new(format!(
                    "cannot lock database `{}`: {error}",
                    directory.display()
                )));
            }
        }

        let wal_path = directory.join(WAL_FILE);
        let mut wal = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&wal_path)
            .map_err(|error| {
                Error::new(format!(
                    "cannot open write-ahead log `{}`: {error}",
                    wal_path.display()
                ))
            })?;

        let length = wal
            .metadata()
            .map_err(|error| {
                Error::new(format!(
                    "cannot inspect write-ahead log `{}`: {error}",
                    wal_path.display()
                ))
            })?
            .len();

        if length < WAL_HEADER.len() as u64 {
            // An empty file or a partial header can only mean the log was
            // created but no record was ever written. Reinitialise it.
            wal.set_len(0).map_err(wal_error)?;
            wal.seek(SeekFrom::Start(0)).map_err(wal_error)?;
            wal.write_all(WAL_HEADER).map_err(wal_error)?;
            wal.sync_all().map_err(wal_error)?;
        } else {
            let mut header = [0_u8; WAL_HEADER.len()];
            wal.read_exact(&mut header).map_err(wal_error)?;
            if header != WAL_HEADER {
                return Err(Error::new(format!(
                    "`{}` is not a Graphite write-ahead log",
                    wal_path.display()
                )));
            }
        }

        // Appends must start at the end of the existing log, not at byte 0.
        wal.seek(SeekFrom::End(0)).map_err(wal_error)?;

        Ok(Storage {
            directory: directory.to_path_buf(),
            _lock: lock,
            wal,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Append one durable transaction record.
    pub fn append(&mut self, sequence: u64, payload: &[u8]) -> Result<()> {
        if payload.len() as u64 > MAX_RECORD_SIZE {
            return Err(Error::new(format!(
                "transaction is too large for the write-ahead log ({} bytes)",
                payload.len()
            )));
        }

        let checksum = crc32_parts(&[&sequence.to_le_bytes(), payload]);
        self.wal
            .write_all(&sequence.to_le_bytes())
            .and_then(|_| self.wal.write_all(&(payload.len() as u64).to_le_bytes()))
            .and_then(|_| self.wal.write_all(&checksum.to_le_bytes()))
            .and_then(|_| self.wal.write_all(payload))
            .map_err(wal_error)?;
        self.wal.sync_all().map_err(wal_error)?;
        Ok(())
    }

    /// Write a snapshot and then clear the WAL.
    ///
    /// The snapshot is durable before the WAL is truncated, and recovery skips
    /// WAL entries at or below the snapshot sequence, so a crash between these
    /// steps is safe.
    pub fn checkpoint(&mut self, sequence: u64, graph_script: &str) -> Result<()> {
        write_snapshot(&self.directory, sequence, graph_script)?;
        self.wal.set_len(0).map_err(wal_error)?;
        self.wal.seek(SeekFrom::Start(0)).map_err(wal_error)?;
        self.wal.write_all(WAL_HEADER).map_err(wal_error)?;
        self.wal.sync_all().map_err(wal_error)?;
        Ok(())
    }
}

fn wal_error(error: std::io::Error) -> Error {
    Error::new(format!("write-ahead log I/O failed: {error}"))
}

/// Read the snapshot and its sequence, if one exists.
pub fn read_snapshot(directory: &Path) -> Result<Option<(u64, String)>> {
    let path = directory.join(SNAPSHOT_FILE);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Error::new(format!(
                "cannot read snapshot `{}`: {error}",
                path.display()
            )))
        }
    };

    let mut sequence = 0;
    if let Some(first) = content.lines().next() {
        if let Some(rest) = first.strip_prefix(SNAPSHOT_HEADER_PREFIX) {
            let value = rest.split_whitespace().next().unwrap_or("0");
            sequence = value.parse::<u64>().map_err(|_| {
                Error::new(format!(
                    "snapshot `{}` has an invalid sequence number `{value}`",
                    path.display()
                ))
            })?;
        }
    }

    Ok(Some((sequence, content)))
}

/// Read every valid WAL record, stopping at a torn or corrupt tail.
pub fn read_wal(directory: &Path) -> Result<Vec<(u64, Vec<u8>)>> {
    let path = directory.join(WAL_FILE);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::new(format!(
                "cannot read write-ahead log `{}`: {error}",
                path.display()
            )))
        }
    };

    let mut data = Vec::new();
    file.read_to_end(&mut data).map_err(|error| {
        Error::new(format!(
            "cannot read write-ahead log `{}`: {error}",
            path.display()
        ))
    })?;

    if data.is_empty() {
        return Ok(Vec::new());
    }
    if !data.starts_with(WAL_HEADER) {
        // A crash while creating the log can leave a prefix of the header.
        if WAL_HEADER.starts_with(&data) {
            return Ok(Vec::new());
        }
        return Err(Error::new(format!(
            "`{}` is not a Graphite write-ahead log",
            path.display()
        )));
    }

    let mut records = Vec::new();
    let mut position = WAL_HEADER.len();

    while position < data.len() {
        let header_end = position + 8 + 8 + 4;
        if data.len() < header_end {
            break;
        }

        let sequence = u64::from_le_bytes(
            data[position..position + 8]
                .try_into()
                .expect("checked length"),
        );
        let length = u64::from_le_bytes(
            data[position + 8..position + 16]
                .try_into()
                .expect("checked length"),
        );
        let checksum = u32::from_le_bytes(
            data[position + 16..position + 20]
                .try_into()
                .expect("checked length"),
        );
        position = header_end;

        if length > MAX_RECORD_SIZE {
            break;
        }
        let length = length as usize;
        if data.len() - position < length {
            break;
        }

        let payload = &data[position..position + length];
        if crc32_parts(&[&sequence.to_le_bytes(), payload]) != checksum {
            break;
        }
        if sequence == 0 {
            return Err(Error::new(format!(
                "`{}` contains an invalid zero sequence number",
                path.display()
            )));
        }
        if let Some((previous, _)) = records.last() {
            if sequence <= *previous {
                return Err(Error::new(format!(
                    "`{}` has out-of-order sequence numbers ({previous} then {sequence})",
                    path.display()
                )));
            }
        }

        position += length;
        records.push((sequence, payload.to_vec()));
    }

    Ok(records)
}

/// Atomically replace `snapshot.gql` with a new snapshot.
pub fn write_snapshot(directory: &Path, sequence: u64, graph_script: &str) -> Result<()> {
    fs::create_dir_all(directory).map_err(|error| {
        Error::new(format!(
            "cannot create database directory `{}`: {error}",
            directory.display()
        ))
    })?;

    let path = directory.join(SNAPSHOT_FILE);
    let temporary = directory.join("snapshot.gql.tmp");

    let mut content = format!("{SNAPSHOT_HEADER_PREFIX}{sequence}\n");
    content.push_str(graph_script);

    let mut file = File::create(&temporary).map_err(|error| {
        Error::new(format!(
            "cannot create temporary snapshot `{}`: {error}",
            temporary.display()
        ))
    })?;
    file.write_all(content.as_bytes()).map_err(|error| {
        Error::new(format!(
            "cannot write temporary snapshot `{}`: {error}",
            temporary.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        Error::new(format!(
            "cannot sync temporary snapshot `{}`: {error}",
            temporary.display()
        ))
    })?;
    drop(file);

    fs::rename(&temporary, &path).map_err(|error| {
        Error::new(format!(
            "cannot replace snapshot `{}`: {error}",
            path.display()
        ))
    })?;
    sync_directory(directory)?;
    Ok(())
}

fn sync_directory(directory: &Path) -> Result<()> {
    let handle = File::open(directory).map_err(|error| {
        Error::new(format!(
            "cannot open database directory `{}` for sync: {error}",
            directory.display()
        ))
    })?;
    handle.sync_all().map_err(|error| {
        Error::new(format!(
            "cannot sync database directory `{}`: {error}",
            directory.display()
        ))
    })
}

fn crc32_parts(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for part in parts {
        for byte in *part {
            crc ^= *byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "graphite-storage-test-{}-{nanos}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            TempDirectory(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn appends_and_reads_records() {
        let directory = TempDirectory::new();
        let mut storage = Storage::open(&directory.0).unwrap();
        storage.append(1, b"CREATE (:A);").unwrap();
        storage.append(2, b"CREATE (:B);").unwrap();

        let records = read_wal(&directory.0).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0, 1);
        assert_eq!(records[0].1, b"CREATE (:A);");
        assert_eq!(records[1].0, 2);
    }

    #[test]
    fn locks_a_database_directory_until_dropped() {
        let directory = TempDirectory::new();
        let storage = Storage::open(&directory.0).unwrap();

        let error = Storage::open(&directory.0).unwrap_err();
        assert!(error.message.contains("already open"));

        drop(storage);
        Storage::open(&directory.0).unwrap();
    }

    #[test]
    fn reopens_at_the_end_of_the_log() {
        let directory = TempDirectory::new();
        {
            let mut storage = Storage::open(&directory.0).unwrap();
            storage.append(1, b"CREATE (:A);").unwrap();
        }
        {
            let mut storage = Storage::open(&directory.0).unwrap();
            storage.append(2, b"CREATE (:B);").unwrap();
        }

        let records = read_wal(&directory.0).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].1, b"CREATE (:A);");
        assert_eq!(records[1].1, b"CREATE (:B);");
    }

    #[test]
    fn repairs_a_torn_header() {
        let directory = TempDirectory::new();
        fs::write(directory.0.join(WAL_FILE), b"GRAPH").unwrap();

        let mut storage = Storage::open(&directory.0).unwrap();
        storage.append(1, b"CREATE (:A);").unwrap();

        let records = read_wal(&directory.0).unwrap();
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn ignores_a_torn_record_tail() {
        let directory = TempDirectory::new();
        let mut storage = Storage::open(&directory.0).unwrap();
        storage.append(1, b"CREATE (:A);").unwrap();
        drop(storage);

        let mut file = OpenOptions::new()
            .append(true)
            .open(directory.0.join(WAL_FILE))
            .unwrap();
        file.write_all(&99_u64.to_le_bytes()).unwrap();
        file.write_all(&100_u64.to_le_bytes()).unwrap();
        file.write_all(b"partial").unwrap();
        drop(file);

        let records = read_wal(&directory.0).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1, b"CREATE (:A);");
    }

    #[test]
    fn ignores_a_corrupt_record_tail() {
        let directory = TempDirectory::new();
        let mut storage = Storage::open(&directory.0).unwrap();
        storage.append(1, b"CREATE (:A);").unwrap();
        drop(storage);

        let path = directory.0.join(WAL_FILE);
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&path, bytes).unwrap();

        assert!(read_wal(&directory.0).unwrap().is_empty());
    }

    #[test]
    fn writes_and_reads_snapshot_sequence() {
        let directory = TempDirectory::new();
        write_snapshot(&directory.0, 42, "CREATE (:A);").unwrap();
        let (sequence, content) = read_snapshot(&directory.0).unwrap().unwrap();
        assert_eq!(sequence, 42);
        assert!(content.contains("CREATE (:A);"));
    }

    #[test]
    fn checkpoint_clears_the_wal() {
        let directory = TempDirectory::new();
        let mut storage = Storage::open(&directory.0).unwrap();
        storage.append(1, b"CREATE (:A);").unwrap();
        storage.checkpoint(1, "CREATE (:A);").unwrap();

        assert!(read_wal(&directory.0).unwrap().is_empty());
        let snapshot = read_snapshot(&directory.0).unwrap().unwrap();
        assert_eq!(snapshot.0, 1);
        assert!(snapshot.1.contains("CREATE (:A);"));
    }
}
