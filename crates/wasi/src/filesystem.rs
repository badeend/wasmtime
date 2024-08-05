use crate::bindings::clocks::wall_clock;
use crate::bindings::filesystem::types::{self, ErrorCode, PathFlags};
use crate::bindings::io::streams::{InputStream, OutputStream};
use crate::runtime::{spawn_blocking, AbortOnDropJoinHandle};
use crate::{HostOutputStream, StreamError, Subscribe, TrappableError};
use anyhow::anyhow;
use bytes::{Bytes, BytesMut};
use std::collections::HashMap;
use std::io;
use std::mem;
use std::sync::Arc;
use system_interface::fs::FdFlags;

pub type FsResult<T> = Result<T, FsError>;

pub type FsError = TrappableError<types::ErrorCode>;

impl From<wasmtime::component::ResourceTableError> for FsError {
    fn from(error: wasmtime::component::ResourceTableError) -> Self {
        Self::trap(error)
    }
}

impl From<io::Error> for FsError {
    fn from(error: io::Error) -> Self {
        types::ErrorCode::from(error).into()
    }
}

#[derive(Clone)]
pub enum Descriptor {
    File(File),
    Dir(Dir),
    VDir(VDir),
}

pub(crate) enum PreopenMatch<'a> {
    Descriptor(&'a Descriptor),
    Path(PathAt<'a>),
}
impl<'a> PreopenMatch<'a> {
    fn path(self) -> Result<PathAt<'a>, types::ErrorCode> {
        match self {
            PreopenMatch::Path(path) => Ok(path),
            PreopenMatch::Descriptor(_) => Err(ErrorCode::NotPermitted.into()),
        }
    }
}

impl Descriptor {
    pub fn file(&self) -> Result<&File, types::ErrorCode> {
        match self {
            Descriptor::File(f) => Ok(f),
            Descriptor::Dir(_) | Descriptor::VDir(_) => Err(types::ErrorCode::IsDirectory),
        }
    }

    fn match_preopen<'a>(
        &'a self,
        path: &'a str,
        flags: PathFlags,
    ) -> Result<PreopenMatch<'a>, types::ErrorCode> {
        // TODO: proper support for "." and ".." _anywhere_ in the path. This here is only barely good enough to make the unit tests pass.
        if path == "." {
            return Ok(PreopenMatch::Descriptor(self));
        }

        match self {
            Descriptor::File(_) => Err(types::ErrorCode::NotDirectory),
            Descriptor::Dir(dir) => Ok(PreopenMatch::Path(PathAt { dir, path, flags })),
            Descriptor::VDir(v) => v.resolve_path(path, flags),
        }
    }

    pub(crate) async fn advise(
        &self,
        offset: types::Filesize,
        len: types::Filesize,
        advice: types::Advice,
    ) -> FsResult<()> {
        self.file()?.advise(offset, len, advice).await
    }

    pub(crate) async fn sync_data(&self) -> FsResult<()> {
        match self {
            Descriptor::File(f) => f.sync_data().await,
            Descriptor::Dir(d) => d.sync_data().await,
            Descriptor::VDir(v) => v.sync_data(),
        }
    }

    pub(crate) async fn get_flags(&self) -> FsResult<types::DescriptorFlags> {
        match self {
            Descriptor::File(f) => f.get_flags().await,
            Descriptor::Dir(d) => d.get_flags().await,
            Descriptor::VDir(v) => v.get_flags(),
        }
    }

    pub(crate) async fn get_type(&self) -> FsResult<types::DescriptorType> {
        match self {
            Descriptor::File(f) => f.get_type().await,
            Descriptor::Dir(_) | Descriptor::VDir(_) => Ok(types::DescriptorType::Directory),
        }
    }

    pub(crate) async fn set_size(&self, size: types::Filesize) -> FsResult<()> {
        self.file()?.set_size(size).await
    }

    pub(crate) async fn set_times(
        &self,
        atim: types::NewTimestamp,
        mtim: types::NewTimestamp,
    ) -> FsResult<()> {
        match self {
            Descriptor::File(f) => f.set_times(atim, mtim).await,
            Descriptor::Dir(d) => d.set_times(atim, mtim).await,
            Descriptor::VDir(v) => v.set_times(atim, mtim),
        }
    }

    pub(crate) async fn set_times_at(
        &self,
        path_flags: PathFlags,
        path: String,
        atim: types::NewTimestamp,
        mtim: types::NewTimestamp,
    ) -> FsResult<()> {
        match self.match_preopen(&path, path_flags)? {
            PreopenMatch::Descriptor(child) => child.set_times(atim, mtim).await,
            PreopenMatch::Path(path) => path.set_times(atim, mtim).await,
        }
    }

    pub(crate) async fn read_directory(&self) -> FsResult<ReaddirIterator> {
        match self {
            Descriptor::File(_) => Err(types::ErrorCode::NotDirectory.into()),
            Descriptor::Dir(d) => d.read_directory().await,
            Descriptor::VDir(v) => v.read_directory().await,
        }
    }

    pub(crate) async fn sync(&self) -> FsResult<()> {
        match self {
            Descriptor::File(f) => f.sync().await,
            Descriptor::Dir(d) => d.sync().await,
            Descriptor::VDir(v) => v.sync(),
        }
    }

    pub(crate) async fn create_directory_at(&self, path: String) -> FsResult<()> {
        self.match_preopen(&path, PathFlags::empty())?
            .path()?
            .create_directory()
            .await
    }

    pub(crate) async fn stat(&self) -> FsResult<types::DescriptorStat> {
        match self {
            Descriptor::File(f) => f.stat().await,
            Descriptor::Dir(d) => d.stat().await,
            Descriptor::VDir(v) => v.stat(),
        }
    }

    pub(crate) async fn stat_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> FsResult<types::DescriptorStat> {
        match self.match_preopen(&path, path_flags)? {
            PreopenMatch::Descriptor(child) => child.stat().await,
            PreopenMatch::Path(path) => path.stat().await,
        }
    }

    pub(crate) async fn link_at(
        old_descriptor: &Descriptor,
        // TODO delete the path flags from this function
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: &Descriptor,
        new_path: String,
    ) -> FsResult<()> {
        let old_path = old_descriptor
            .match_preopen(&old_path, old_path_flags)?
            .path()?;
        let new_path = new_descriptor
            .match_preopen(&new_path, PathFlags::empty())?
            .path()?;

        old_path.link(new_path).await
    }

    pub(crate) async fn open_at(
        &self,
        path_flags: PathFlags,
        path: String,
        oflags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FsResult<types::Descriptor> {
        match self.match_preopen(&path, path_flags)? {
            PreopenMatch::Descriptor(descriptor) => Ok(descriptor.clone()),
            PreopenMatch::Path(path) => path.open(oflags, flags).await,
        }
    }

    pub(crate) async fn readlink_at(&self, path: String) -> FsResult<String> {
        match self.match_preopen(&path, PathFlags::empty())? {
            PreopenMatch::Descriptor(_) => Err(ErrorCode::Invalid.into()),
            PreopenMatch::Path(path) => path.readlink().await,
        }
    }

    pub(crate) async fn remove_directory_at(&self, path: String) -> FsResult<()> {
        match self.match_preopen(&path, PathFlags::empty())? {
            PreopenMatch::Descriptor(_) => Err(ErrorCode::NotPermitted.into()),
            PreopenMatch::Path(path) => path.remove_directory().await,
        }
    }

    pub(crate) async fn rename_at(
        old_fd: &Descriptor,
        old_path: String,
        new_fd: &Descriptor,
        new_path: String,
    ) -> FsResult<()> {
        let old_path = old_fd
            .match_preopen(&old_path, PathFlags::empty())?
            .path()?;
        let new_path = new_fd
            .match_preopen(&new_path, PathFlags::empty())?
            .path()?;

        old_path.rename(new_path).await
    }

    pub(crate) async fn symlink_at(&self, src_path: String, dest_path: String) -> FsResult<()> {
        let src_path = self.match_preopen(&src_path, PathFlags::empty())?.path()?;
        let dest_path = self.match_preopen(&dest_path, PathFlags::empty())?.path()?;

        src_path.symlink(dest_path).await
    }

    pub(crate) async fn unlink_file_at(&self, path: String) -> FsResult<()> {
        self.match_preopen(&path, PathFlags::empty())?
            .path()?
            .unlink_file()
            .await
    }

    pub(crate) async fn read(
        &self,
        len: types::Filesize,
        offset: types::Filesize,
    ) -> FsResult<(Vec<u8>, bool)> {
        self.file()?.read(len, offset).await
    }

    pub(crate) async fn write(
        &self,
        buf: Vec<u8>,
        offset: types::Filesize,
    ) -> FsResult<types::Filesize> {
        self.file()?.write(buf, offset).await
    }

    pub(crate) fn read_via_stream(&self, offset: types::Filesize) -> FsResult<InputStream> {
        self.file()?.read_via_stream(offset)
    }

    pub(crate) fn write_via_stream(&self, offset: types::Filesize) -> FsResult<OutputStream> {
        self.file()?.write_via_stream(offset)
    }

    pub(crate) fn append_via_stream(&self) -> FsResult<OutputStream> {
        self.file()?.append_via_stream()
    }

    pub(crate) async fn is_same_object(a: &Descriptor, b: &Descriptor) -> anyhow::Result<bool> {
        match (a, b) {
            (Descriptor::File(a), Descriptor::File(b)) => Ok(File::is_same_object(a, b).await?),
            (Descriptor::Dir(a), Descriptor::Dir(b)) => Ok(Dir::is_same_object(a, b).await?),
            (Descriptor::VDir(a), Descriptor::VDir(b)) => Ok(VDir::is_same_object(a, b)),
            (_, _) => Ok(false),
        }
    }
    pub(crate) async fn metadata_hash(&self) -> FsResult<types::MetadataHashValue> {
        match self {
            Descriptor::File(f) => f.metadata_hash().await,
            Descriptor::Dir(d) => d.metadata_hash().await,
            Descriptor::VDir(v) => v.metadata_hash(),
        }
    }
    pub(crate) async fn metadata_hash_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> FsResult<types::MetadataHashValue> {
        match self.match_preopen(&path, path_flags)? {
            PreopenMatch::Descriptor(child) => child.metadata_hash().await,
            PreopenMatch::Path(path) => path.metadata_hash().await,
        }
    }
}

