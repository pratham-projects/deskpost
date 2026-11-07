use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::name::{sanitize_content_type, sanitize_filename};
use crate::{Error, PutMeta, Result, Storage, StoredObject, hex};

const TMP_DIR: &str = ".tmp";
const META_EXT: &str = ".json";
const CHUNK: usize = 64 * 1024;

/// Filesystem driver. Layout: `<root>/ab/cd/abcd<28 hex>` (+ `.json` sidecar), temp files in `<root>/.tmp`.
pub struct FsStorage {
    root: PathBuf,
    max_object_bytes: u64,
}

/// Create `path` and parents readable by the service user only (the attachments are customer data).
async fn mkdir(path: &Path) -> std::io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    b.mode(0o700);
    b.create(path).await
}

/// Make a rename durable: flush the directory entry, not just the file contents.
async fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir).await?.sync_all().await
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// Owner-only exclusive file creation.
fn create_new_private() -> OpenOptions {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    o.mode(0o600);
    o
}

/// Removes the file on drop unless disarmed, so errors and cancelled futures leave no partial files.
struct TempGuard(Option<PathBuf>);

impl TempGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl FsStorage {
    /// Creates `root` if missing and resolves it once; a symlinked root itself is followed
    /// (operator-configured), symlinks below it are not.
    pub async fn new(root: impl AsRef<Path>, max_object_bytes: u64) -> Result<Self> {
        mkdir(root.as_ref()).await?;
        let root = fs::canonicalize(root.as_ref()).await?;
        Ok(Self {
            root,
            max_object_bytes,
        })
    }

    /// Strict shape check: `ab/cd/abcd` + 28 lowercase hex. Anything else (`..`, absolute, NUL,
    /// unicode lookalikes, wrong fan-out) is rejected before touching the filesystem.
    fn parse_key(key: &str) -> Result<(&str, &str)> {
        let b = key.as_bytes();
        let hex = |s: &[u8]| s.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
        let ok = b.len() == 38
            && b[2] == b'/'
            && b[5] == b'/'
            && hex(&b[0..2])
            && hex(&b[3..5])
            && hex(&b[6..38])
            && b[0..2] == b[6..8]
            && b[3..5] == b[8..10];
        if ok {
            Ok((&key[0..2], &key[3..5]))
        } else {
            Err(Error::InvalidKey)
        }
    }