bitflags::bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct FilePerms: usize {
        const READ = 0b1;
        const WRITE = 0b10;
    }
}

bitflags::bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct OpenMode: usize {
        const READ = 0b1;
        const WRITE = 0b10;
    }
}

#[derive(Clone)]
pub struct File {
    /// The operating system File this struct is mediating access to.
    ///
    /// Wrapped in an Arc because the same underlying file is used for
    /// implementing the stream types. A copy is also needed for
    /// [`spawn_blocking`].
    ///
    /// [`spawn_blocking`]: Self::spawn_blocking
    pub file: Arc<cap_std::fs::File>,
    /// Permissions to enforce on access to the file. These permissions are
    /// specified by a user of the `crate::WasiCtxBuilder`, and are
    /// enforced prior to any enforced by the underlying operating system.
    pub perms: FilePerms,
    /// The mode the file was opened under: bits for reading, and writing.
    /// Required to correctly report the DescriptorFlags, because cap-std
    /// doesn't presently provide a cross-platform equivalent of reading the
    /// oflags back out using fcntl.
    pub open_mode: OpenMode,

    allow_blocking_current_thread: bool,
}

impl File {
    pub fn new(
        file: cap_std::fs::File,
        perms: FilePerms,
        open_mode: OpenMode,
        allow_blocking_current_thread: bool,
    ) -> Self {
        Self {
            file: Arc::new(file),
            perms,
            open_mode,
            allow_blocking_current_thread,
        }
    }

    async fn advise(
        &self,
        offset: types::Filesize,
        len: types::Filesize,
        advice: types::Advice,
    ) -> FsResult<()> {
        use system_interface::fs::{Advice as A, FileIoExt};
        use types::Advice;

        let advice = match advice {
            Advice::Normal => A::Normal,
            Advice::Sequential => A::Sequential,
            Advice::Random => A::Random,
            Advice::WillNeed => A::WillNeed,
            Advice::DontNeed => A::DontNeed,
            Advice::NoReuse => A::NoReuse,
        };

        self.spawn_blocking(move |f| f.advise(offset, len, advice))
            .await?;
        Ok(())
    }