    /// Plain directory (not a symlink) or absent.
    async fn check_dir(path: &Path) -> Result<bool> {
        match fs::symlink_metadata(path).await {
            Ok(m) if m.is_dir() => Ok(true),
            Ok(_) => Err(Error::Unsafe("symlink or non-directory in store")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Returns `(data, meta)` paths after refusing symlinks on every component under the root.
    async fn locate(&self, key: &str) -> Result<(PathBuf, PathBuf)> {
        let (a, b) = Self::parse_key(key)?;
        let l1 = self.root.join(a);
        let l2 = l1.join(b);
        if Self::check_dir(&l1).await? {
            Self::check_dir(&l2).await?;
        }
        let data = self.root.join(key);
        let mut meta = data.clone().into_os_string();
        meta.push(META_EXT);
        Ok((data, PathBuf::from(meta)))
    }

    /// `Some(metadata)` for a plain file, `None` if absent, error for a symlink or other type.
    async fn plain_file(path: &Path) -> Result<Option<std::fs::Metadata>> {
        match fs::symlink_metadata(path).await {
            Ok(m) if m.is_file() => Ok(Some(m)),
            Ok(_) => Err(Error::Unsafe("symlink or non-file in store")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn write_temp<R: AsyncRead + Unpin>(
        &self,
        mut body: R,
        tmp: &Path,
    ) -> Result<(u64, String)> {
        let mut file = create_new_private().open(tmp).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        let mut size = 0u64;
        loop {
            let n = body.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > self.max_object_bytes {
                return Err(Error::TooLarge {
                    max: self.max_object_bytes,
                });
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok((size, hex::encode(&hasher.finalize())))
    }
}

impl FsStorage {
    /// Delete leftovers in `<root>/.tmp` older than `older_than` (a crash between create and
    /// rename). Returns how many files went. A live upload is never that old.
    pub async fn sweep_tmp(&self, older_than: std::time::Duration) -> Result<u64> {
        let dir = self.root.join(TMP_DIR);
        let mut rd = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        let mut removed = 0;
        while let Some(entry) = rd.next_entry().await? {
            // symlink_metadata: never follow a link planted in the temp dir.
            let Ok(m) = fs::symlink_metadata(entry.path()).await else {
                continue;
            };
            let old = m
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age >= older_than);
            if m.is_file() && old && fs::remove_file(entry.path()).await.is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl Storage for FsStorage {
    type Reader = File;

    async fn put<R: AsyncRead + Send + Unpin>(
        &self,
        body: R,
        meta: PutMeta,
    ) -> Result<StoredObject> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let key = format!("{}/{}/{id}", &id[0..2], &id[2..4]);
        let (data_path, meta_path) = self.locate(&key).await?;

        let tmp_dir = self.root.join(TMP_DIR);
        mkdir(&tmp_dir).await?;
        Self::check_dir(&tmp_dir).await?;
        let dir = data_path.parent().expect("key has a parent");
        mkdir(dir).await?;
        // Re-check after creation: a symlink planted between locate and create_dir_all.
        self.locate(&key).await?;

        let tmp_data = tmp_dir.join(format!("{id}.data"));
        let tmp_meta = tmp_dir.join(format!("{id}.meta"));
        let mut g_data = TempGuard(Some(tmp_data.clone()));
        let mut g_meta = TempGuard(Some(tmp_meta.clone()));

        let (size, sha256) = self.write_temp(body, &tmp_data).await?;
        let obj = StoredObject {
            key,
            size,
            sha256,
            original_filename: meta.original_filename.as_deref().map(sanitize_filename),
            content_type: sanitize_content_type(meta.content_type.as_deref().unwrap_or("")),
        };
        let json = serde_json::to_vec(&obj).map_err(|e| Error::Meta(e.to_string()))?;
        let mut meta_file = create_new_private().open(&tmp_meta).await?;
        meta_file.write_all(&json).await?;
        meta_file.sync_all().await?;
        drop(meta_file);

        // Sidecar first: an object without data is invisible, data without sidecar is not.
        fs::rename(&tmp_meta, &meta_path).await?;
        g_meta.disarm();
        if let Err(e) = fs::rename(&tmp_data, &data_path).await {
            let _ = fs::remove_file(&meta_path).await;
            return Err(e.into());
        }
        g_data.disarm();
        // Both renames are in `dir`: one directory flush makes them durable before the DB row exists.
        sync_dir(dir).await?;
        Ok(obj)
    }

    async fn get(&self, key: &str) -> Result<(File, StoredObject)> {
        let (data, meta) = self.locate(key).await?;
        if Self::plain_file(&meta).await?.is_none() || Self::plain_file(&data).await?.is_none() {
            return Err(Error::NotFound);
        }
        let obj: StoredObject = serde_json::from_slice(&fs::read(&meta).await?)
            .map_err(|e| Error::Meta(e.to_string()))?;
        Ok((File::open(&data).await?, obj))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let (data, meta) = self.locate(key).await?;
        for p in [&data, &meta] {
            if Self::plain_file(p).await?.is_some() {
                match fs::remove_file(p).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        let (data, meta) = self.locate(key).await?;
        Ok(Self::plain_file(&data).await?.is_some() && Self::plain_file(&meta).await?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::io::AsyncReadExt;

    async fn store(max: u64) -> (TempDir, FsStorage) {
        let dir = TempDir::new().unwrap();
        let s = FsStorage::new(dir.path().join("root"), max).await.unwrap();
        (dir, s)
    }

    async fn read_all(mut r: File) -> Vec<u8> {
        let mut v = Vec::new();
        r.read_to_end(&mut v).await.unwrap();
        v
    }

    fn tmp_is_empty(s: &FsStorage) -> bool {
        match std::fs::read_dir(s.root.join(TMP_DIR)) {
            Ok(mut d) => d.next().is_none(),
            Err(_) => true,
        }
    }

    #[tokio::test]
    async fn roundtrip_size_hash_meta() {
        let (_d, s) = store(1024).await;
        let meta = PutMeta {
            original_filename: Some("../../evil\"name.txt".into()),
            content_type: Some("Text/Plain; charset=utf-8".into()),
        };
        let obj = s.put(&b"hello"[..], meta).await.unwrap();
        assert_eq!(obj.size, 5);
        assert_eq!(
            obj.sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(obj.original_filename.as_deref(), Some("evil_name.txt"));
        assert_eq!(obj.content_type, "text/plain");
        FsStorage::parse_key(&obj.key).unwrap();
        assert!(s.exists(&obj.key).await.unwrap());
        let (r, got) = s.get(&obj.key).await.unwrap();
        assert_eq!(got, obj);
        assert_eq!(read_all(r).await, b"hello");
        assert!(tmp_is_empty(&s));
    }

    #[tokio::test]
    async fn empty_object_and_exact_cap() {
        let (_d, s) = store(4).await;
        assert_eq!(s.put(&b""[..], PutMeta::default()).await.unwrap().size, 0);
        assert_eq!(
            s.put(&b"abcd"[..], PutMeta::default()).await.unwrap().size,
            4
        );
    }

    #[tokio::test]
    async fn over_cap_aborts_and_cleans_up() {
        let cap = 3 * CHUNK as u64 + 10;
        let (_d, s) = store(cap).await;
        // 10 MiB of zeros streamed from a reader that never materialises it.
        let big = tokio::io::repeat(0).take(10 * 1024 * 1024);
        let err = s.put(big, PutMeta::default()).await.unwrap_err();
        assert!(matches!(err, Error::TooLarge { max } if max == cap));
        assert!(tmp_is_empty(&s));
        let objects = walk(&s.root)
            .into_iter()
            .filter(|p| !p.starts_with(s.root.join(TMP_DIR)))
            .count();
        assert_eq!(objects, 0);
    }

    /// Reader that yields some bytes, then fails.
    struct Flaky(u8);
    impl AsyncRead for Flaky {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.0 == 0 {
                return std::task::Poll::Ready(Err(std::io::Error::other("boom")));
            }
            self.0 -= 1;
            buf.put_slice(b"partial");
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn read_error_leaves_no_partial_file() {
        let (_d, s) = store(1024).await;
        assert!(matches!(
            s.put(Flaky(3), PutMeta::default()).await,
            Err(Error::Io(_))
        ));
        assert!(tmp_is_empty(&s));
    }

    #[tokio::test]
    async fn cancelled_put_leaves_no_partial_file() {
        let (_d, s) = store(1024).await;
        let (mut w, r) = tokio::io::duplex(64);
        w.write_all(b"some bytes").await.unwrap();
        // Writer stays open and silent, so put hangs mid-stream until the timeout drops it.
        let res =
            tokio::time::timeout(Duration::from_millis(200), s.put(r, PutMeta::default())).await;
        assert!(res.is_err());
        assert!(tmp_is_empty(&s));
        drop(w);
    }

    fn walk(p: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    out.extend(walk(&path));
                } else {
                    out.push(path);
                }
            }
        }
        out
    }

    #[tokio::test]
    async fn hostile_keys_rejected() {
        let (d, s) = store(1024).await;
        std::fs::write(d.path().join("secret"), "x").unwrap();
        let good = s.put(&b"x"[..], PutMeta::default()).await.unwrap().key;
        let hostile = [
            "../secret".to_string(),
            "../../secret".into(),
            "/etc/passwd".into(),
            format!("/{good}"),
            format!("{good}/"),
            format!("{good}\0"),
            format!("{good}/../../secret"),
            format!("./{good}"),
            good.to_uppercase(),
            good.replace('/', "\\"),
            format!("ａ{}", &good[1..]),
            format!("{good}\u{202e}"),
            "%2e%2e/%2e%2e/secret".into(),
            "..%2f..%2fsecret".into(),
            format!("{}.json", good),
            "".into(),
            // right shape, wrong fan-out
            format!("00/00/{}", &good[6..]),
        ];
        for k in hostile {
            assert!(
                matches!(s.get(&k).await, Err(Error::InvalidKey)),
                "get {k:?}"
            );
            assert!(
                matches!(s.exists(&k).await, Err(Error::InvalidKey)),
                "exists {k:?}"
            );
            assert!(
                matches!(s.delete(&k).await, Err(Error::InvalidKey)),
                "delete {k:?}"
            );
        }
        assert!(d.path().join("secret").exists());
        assert!(s.exists(&good).await.unwrap());
    }

    #[tokio::test]
    async fn delete_is_idempotent() {
        let (_d, s) = store(1024).await;
        let k = s.put(&b"x"[..], PutMeta::default()).await.unwrap().key;
        s.delete(&k).await.unwrap();
        s.delete(&k).await.unwrap();
        assert!(!s.exists(&k).await.unwrap());
        assert!(matches!(s.get(&k).await, Err(Error::NotFound)));
        // never-existed but well-formed key
        s.delete("00/00/00000000000000000000000000000000")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn concurrent_puts() {
        let (_d, s) = store(1 << 20).await;
        let s = Arc::new(s);
        let tasks: Vec<_> = (0..32u8)
            .map(|i| {
                let s = s.clone();
                tokio::spawn(async move {
                    let body = vec![i; 100_000 + usize::from(i)];
                    let obj = s.put(&body[..], PutMeta::default()).await.unwrap();
                    (i, obj)
                })
            })
            .collect();
        let mut keys = std::collections::HashSet::new();
        for t in tasks {
            let (i, obj) = t.await.unwrap();
            assert!(keys.insert(obj.key.clone()));
            let (r, _) = s.get(&obj.key).await.unwrap();
            let data = read_all(r).await;
            assert_eq!(data.len(), 100_000 + usize::from(i));
            assert!(data.iter().all(|b| *b == i));
        }
        assert!(tmp_is_empty(&s));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_in_root_refused() {
        use std::os::unix::fs::symlink;
        let (d, s) = store(1024).await;
        let outside = d.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("abcd0000000000000000000000000000"), "stolen").unwrap();

        // fan-out dir is a symlink
        symlink(&outside, s.root.join("ab")).unwrap();
        let k = "ab/cd/abcd0000000000000000000000000000";
        assert!(matches!(s.get(k).await, Err(Error::Unsafe(_))));
        assert!(matches!(s.exists(k).await, Err(Error::Unsafe(_))));
        assert!(matches!(s.delete(k).await, Err(Error::Unsafe(_))));
        assert!(outside.join("abcd0000000000000000000000000000").exists());

        // second level is a symlink
        std::fs::create_dir(s.root.join("12")).unwrap();
        symlink(&outside, s.root.join("12/34")).unwrap();
        assert!(matches!(
            s.get("12/34/12340000000000000000000000000000").await,
            Err(Error::Unsafe(_))
        ));

        // the object file itself is a symlink
        std::fs::create_dir_all(s.root.join("ff/ee")).unwrap();
        let f = s.root.join("ff/ee/ffee0000000000000000000000000000");
        symlink(outside.join("abcd0000000000000000000000000000"), &f).unwrap();
        std::fs::write(f.with_extension("json"), "{}").unwrap();
        let k = "ff/ee/ffee0000000000000000000000000000";
        assert!(matches!(s.get(k).await, Err(Error::Unsafe(_))));
        assert!(matches!(s.delete(k).await, Err(Error::Unsafe(_))));
        assert!(outside.join("abcd0000000000000000000000000000").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_tmp_dir_refuses_put() {
        let (d, s) = store(1024).await;
        let outside = d.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, s.root.join(TMP_DIR)).unwrap();
        assert!(matches!(
            s.put(&b"x"[..], PutMeta::default()).await,
            Err(Error::Unsafe(_))
        ));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn put_never_writes_through_planted_fanout_symlink() {
        // Plant symlinks for every possible first-level directory; put must refuse, not follow.
        let (d, s) = store(1024).await;
        let outside = d.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        for i in 0..=255u8 {
            std::os::unix::fs::symlink(&outside, s.root.join(format!("{i:02x}"))).unwrap();
        }
        assert!(matches!(
            s.put(&b"x"[..], PutMeta::default()).await,
            Err(Error::Unsafe(_))
        ));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(tmp_is_empty(&s));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn files_and_dirs_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, s) = store(1024).await;
        let k = s.put(&b"secret"[..], PutMeta::default()).await.unwrap().key;
        let (data, meta) = s.locate(&k).await.unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&data), 0o600);
        assert_eq!(mode(&meta), 0o600);
        assert_eq!(mode(data.parent().unwrap()), 0o700);
        assert_eq!(mode(&s.root.join(TMP_DIR)), 0o700);
    }

    #[tokio::test]
    async fn sweep_removes_only_old_temp_files() {
        let (_d, s) = store(1024).await;
        s.put(&b"x"[..], PutMeta::default()).await.unwrap();
        let tmp = s.root.join(TMP_DIR);
        std::fs::write(tmp.join("stale.data"), b"junk").unwrap();
        // fresh files stay
        assert_eq!(s.sweep_tmp(Duration::from_secs(3600)).await.unwrap(), 0);
        assert!(tmp.join("stale.data").exists());
        // old enough: gone, and stored objects are untouched
        assert_eq!(s.sweep_tmp(Duration::ZERO).await.unwrap(), 1);
        assert!(!tmp.join("stale.data").exists());
        assert_eq!(s.sweep_tmp(Duration::ZERO).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn sweep_without_a_temp_dir_is_a_no_op() {
        let (_d, s) = store(1024).await;
        assert_eq!(s.sweep_tmp(Duration::ZERO).await.unwrap(), 0);
    }
}