    async fn sync_data(&self) -> FsResult<()> {
        match self.spawn_blocking(|f| f.sync_data()).await {
            Ok(()) => Ok(()),
            // On windows, `sync_data` uses `FileFlushBuffers` which fails with
            // `ERROR_ACCESS_DENIED` if the file is not upen for writing. Ignore
            // this error, for POSIX compatibility.
            #[cfg(windows)]
            Err(e)
                if e.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as _) =>
            {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn get_flags(&self) -> FsResult<types::DescriptorFlags> {
        use system_interface::fs::GetSetFdFlags;
        use types::DescriptorFlags;

        let flags = self.spawn_blocking(|f| f.get_fd_flags()).await?;
        let mut flags = descriptor_flags_from_fdflags(flags);
        if self.open_mode.contains(OpenMode::READ) {
            flags |= DescriptorFlags::READ;
        }
        if self.open_mode.contains(OpenMode::WRITE) {
            flags |= DescriptorFlags::WRITE;
        }
        Ok(flags)
    }

    async fn get_type(&self) -> FsResult<types::DescriptorType> {
        let meta = self.spawn_blocking(|f| f.metadata()).await?;
        Ok(descriptortype_from(meta.file_type()))
    }

    async fn set_size(&self, size: types::Filesize) -> FsResult<()> {
        if !self.perms.contains(FilePerms::WRITE) {
            Err(ErrorCode::NotPermitted)?;
        }
        self.spawn_blocking(move |f| f.set_len(size)).await?;
        Ok(())
    }

    async fn set_times(
        &self,
        atim: types::NewTimestamp,
        mtim: types::NewTimestamp,
    ) -> FsResult<()> {
        use fs_set_times::SetTimes;

        if !self.perms.contains(FilePerms::WRITE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        let atim = systemtimespec_from(atim)?;
        let mtim = systemtimespec_from(mtim)?;
        self.spawn_blocking(|f| f.set_times(atim, mtim)).await?;
        Ok(())
    }

    async fn read(
        &self,
        len: types::Filesize,
        offset: types::Filesize,
    ) -> FsResult<(Vec<u8>, bool)> {
        use std::io::IoSliceMut;
        use system_interface::fs::FileIoExt;

        if !self.perms.contains(FilePerms::READ) {
            return Err(ErrorCode::NotPermitted.into());
        }

        let (mut buffer, r) = self
            .spawn_blocking(move |f| {
                let mut buffer = vec![0; len.try_into().unwrap_or(usize::MAX)];
                let r = f.read_vectored_at(&mut [IoSliceMut::new(&mut buffer)], offset);
                (buffer, r)
            })
            .await;

        let (bytes_read, state) = match r? {
            0 => (0, true),
            n => (n, false),
        };

        buffer.truncate(
            bytes_read
                .try_into()
                .expect("bytes read into memory as u64 fits in usize"),
        );

        Ok((buffer, state))
    }

    async fn write(&self, buf: Vec<u8>, offset: types::Filesize) -> FsResult<types::Filesize> {
        use std::io::IoSlice;
        use system_interface::fs::FileIoExt;

        if !self.perms.contains(FilePerms::WRITE) {
            return Err(ErrorCode::NotPermitted.into());
        }

        let bytes_written = self
            .spawn_blocking(move |f| f.write_vectored_at(&[IoSlice::new(&buf)], offset))
            .await?;

        Ok(types::Filesize::try_from(bytes_written).expect("usize fits in Filesize"))
    }

    async fn sync(&self) -> FsResult<()> {
        match self.spawn_blocking(|f| f.sync_all()).await {
            Ok(()) => Ok(()),
            // On windows, `sync_data` uses `FileFlushBuffers` which fails with
            // `ERROR_ACCESS_DENIED` if the file is not upen for writing. Ignore
            // this error, for POSIX compatibility.
            #[cfg(windows)]
            Err(e)
                if e.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as _) =>
            {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    fn read_via_stream(&self, offset: types::Filesize) -> FsResult<InputStream> {
        if !self.perms.contains(FilePerms::READ) {
            Err(types::ErrorCode::BadDescriptor)?;
        }

        // Create a stream view for it.
        let reader = FileInputStream::new(self, offset);

        Ok(InputStream::File(reader))
    }

    fn write_via_stream(&self, offset: types::Filesize) -> FsResult<OutputStream> {
        if !self.perms.contains(FilePerms::WRITE) {
            Err(types::ErrorCode::BadDescriptor)?;
        }

        // Create a stream view for it.
        let writer = FileOutputStream::write_at(self, offset);
        Ok(Box::new(writer))
    }

    fn append_via_stream(&self) -> FsResult<OutputStream> {
        if !self.perms.contains(FilePerms::WRITE) {
            Err(types::ErrorCode::BadDescriptor)?;
        }

        // Create a stream view for it.
        let appender = FileOutputStream::append(self);

        Ok(Box::new(appender))
    }

    async fn stat(&self) -> FsResult<types::DescriptorStat> {
        // No permissions check on stat: if opened, allowed to stat it
        let meta = self.spawn_blocking(|f| f.metadata()).await?;
        Ok(descriptorstat_from(meta))
    }

    async fn is_same_object(a: &File, b: &File) -> FsResult<bool> {
        // Short circuit if they're the same object in memory.
        if Arc::ptr_eq(&a.file, &b.file) {
            return Ok(true);
        }

        let id_a = a.get_id().await?;
        let id_b = b.get_id().await?;

        Ok(id_a == id_b)
    }

    async fn get_id(&self) -> FsResult<FileId> {
        // No permissions check on metadata: if opened, allowed to stat it
        let meta = self
            .spawn_blocking(|f| f.metadata())
            .await
            .map_err(|err| FsError::trap(err))?;
        Ok(FileId::from_metadata(&meta))
    }

    async fn metadata_hash(&self) -> FsResult<types::MetadataHashValue> {
        Ok(self.get_id().await?.hash())
    }

    /// Returns `Some` when the current thread is allowed to block in filesystem
    /// operations, and otherwise returns `None` to indicate that
    /// `spawn_blocking` must be used.
    pub(crate) fn as_blocking_file(&self) -> Option<&cap_std::fs::File> {
        if self.allow_blocking_current_thread {
            Some(&self.file)
        } else {
            None
        }
    }

    /// Spawn a task on tokio's blocking thread for performing blocking
    /// syscalls on the underlying [`cap_std::fs::File`].
    pub(crate) async fn spawn_blocking<F, R>(&self, body: F) -> R
    where
        F: FnOnce(&cap_std::fs::File) -> R + Send + 'static,
        R: Send + 'static,
    {
        match self._spawn_blocking(body) {
            SpawnBlocking::Done(result) => result,
            SpawnBlocking::Spawned(task) => task.await,
        }
    }

    fn _spawn_blocking<F, R>(&self, body: F) -> SpawnBlocking<R>
    where
        F: FnOnce(&cap_std::fs::File) -> R + Send + 'static,
        R: Send + 'static,
    {
        match self.as_blocking_file() {
            Some(file) => SpawnBlocking::Done(body(file)),
            None => {
                let f = self.file.clone();
                SpawnBlocking::Spawned(spawn_blocking(move || body(&f)))
            }
        }
    }
}

enum SpawnBlocking<T> {
    Done(T),
    Spawned(AbortOnDropJoinHandle<T>),
}

bitflags::bitflags! {
    /// Permission bits for operating on a directory.
    ///
    /// Directories can be limited to being readonly. This will restrict what
    /// can be done with them, for example preventing creation of new files.
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct DirPerms: usize {
        /// This directory can be read, for example its entries can be iterated
        /// over and files can be opened.
        const READ = 0b1;

        /// This directory can be mutated, for example by creating new files
        /// within it.
        const MUTATE = 0b10;
    }
}

#[derive(Clone)]
pub struct Dir {
    /// The operating system file descriptor this struct is mediating access
    /// to.
    ///
    /// Wrapped in an Arc because a copy is needed for [`spawn_blocking`].
    ///
    /// [`spawn_blocking`]: Self::spawn_blocking
    pub dir: Arc<cap_std::fs::Dir>,
    /// Permissions to enforce on access to this directory. These permissions
    /// are specified by a user of the `crate::WasiCtxBuilder`, and
    /// are enforced prior to any enforced by the underlying operating system.
    ///
    /// These permissions are also enforced on any directories opened under
    /// this directory.
    pub perms: DirPerms,
    /// Permissions to enforce on any files opened under this directory.
    pub file_perms: FilePerms,
    /// The mode the directory was opened under: bits for reading, and writing.
    /// Required to correctly report the DescriptorFlags, because cap-std
    /// doesn't presently provide a cross-platform equivalent of reading the
    /// oflags back out using fcntl.
    pub open_mode: OpenMode,

    allow_blocking_current_thread: bool,
}

impl Dir {
    pub fn new(
        dir: cap_std::fs::Dir,
        perms: DirPerms,
        file_perms: FilePerms,
        open_mode: OpenMode,
        allow_blocking_current_thread: bool,
    ) -> Self {
        Dir {
            dir: Arc::new(dir),
            perms,
            file_perms,
            open_mode,
            allow_blocking_current_thread,
        }
    }

    async fn sync_data(&self) -> FsResult<()> {
        self.spawn_blocking(|d| Ok(d.open(std::path::Component::CurDir)?.sync_data()?))
            .await
    }

    async fn get_flags(&self) -> FsResult<types::DescriptorFlags> {
        use system_interface::fs::GetSetFdFlags;
        use types::DescriptorFlags;

        let flags = self.spawn_blocking(|d| d.get_fd_flags()).await?;
        let mut flags = descriptor_flags_from_fdflags(flags);
        if self.open_mode.contains(OpenMode::READ) {
            flags |= DescriptorFlags::READ;
        }
        if self.open_mode.contains(OpenMode::WRITE) {
            flags |= DescriptorFlags::MUTATE_DIRECTORY;
        }
        Ok(flags)
    }

    async fn read_directory(&self) -> FsResult<ReaddirIterator> {
        if !self.perms.contains(DirPerms::READ) {
            return Err(ErrorCode::NotPermitted.into());
        }

        enum ReaddirError {
            Io(std::io::Error),
            IllegalSequence,
        }
        impl From<std::io::Error> for ReaddirError {
            fn from(e: std::io::Error) -> ReaddirError {
                ReaddirError::Io(e)
            }
        }

        let entries = self
            .spawn_blocking(|d| {
                // Both `entries` and `metadata` perform syscalls, which is why they are done
                // within this `block` call, rather than delay calculating the metadata
                // for entries when they're demanded later in the iterator chain.
                Ok::<_, std::io::Error>(
                    d.entries()?
                        .map(|entry| {
                            let entry = entry?;
                            let meta = entry.metadata()?;
                            let type_ = descriptortype_from(meta.file_type());
                            let name = entry
                                .file_name()
                                .into_string()
                                .map_err(|_| ReaddirError::IllegalSequence)?;
                            Ok(types::DirectoryEntry { type_, name })
                        })
                        .collect::<Vec<Result<types::DirectoryEntry, ReaddirError>>>(),
                )
            })
            .await?
            .into_iter();

        // On windows, filter out files like `C:\DumpStack.log.tmp` which we
        // can't get full metadata for.
        #[cfg(windows)]
        let entries = entries.filter(|entry| {
            use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};
            if let Err(ReaddirError::Io(err)) = entry {
                if err.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32)
                    || err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32)
                {
                    return false;
                }
            }
            true
        });
        let entries = entries.map(|r| match r {
            Ok(r) => Ok(r),
            Err(ReaddirError::Io(e)) => Err(e.into()),
            Err(ReaddirError::IllegalSequence) => Err(ErrorCode::IllegalByteSequence.into()),
        });
        Ok(ReaddirIterator::new(entries))
    }

    async fn sync(&self) -> FsResult<()> {
        self.spawn_blocking(|d| Ok(d.open(std::path::Component::CurDir)?.sync_all()?))
            .await
    }

    async fn stat(&self) -> FsResult<types::DescriptorStat> {
        // No permissions check on stat: if opened, allowed to stat it
        let meta = self.spawn_blocking(|d| d.dir_metadata()).await?;
        Ok(descriptorstat_from(meta))
    }

    async fn set_times(
        &self,
        atim: types::NewTimestamp,
        mtim: types::NewTimestamp,
    ) -> FsResult<()> {
        use fs_set_times::SetTimes;

        if !self.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        let atim = systemtimespec_from(atim)?;
        let mtim = systemtimespec_from(mtim)?;
        self.spawn_blocking(|d| d.set_times(atim, mtim)).await?;
        Ok(())
    }

    async fn is_same_object(a: &Dir, b: &Dir) -> FsResult<bool> {
        // Short circuit if they're the same object in memory.
        if Arc::ptr_eq(&a.dir, &b.dir) {
            return Ok(true);
        }

        let id_a = a.get_id().await?;
        let id_b = b.get_id().await?;

        Ok(id_a == id_b)
    }

    async fn get_id(&self) -> FsResult<FileId> {
        // No permissions check on metadata: if opened, allowed to stat it
        let meta = self
            .spawn_blocking(|d| d.dir_metadata())
            .await
            .map_err(|err| FsError::trap(err))?;
        Ok(FileId::from_metadata(&meta))
    }

    async fn metadata_hash(&self) -> FsResult<types::MetadataHashValue> {
        Ok(self.get_id().await?.hash())
    }

    /// Spawn a task on tokio's blocking thread for performing blocking
    /// syscalls on the underlying [`cap_std::fs::Dir`].
    async fn spawn_blocking<F, R>(&self, body: F) -> R
    where
        F: FnOnce(&cap_std::fs::Dir) -> R + Send + 'static,
        R: Send + 'static,
    {
        if self.allow_blocking_current_thread {
            body(&self.dir)
        } else {
            let d = self.dir.clone();
            spawn_blocking(move || body(&d)).await
        }
    }
}

/// Represents an unvalidated(!) path, relative to a specific directory file descriptor.
pub(crate) struct PathAt<'a> {
    dir: &'a Dir,
    path: &'a str,
    flags: PathFlags,
}
impl<'a> PathAt<'a> {
    async fn create_directory(self) -> FsResult<()> {
        if !self.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        self.spawn_blocking(move |d, p| d.create_dir(p)).await?;
        Ok(())
    }

    async fn stat(&self) -> FsResult<types::DescriptorStat> {
        if !self.dir.perms.contains(DirPerms::READ) {
            return Err(ErrorCode::NotPermitted.into());
        }

        let meta = if Self::symlink_follow(self.flags) {
            self.spawn_blocking(move |d, p| d.metadata(p)).await?
        } else {
            self.spawn_blocking(move |d, p| d.symlink_metadata(p))
                .await?
        };

        Ok(descriptorstat_from(meta))
    }

    async fn set_times(
        &self,
        atim: types::NewTimestamp,
        mtim: types::NewTimestamp,
    ) -> FsResult<()> {
        use cap_fs_ext::DirExt;

        if !self.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        let atim = systemtimespec_from(atim)?;
        let mtim = systemtimespec_from(mtim)?;
        if Self::symlink_follow(self.flags) {
            self.spawn_blocking(move |d, p| {
                d.set_times(
                    p,
                    atim.map(cap_fs_ext::SystemTimeSpec::from_std),
                    mtim.map(cap_fs_ext::SystemTimeSpec::from_std),
                )
            })
            .await?;
        } else {
            self.spawn_blocking(move |d, p| {
                d.set_symlink_times(
                    p,
                    atim.map(cap_fs_ext::SystemTimeSpec::from_std),
                    mtim.map(cap_fs_ext::SystemTimeSpec::from_std),
                )
            })
            .await?;
        }
        Ok(())
    }

    async fn link(&self, new_path: PathAt<'_>) -> FsResult<()> {
        if !self.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        if !new_path.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        if Self::symlink_follow(self.flags) {
            return Err(ErrorCode::Invalid.into());
        }
        if Self::symlink_follow(new_path.flags) {
            return Err(ErrorCode::Invalid.into());
        }
        let new_dir_handle = std::sync::Arc::clone(&new_path.dir.dir);
        let new_path = new_path.path.to_string();
        self.spawn_blocking(move |d, p| d.hard_link(p, &new_dir_handle, &new_path))
            .await?;
        Ok(())
    }

    async fn open(
        &self,
        oflags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FsResult<types::Descriptor> {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt};
        use system_interface::fs::{FdFlags, GetSetFdFlags};
        use types::{DescriptorFlags, OpenFlags};

        if !self.dir.perms.contains(DirPerms::READ) {
            Err(ErrorCode::NotPermitted)?;
        }

        if !self.dir.perms.contains(DirPerms::MUTATE) {
            if oflags.contains(OpenFlags::CREATE) || oflags.contains(OpenFlags::TRUNCATE) {
                Err(ErrorCode::NotPermitted)?;
            }
            if flags.contains(DescriptorFlags::WRITE) {
                Err(ErrorCode::NotPermitted)?;
            }
        }

        // Track whether we are creating file, for permission check:
        let mut create = false;
        // Track open mode, for permission check and recording in created descriptor:
        let mut open_mode = OpenMode::empty();
        // Construct the OpenOptions to give the OS:
        let mut opts = cap_std::fs::OpenOptions::new();
        opts.maybe_dir(true);

        if oflags.contains(OpenFlags::CREATE) {
            if oflags.contains(OpenFlags::EXCLUSIVE) {
                opts.create_new(true);
            } else {
                opts.create(true);
            }
            create = true;
            opts.write(true);
            open_mode |= OpenMode::WRITE;
        }

        if oflags.contains(OpenFlags::TRUNCATE) {
            opts.truncate(true).write(true);
        }
        if flags.contains(DescriptorFlags::READ) {
            opts.read(true);
            open_mode |= OpenMode::READ;
        }
        if flags.contains(DescriptorFlags::WRITE) {
            opts.write(true);
            open_mode |= OpenMode::WRITE;
        } else {
            // If not opened write, open read. This way the OS lets us open
            // the file, but we can use perms to reject use of the file later.
            opts.read(true);
            open_mode |= OpenMode::READ;
        }
        if Self::symlink_follow(self.flags) {
            opts.follow(FollowSymlinks::Yes);
        } else {
            opts.follow(FollowSymlinks::No);
        }

        // These flags are not yet supported in cap-std:
        if flags.contains(DescriptorFlags::FILE_INTEGRITY_SYNC)
            || flags.contains(DescriptorFlags::DATA_INTEGRITY_SYNC)
            || flags.contains(DescriptorFlags::REQUESTED_WRITE_SYNC)
        {
            Err(ErrorCode::Unsupported)?;
        }

        if oflags.contains(OpenFlags::DIRECTORY) {
            if oflags.contains(OpenFlags::CREATE)
                || oflags.contains(OpenFlags::EXCLUSIVE)
                || oflags.contains(OpenFlags::TRUNCATE)
            {
                Err(ErrorCode::Invalid)?;
            }
        }

        // Now enforce this WasiCtx's permissions before letting the OS have
        // its shot:
        if !self.dir.perms.contains(DirPerms::MUTATE) && create {
            Err(ErrorCode::NotPermitted)?;
        }
        if !self.dir.file_perms.contains(FilePerms::WRITE) && open_mode.contains(OpenMode::WRITE) {
            Err(ErrorCode::NotPermitted)?;
        }

        // Represents each possible outcome from the spawn_blocking operation.
        // This makes sure we don't have to give spawn_blocking any way to
        // manipulate the table.
        enum OpenResult {
            Dir(cap_std::fs::Dir),
            File(cap_std::fs::File),
            NotDir,
        }

        let opened = self
            .spawn_blocking::<_, std::io::Result<OpenResult>>(move |d, p| {
                let mut opened = d.open_with(p, &opts)?;
                if opened.metadata()?.is_dir() {
                    Ok(OpenResult::Dir(cap_std::fs::Dir::from_std_file(
                        opened.into_std(),
                    )))
                } else if oflags.contains(OpenFlags::DIRECTORY) {
                    Ok(OpenResult::NotDir)
                } else {
                    // FIXME cap-std needs a nonblocking open option so that files reads and writes
                    // are nonblocking. Instead we set it after opening here:
                    let set_fd_flags = opened.new_set_fd_flags(FdFlags::NONBLOCK)?;
                    opened.set_fd_flags(set_fd_flags)?;
                    Ok(OpenResult::File(opened))
                }
            })
            .await?;

        match opened {
            OpenResult::Dir(dir) => Ok(Descriptor::Dir(Dir::new(
                dir,
                self.dir.perms,
                self.dir.file_perms,
                open_mode,
                self.dir.allow_blocking_current_thread,
            ))),

            OpenResult::File(file) => Ok(Descriptor::File(File::new(
                file,
                self.dir.file_perms,
                open_mode,
                self.dir.allow_blocking_current_thread,
            ))),

            OpenResult::NotDir => Err(ErrorCode::NotDirectory.into()),
        }
    }

    async fn readlink(&self) -> FsResult<String> {
        if !self.dir.perms.contains(DirPerms::READ) {
            return Err(ErrorCode::NotPermitted.into());
        }
        let link = self.spawn_blocking(move |d, p| d.read_link(p)).await?;
        Ok(link
            .into_os_string()
            .into_string()
            .map_err(|_| ErrorCode::IllegalByteSequence)?)
    }

    async fn remove_directory(&self) -> FsResult<()> {
        if !self.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        Ok(self.spawn_blocking(move |d, p| d.remove_dir(p)).await?)
    }

    async fn rename(&self, new_path: PathAt<'_>) -> FsResult<()> {
        if !self.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        if !new_path.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        let new_dir_handle = std::sync::Arc::clone(&new_path.dir.dir);
        let new_path = new_path.path.to_string();
        Ok(self
            .spawn_blocking(move |d, p| d.rename(p, &new_dir_handle, &new_path))
            .await?)
    }

    async fn symlink(&self, dest: PathAt<'_>) -> FsResult<()> {
        // On windows, Dir.symlink is provided by DirExt
        #[cfg(windows)]
        use cap_fs_ext::DirExt;

        // Only allow the creation of symlinks within the same physical preopen.
        if Dir::is_same_object(self.dir, dest.dir).await? == false {
            return Err(ErrorCode::NotPermitted.into());
        }

        if !self.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }

        let dest_path = dest.path.to_string();
        Ok(self
            .spawn_blocking(move |d, p| d.symlink(p, &dest_path))
            .await?)
    }

    async fn unlink_file(&self) -> FsResult<()> {
        use cap_fs_ext::DirExt;

        if !self.dir.perms.contains(DirPerms::MUTATE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        Ok(self
            .spawn_blocking(move |d, p| d.remove_file_or_symlink(p))
            .await?)
    }

    async fn metadata_hash(&self) -> FsResult<types::MetadataHashValue> {
        // No permissions check on metadata: if dir opened, allowed to stat it
        let symlink_follow = Self::symlink_follow(self.flags);
        let meta = self
            .spawn_blocking(move |d, p| {
                if symlink_follow {
                    d.metadata(p)
                } else {
                    d.symlink_metadata(p)
                }
            })
            .await?;
        Ok(FileId::from_metadata(&meta).hash())
    }

    /// Spawn a task on tokio's blocking thread for performing blocking
    /// syscalls on the underlying [`cap_std::fs::Dir`].
    async fn spawn_blocking<F, R>(&self, body: F) -> R
    where
        F: FnOnce(&cap_std::fs::Dir, &str) -> R + Send + 'static,
        R: Send + 'static,
    {
        if self.dir.allow_blocking_current_thread {
            body(&self.dir.dir, self.path)
        } else {
            let dir = self.dir.dir.clone();
            let path = self.path.to_string();
            spawn_blocking(move || body(&dir, &path)).await
        }
    }

    fn symlink_follow(path_flags: PathFlags) -> bool {
        path_flags.contains(PathFlags::SYMLINK_FOLLOW)
    }
}

#[derive(Clone)]
pub struct VDir {
    /// The Arc's allocation pointer also doubles as the "identity" of this
    /// virtual directory. Which is used to implement wasi-filesystem methods
    /// such as `is-same-object` and `metadata-hash`.
    entries: Arc<HashMap<String, Descriptor>>,
}

impl VDir {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(HashMap::new()),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn mount(&mut self, path: &str, descriptor: Descriptor) -> anyhow::Result<()> {
        use std::collections::hash_map::Entry;

        let entries = Arc::get_mut(&mut self.entries).expect("vdir already shared");

        let (entry_name, rest) = match path.split_once('/') {
            Some((p, s)) => (p, Some(s)),
            None => (path, None),
        };

        if !Self::is_valid_segment(entry_name) {
            return Err(anyhow!("invalid preopen guest path"));
        }

        if let Some(rest) = rest {
            let entry_descriptor = match entries.entry(entry_name.to_string()) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => v.insert(Descriptor::VDir(VDir::new())),
            };

            match entry_descriptor {
                Descriptor::File(_) => Err(anyhow!("conflicting preopen guest path. can not be both a file and virtual directory at the same time")),
                Descriptor::Dir(_) => Err(anyhow!("conflicting preopen guest path. can not overlay real directory with virtual directory")),
                Descriptor::VDir(v) => v.mount(rest, descriptor),
            }
        } else {
            match entries.entry(entry_name.to_string()) {
                Entry::Occupied(_) => Err(anyhow!("duplicate mount on same guest path")),
                Entry::Vacant(v) => {
                    v.insert(descriptor);
                    Ok(())
                }
            }
        }
    }

    pub(crate) fn resolve_path<'a>(
        &'a self,
        path: &'a str,
        flags: PathFlags,
    ) -> Result<PreopenMatch<'a>, types::ErrorCode> {
        let (current, rest) = match path.split_once('/') {
            Some((p, s)) => (p, Some(s)),
            None => (path, None),
        };

        // TODO: properly implement these special segments:
        if !Self::is_valid_segment(current) {
            return Err(types::ErrorCode::Unsupported);
        }

        let Some(child_descriptor) = self.entries.get(current) else {
            return Err(types::ErrorCode::NoEntry);
        };

        let Some(rest) = rest else {
            return Ok(PreopenMatch::Descriptor(child_descriptor));
        };

        match child_descriptor {
            Descriptor::File(_) => Err(types::ErrorCode::NoEntry), // Can't navigate into a regular file.
            Descriptor::Dir(dir) => Ok(PreopenMatch::Path(PathAt {
                dir,
                path: rest,
                flags,
            })),
            Descriptor::VDir(v) => v.resolve_path(rest, flags),
        }
    }

    fn sync_data(&self) -> FsResult<()> {
        // Nothing to do
        Ok(())
    }

    fn get_flags(&self) -> FsResult<types::DescriptorFlags> {
        Ok(types::DescriptorFlags::READ)
    }

    fn set_times(&self, _atim: types::NewTimestamp, _mtim: types::NewTimestamp) -> FsResult<()> {
        Err(ErrorCode::NotPermitted.into())
    }

    async fn read_directory(&self) -> FsResult<ReaddirIterator> {
        let mut results = Vec::new();
        for (name, descriptor) in self.entries.iter() {
            results.push(Ok(types::DirectoryEntry {
                type_: descriptor.get_type().await?,
                name: name.to_string(),
            }));
        }
        Ok(ReaddirIterator::new(results.into_iter()))
    }

    fn sync(&self) -> FsResult<()> {
        // Nothing to do
        Ok(())
    }

    fn stat(&self) -> FsResult<types::DescriptorStat> {
        Ok(types::DescriptorStat {
            type_: types::DescriptorType::Directory,
            link_count: 0,
            size: 0,
            data_access_timestamp: None,
            data_modification_timestamp: None,
            status_change_timestamp: None,
        })
    }

    fn get_id(&self) -> FileId {
        FileId::from_ptr(Arc::as_ptr(&self.entries))
    }

    fn is_same_object(a: &VDir, b: &VDir) -> bool {
        Arc::ptr_eq(&a.entries, &b.entries)
    }

    fn metadata_hash(&self) -> FsResult<types::MetadataHashValue> {
        Ok(self.get_id().hash())
    }

    fn is_valid_segment(s: &str) -> bool {
        !matches!(s, "" | "." | "..")
    }
}

pub struct FileInputStream {
    file: File,
    position: u64,
}
impl FileInputStream {
    pub fn new(file: &File, position: u64) -> Self {
        Self {
            file: file.clone(),
            position,
        }
    }

    pub async fn read(&mut self, size: usize) -> Result<Bytes, StreamError> {
        use system_interface::fs::FileIoExt;
        let p = self.position;

        let (r, mut buf) = self
            .file
            .spawn_blocking(move |f| {
                let mut buf = BytesMut::zeroed(size);
                let r = f.read_at(&mut buf, p);
                (r, buf)
            })
            .await;
        let n = read_result(r, size)?;
        buf.truncate(n);
        self.position += n as u64;
        Ok(buf.freeze())
    }

    pub async fn skip(&mut self, nelem: usize) -> Result<usize, StreamError> {
        let bs = self.read(nelem).await?;
        Ok(bs.len())
    }
}

fn read_result(r: io::Result<usize>, size: usize) -> Result<usize, StreamError> {
    match r {
        Ok(0) if size > 0 => Err(StreamError::Closed),
        Ok(n) => Ok(n),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(0),
        Err(e) => Err(StreamError::LastOperationFailed(e.into())),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum FileOutputMode {
    Position(u64),
    Append,
}

pub(crate) struct FileOutputStream {
    file: File,
    mode: FileOutputMode,
    state: OutputState,
}

enum OutputState {
    Ready,
    /// Allows join future to be awaited in a cancellable manner. Gone variant indicates
    /// no task is currently outstanding.
    Waiting(AbortOnDropJoinHandle<io::Result<usize>>),
    /// The last I/O operation failed with this error.
    Error(io::Error),
    Closed,
}

impl FileOutputStream {
    pub fn write_at(file: &File, position: u64) -> Self {
        Self {
            file: file.clone(),
            mode: FileOutputMode::Position(position),
            state: OutputState::Ready,
        }
    }

    pub fn append(file: &File) -> Self {
        Self {
            file: file.clone(),
            mode: FileOutputMode::Append,
            state: OutputState::Ready,
        }
    }
}

// FIXME: configurable? determine from how much space left in file?
const FILE_WRITE_CAPACITY: usize = 1024 * 1024;

impl HostOutputStream for FileOutputStream {
    fn write(&mut self, buf: Bytes) -> Result<(), StreamError> {
        use system_interface::fs::FileIoExt;
        match self.state {
            OutputState::Ready => {}
            OutputState::Closed => return Err(StreamError::Closed),
            OutputState::Waiting(_) | OutputState::Error(_) => {
                // a write is pending - this call was not permitted
                return Err(StreamError::Trap(anyhow!(
                    "write not permitted: check_write not called first"
                )));
            }
        }

        let m = self.mode;
        let result = self.file._spawn_blocking(move |f| {
            match m {
                FileOutputMode::Position(mut p) => {
                    let mut total = 0;
                    let mut buf = buf;
                    loop {
                        let nwritten = f.write_at(buf.as_ref(), p)?;
                        // afterwards buf contains [nwritten, len):
                        let _ = buf.split_to(nwritten);
                        p += nwritten as u64;
                        total += nwritten;
                        if buf.is_empty() {
                            break;
                        }
                    }
                    Ok(total)
                }
                FileOutputMode::Append => {
                    let mut total = 0;
                    let mut buf = buf;
                    loop {
                        let nwritten = f.append(buf.as_ref())?;
                        let _ = buf.split_to(nwritten);
                        total += nwritten;
                        if buf.is_empty() {
                            break;
                        }
                    }
                    Ok(total)
                }
            }
        });
        self.state = match result {
            SpawnBlocking::Done(Ok(nwritten)) => {
                if let FileOutputMode::Position(ref mut p) = &mut self.mode {
                    *p += nwritten as u64;
                }
                OutputState::Ready
            }
            SpawnBlocking::Done(Err(e)) => OutputState::Error(e),
            SpawnBlocking::Spawned(task) => OutputState::Waiting(task),
        };
        Ok(())
    }
    fn flush(&mut self) -> Result<(), StreamError> {
        match self.state {
            // Only userland buffering of file writes is in the blocking task,
            // so there's nothing extra that needs to be done to request a
            // flush.
            OutputState::Ready | OutputState::Waiting(_) => Ok(()),
            OutputState::Closed => Err(StreamError::Closed),
            OutputState::Error(_) => match mem::replace(&mut self.state, OutputState::Closed) {
                OutputState::Error(e) => Err(StreamError::LastOperationFailed(e.into())),
                _ => unreachable!(),
            },
        }
    }
    fn check_write(&mut self) -> Result<usize, StreamError> {
        match self.state {
            OutputState::Ready => Ok(FILE_WRITE_CAPACITY),
            OutputState::Closed => Err(StreamError::Closed),
            OutputState::Error(_) => match mem::replace(&mut self.state, OutputState::Closed) {
                OutputState::Error(e) => Err(StreamError::LastOperationFailed(e.into())),
                _ => unreachable!(),
            },
            OutputState::Waiting(_) => Ok(0),
        }
    }
}

#[async_trait::async_trait]
impl Subscribe for FileOutputStream {
    async fn ready(&mut self) {
        if let OutputState::Waiting(task) = &mut self.state {
            self.state = match task.await {
                Ok(nwritten) => {
                    if let FileOutputMode::Position(ref mut p) = &mut self.mode {
                        *p += nwritten as u64;
                    }
                    OutputState::Ready
                }
                Err(e) => OutputState::Error(e),
            };
        }
    }
}

pub struct ReaddirIterator(
    std::sync::Mutex<Box<dyn Iterator<Item = FsResult<types::DirectoryEntry>> + Send + 'static>>,
);

impl ReaddirIterator {
    pub(crate) fn new(
        i: impl Iterator<Item = FsResult<types::DirectoryEntry>> + Send + 'static,
    ) -> Self {
        ReaddirIterator(std::sync::Mutex::new(Box::new(i)))
    }
    pub(crate) fn next(&self) -> FsResult<Option<types::DirectoryEntry>> {
        self.0.lock().unwrap().next().transpose()
    }
}

impl IntoIterator for ReaddirIterator {
    type Item = FsResult<types::DirectoryEntry>;
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + Send>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_inner().unwrap()
    }
}

/// A 128-bit number that uniquely identifies a file on the system.
///
/// Warning! This should be treated as sensitive information and only be
/// exposed to the WASM guest in hashed form.
#[derive(Hash, PartialEq, Eq)]
struct FileId {
    a: u64,
    b: u64,
}
impl FileId {
    fn from_metadata(meta: &cap_fs_ext::Metadata) -> Self {
        use cap_fs_ext::MetadataExt;

        Self {
            a: meta.ino(),
            b: meta.dev(),
        }
    }

    fn from_ptr<T>(ptr: *const T) -> Self {
        Self {
            a: ptr as u64,
            b: u64::MAX,
        }
    }

    fn hash(&self) -> types::MetadataHashValue {
        // Without incurring any deps, std provides us with a 64 bit hash
        // function:
        use std::hash::Hasher;
        // Note that this means that the metadata hash (which becomes a preview1 ino) may
        // change when a different rustc release is used to build this host implementation:
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write_u64(self.a);
        hasher.write_u64(self.b);
        let lower = hasher.finish();
        // MetadataHashValue has a pair of 64-bit members for representing a
        // single 128-bit number. However, we only have 64 bits of entropy. To
        // synthesize the upper 64 bits, lets xor the lower half with an arbitrary
        // constant, in this case the 64 bit integer corresponding to the IEEE
        // double representation of (a number as close as possible to) pi.
        // This seems better than just repeating the same bits in the upper and
        // lower parts outright, which could make folks wonder if the struct was
        // mangled in the ABI, or worse yet, lead to consumers of this interface
        // expecting them to be equal.
        let upper = lower ^ 4614256656552045848u64;
        types::MetadataHashValue { lower, upper }
    }
}

fn descriptortype_from(ft: cap_std::fs::FileType) -> types::DescriptorType {
    use cap_fs_ext::FileTypeExt;
    use types::DescriptorType;
    if ft.is_dir() {
        DescriptorType::Directory
    } else if ft.is_symlink() {
        DescriptorType::SymbolicLink
    } else if ft.is_block_device() {
        DescriptorType::BlockDevice
    } else if ft.is_char_device() {
        DescriptorType::CharacterDevice
    } else if ft.is_file() {
        DescriptorType::RegularFile
    } else {
        DescriptorType::Unknown
    }
}

fn systemtimespec_from(t: types::NewTimestamp) -> FsResult<Option<fs_set_times::SystemTimeSpec>> {
    use fs_set_times::SystemTimeSpec;
    use types::NewTimestamp;
    match t {
        NewTimestamp::NoChange => Ok(None),
        NewTimestamp::Now => Ok(Some(SystemTimeSpec::SymbolicNow)),
        NewTimestamp::Timestamp(st) => Ok(Some(SystemTimeSpec::Absolute(systemtime_from(st)?))),
    }
}

fn systemtime_from(t: wall_clock::Datetime) -> FsResult<std::time::SystemTime> {
    use std::time::{Duration, SystemTime};
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::new(t.seconds, t.nanoseconds))
        .ok_or_else(|| ErrorCode::Overflow.into())
}

fn datetime_from(t: std::time::SystemTime) -> wall_clock::Datetime {
    // FIXME make this infallible or handle errors properly
    wall_clock::Datetime::try_from(cap_std::time::SystemTime::from_std(t)).unwrap()
}

fn descriptorstat_from(meta: cap_std::fs::Metadata) -> types::DescriptorStat {
    use cap_fs_ext::MetadataExt;
    types::DescriptorStat {
        type_: descriptortype_from(meta.file_type()),
        link_count: meta.nlink(),
        size: meta.len(),
        data_access_timestamp: meta.accessed().map(|t| datetime_from(t.into_std())).ok(),
        data_modification_timestamp: meta.modified().map(|t| datetime_from(t.into_std())).ok(),
        status_change_timestamp: meta.created().map(|t| datetime_from(t.into_std())).ok(),
    }
}

fn descriptor_flags_from_fdflags(flags: FdFlags) -> types::DescriptorFlags {
    let mut out = types::DescriptorFlags::empty();
    if flags.contains(FdFlags::DSYNC) {
        out |= types::DescriptorFlags::REQUESTED_WRITE_SYNC;
    }
    if flags.contains(FdFlags::RSYNC) {
        out |= types::DescriptorFlags::DATA_INTEGRITY_SYNC;
    }
    if flags.contains(FdFlags::SYNC) {
        out |= types::DescriptorFlags::FILE_INTEGRITY_SYNC;
    }
    out
}
