use crate::bindings::filesystem::preopens;
use crate::bindings::filesystem::types::{
    self, ErrorCode, HostDescriptor, HostDirectoryEntryStream,
};
use crate::bindings::io::streams::{InputStream, OutputStream};
use crate::filesystem::Descriptor;
use crate::{FsError, FsResult, WasiImpl, WasiView};
use anyhow::Context;
use wasmtime::component::Resource;

mod sync;

impl<T> preopens::Host for WasiImpl<T>
where
    T: WasiView,
{
    fn get_directories(
        &mut self,
    ) -> Result<Vec<(Resource<types::Descriptor>, String)>, anyhow::Error> {
        let mut results = Vec::new();
        for (dir, name) in self.ctx().preopens.clone() {
            let fd = self
                .table()
                .push(Descriptor::Dir(dir))
                .with_context(|| format!("failed to push preopen {name}"))?;
            results.push((fd, name));
        }
        Ok(results)
    }
}

#[async_trait::async_trait]
impl<T> types::Host for WasiImpl<T>
where
    T: WasiView,
{
    fn convert_error_code(&mut self, err: FsError) -> anyhow::Result<ErrorCode> {
        err.downcast()
    }

    fn filesystem_error_code(
        &mut self,
        err: Resource<anyhow::Error>,
    ) -> anyhow::Result<Option<ErrorCode>> {
        let err = self.table().get(&err)?;

        // Currently `err` always comes from the stream implementation which
        // uses standard reads/writes so only check for `std::io::Error` here.
        if let Some(err) = err.downcast_ref::<std::io::Error>() {
            return Ok(Some(ErrorCode::from(err)));
        }

        Ok(None)
    }
}

#[async_trait::async_trait]
impl<T> HostDescriptor for WasiImpl<T>
where
    T: WasiView,
{
    async fn advise(
        &mut self,
        fd: Resource<types::Descriptor>,
        offset: types::Filesize,
        len: types::Filesize,
        advice: types::Advice,
    ) -> FsResult<()> {
        self.table().get(&fd)?.advise(offset, len, advice).await
    }

    async fn sync_data(&mut self, fd: Resource<types::Descriptor>) -> FsResult<()> {
        self.table().get(&fd)?.sync_data().await
    }

    async fn get_flags(
        &mut self,
        fd: Resource<types::Descriptor>,
    ) -> FsResult<types::DescriptorFlags> {
        self.table().get(&fd)?.get_flags().await
    }

    async fn get_type(
        &mut self,
        fd: Resource<types::Descriptor>,
    ) -> FsResult<types::DescriptorType> {
        self.table().get(&fd)?.get_type().await
    }

    async fn set_size(
        &mut self,
        fd: Resource<types::Descriptor>,
        size: types::Filesize,
    ) -> FsResult<()> {
        self.table().get(&fd)?.set_size(size).await
    }

    async fn set_times(
        &mut self,
        fd: Resource<types::Descriptor>,
        atim: types::NewTimestamp,
        mtim: types::NewTimestamp,
    ) -> FsResult<()> {
        self.table().get(&fd)?.set_times(atim, mtim).await
    }

    async fn read(
        &mut self,
        fd: Resource<types::Descriptor>,
        len: types::Filesize,
        offset: types::Filesize,
    ) -> FsResult<(Vec<u8>, bool)> {
        self.table().get(&fd)?.read(len, offset).await
    }

    async fn write(
        &mut self,
        fd: Resource<types::Descriptor>,
        buf: Vec<u8>,
        offset: types::Filesize,
    ) -> FsResult<types::Filesize> {
        self.table().get(&fd)?.write(buf, offset).await
    }

    async fn read_directory(
        &mut self,
        fd: Resource<types::Descriptor>,
    ) -> FsResult<Resource<types::DirectoryEntryStream>> {
        let table = self.table();
        Ok(table.push(table.get(&fd)?.read_directory().await?)?)
    }

    async fn sync(&mut self, fd: Resource<types::Descriptor>) -> FsResult<()> {
        self.table().get(&fd)?.sync().await
    }

    async fn create_directory_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        path: String,
    ) -> FsResult<()> {
        self.table().get(&fd)?.create_directory_at(path).await
    }

    async fn stat(&mut self, fd: Resource<types::Descriptor>) -> FsResult<types::DescriptorStat> {
        self.table().get(&fd)?.stat().await
    }

    async fn stat_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FsResult<types::DescriptorStat> {
        self.table().get(&fd)?.stat_at(path_flags, path).await
    }

    async fn set_times_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        atim: types::NewTimestamp,
        mtim: types::NewTimestamp,
    ) -> FsResult<()> {
        self.table()
            .get(&fd)?
            .set_times_at(path_flags, path, atim, mtim)
            .await
    }

    async fn link_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        // TODO delete the path flags from this function
        old_path_flags: types::PathFlags,
        old_path: String,
        new_descriptor: Resource<types::Descriptor>,
        new_path: String,
    ) -> FsResult<()> {
        let table = self.table();
        let old_descriptor = table.get(&fd)?;
        let new_descriptor = table.get(&new_descriptor)?;

        Descriptor::link_at(
            old_descriptor,
            old_path_flags,
            old_path,
            new_descriptor,
            new_path,
        )
        .await
    }

    async fn open_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        oflags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FsResult<Resource<types::Descriptor>> {
        let table = self.table();
        Ok(table.push(
            table
                .get(&fd)?
                .open_at(path_flags, path, oflags, flags)
                .await?,
        )?)
    }

    fn drop(&mut self, fd: Resource<types::Descriptor>) -> anyhow::Result<()> {
        let table = self.table();

        // The Drop will close the file/dir, but if the close syscall
        // blocks the thread, I will face god and walk backwards into hell.
        // tokio::fs::File just uses std::fs::File's Drop impl to close, so
        // it doesn't appear anyone else has found this to be a problem.
        // (Not that they could solve it without async drop...)
        table.delete(fd)?;

        Ok(())
    }

    async fn readlink_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        path: String,
    ) -> FsResult<String> {
        self.table().get(&fd)?.readlink_at(path).await
    }

    async fn remove_directory_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        path: String,
    ) -> FsResult<()> {
        self.table().get(&fd)?.remove_directory_at(path).await
    }

    async fn rename_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        old_path: String,
        new_fd: Resource<types::Descriptor>,
        new_path: String,
    ) -> FsResult<()> {
        let table = self.table();
        let old_fd = table.get(&fd)?;
        let new_fd = table.get(&new_fd)?;

        Descriptor::rename_at(old_fd, old_path, new_fd, new_path).await
    }

    async fn symlink_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        src_path: String,
        dest_path: String,
    ) -> FsResult<()> {
        self.table().get(&fd)?.symlink_at(src_path, dest_path).await
    }

    async fn unlink_file_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        path: String,
    ) -> FsResult<()> {
        self.table().get(&fd)?.unlink_file_at(path).await
    }

    fn read_via_stream(
        &mut self,
        fd: Resource<types::Descriptor>,
        offset: types::Filesize,
    ) -> FsResult<Resource<InputStream>> {
        let table = self.table();
        Ok(table.push(table.get(&fd)?.read_via_stream(offset)?)?)
    }

    fn write_via_stream(
        &mut self,
        fd: Resource<types::Descriptor>,
        offset: types::Filesize,
    ) -> FsResult<Resource<OutputStream>> {
        let table = self.table();
        Ok(table.push(table.get(&fd)?.write_via_stream(offset)?)?)
    }

    fn append_via_stream(
        &mut self,
        fd: Resource<types::Descriptor>,
    ) -> FsResult<Resource<OutputStream>> {
        let table = self.table();
        Ok(table.push(table.get(&fd)?.append_via_stream()?)?)
    }

    async fn is_same_object(
        &mut self,
        a: Resource<types::Descriptor>,
        b: Resource<types::Descriptor>,
    ) -> anyhow::Result<bool> {
        let table = self.table();
        types::Descriptor::is_same_object(table.get(&a)?, table.get(&b)?).await
    }
    async fn metadata_hash(
        &mut self,
        fd: Resource<types::Descriptor>,
    ) -> FsResult<types::MetadataHashValue> {
        self.table().get(&fd)?.metadata_hash().await
    }
    async fn metadata_hash_at(
        &mut self,
        fd: Resource<types::Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FsResult<types::MetadataHashValue> {
        self.table()
            .get(&fd)?
            .metadata_hash_at(path_flags, path)
            .await
    }
}

#[async_trait::async_trait]
impl<T> HostDirectoryEntryStream for WasiImpl<T>
where
    T: WasiView,
{
    async fn read_directory_entry(
        &mut self,
        stream: Resource<types::DirectoryEntryStream>,
    ) -> FsResult<Option<types::DirectoryEntry>> {
        let table = self.table();
        let readdir = table.get(&stream)?;
        readdir.next()
    }

    fn drop(&mut self, stream: Resource<types::DirectoryEntryStream>) -> anyhow::Result<()> {
        self.table().delete(stream)?;
        Ok(())
    }
}

#[cfg(unix)]
fn from_raw_os_error(err: Option<i32>) -> Option<ErrorCode> {
    use rustix::io::Errno as RustixErrno;
    if err.is_none() {
        return None;
    }
    Some(match RustixErrno::from_raw_os_error(err.unwrap()) {
        RustixErrno::PIPE => ErrorCode::Pipe,
        RustixErrno::PERM => ErrorCode::NotPermitted,
        RustixErrno::NOENT => ErrorCode::NoEntry,
        RustixErrno::NOMEM => ErrorCode::InsufficientMemory,
        RustixErrno::IO => ErrorCode::Io,
        RustixErrno::BADF => ErrorCode::BadDescriptor,
        RustixErrno::BUSY => ErrorCode::Busy,
        RustixErrno::ACCESS => ErrorCode::Access,
        RustixErrno::NOTDIR => ErrorCode::NotDirectory,
        RustixErrno::ISDIR => ErrorCode::IsDirectory,
        RustixErrno::INVAL => ErrorCode::Invalid,
        RustixErrno::EXIST => ErrorCode::Exist,
        RustixErrno::FBIG => ErrorCode::FileTooLarge,
        RustixErrno::NOSPC => ErrorCode::InsufficientSpace,
        RustixErrno::SPIPE => ErrorCode::InvalidSeek,
        RustixErrno::MLINK => ErrorCode::TooManyLinks,
        RustixErrno::NAMETOOLONG => ErrorCode::NameTooLong,
        RustixErrno::NOTEMPTY => ErrorCode::NotEmpty,
        RustixErrno::LOOP => ErrorCode::Loop,
        RustixErrno::OVERFLOW => ErrorCode::Overflow,
        RustixErrno::ILSEQ => ErrorCode::IllegalByteSequence,
        RustixErrno::NOTSUP => ErrorCode::Unsupported,
        RustixErrno::ALREADY => ErrorCode::Already,
        RustixErrno::INPROGRESS => ErrorCode::InProgress,
        RustixErrno::INTR => ErrorCode::Interrupted,

        // On some platforms, these have the same value as other errno values.
        #[allow(unreachable_patterns)]
        RustixErrno::OPNOTSUPP => ErrorCode::Unsupported,

        _ => return None,
    })
}
#[cfg(windows)]
fn from_raw_os_error(raw_os_error: Option<i32>) -> Option<ErrorCode> {
    use windows_sys::Win32::Foundation;
    Some(match raw_os_error.map(|code| code as u32) {
        Some(Foundation::ERROR_FILE_NOT_FOUND) => ErrorCode::NoEntry,
        Some(Foundation::ERROR_PATH_NOT_FOUND) => ErrorCode::NoEntry,
        Some(Foundation::ERROR_ACCESS_DENIED) => ErrorCode::Access,
        Some(Foundation::ERROR_SHARING_VIOLATION) => ErrorCode::Access,
        Some(Foundation::ERROR_PRIVILEGE_NOT_HELD) => ErrorCode::NotPermitted,
        Some(Foundation::ERROR_INVALID_HANDLE) => ErrorCode::BadDescriptor,
        Some(Foundation::ERROR_INVALID_NAME) => ErrorCode::NoEntry,
        Some(Foundation::ERROR_NOT_ENOUGH_MEMORY) => ErrorCode::InsufficientMemory,
        Some(Foundation::ERROR_OUTOFMEMORY) => ErrorCode::InsufficientMemory,
        Some(Foundation::ERROR_DIR_NOT_EMPTY) => ErrorCode::NotEmpty,
        Some(Foundation::ERROR_NOT_READY) => ErrorCode::Busy,
        Some(Foundation::ERROR_BUSY) => ErrorCode::Busy,
        Some(Foundation::ERROR_NOT_SUPPORTED) => ErrorCode::Unsupported,
        Some(Foundation::ERROR_FILE_EXISTS) => ErrorCode::Exist,
        Some(Foundation::ERROR_BROKEN_PIPE) => ErrorCode::Pipe,
        Some(Foundation::ERROR_BUFFER_OVERFLOW) => ErrorCode::NameTooLong,
        Some(Foundation::ERROR_NOT_A_REPARSE_POINT) => ErrorCode::Invalid,
        Some(Foundation::ERROR_NEGATIVE_SEEK) => ErrorCode::Invalid,
        Some(Foundation::ERROR_DIRECTORY) => ErrorCode::NotDirectory,
        Some(Foundation::ERROR_ALREADY_EXISTS) => ErrorCode::Exist,
        Some(Foundation::ERROR_STOPPED_ON_SYMLINK) => ErrorCode::Loop,
        Some(Foundation::ERROR_DIRECTORY_NOT_SUPPORTED) => ErrorCode::IsDirectory,
        _ => return None,
    })
}

impl From<std::io::Error> for ErrorCode {
    fn from(err: std::io::Error) -> ErrorCode {
        ErrorCode::from(&err)
    }
}

impl<'a> From<&'a std::io::Error> for ErrorCode {
    fn from(err: &'a std::io::Error) -> ErrorCode {
        match from_raw_os_error(err.raw_os_error()) {
            Some(errno) => errno,
            None => {
                tracing::debug!("unknown raw os error: {err}");
                match err.kind() {
                    std::io::ErrorKind::NotFound => ErrorCode::NoEntry,
                    std::io::ErrorKind::PermissionDenied => ErrorCode::NotPermitted,
                    std::io::ErrorKind::AlreadyExists => ErrorCode::Exist,
                    std::io::ErrorKind::InvalidInput => ErrorCode::Invalid,
                    _ => ErrorCode::Io,
                }
            }
        }
    }
}

impl From<cap_rand::Error> for ErrorCode {
    fn from(err: cap_rand::Error) -> ErrorCode {
        // I picked Error::Io as a 'reasonable default', FIXME dan is this ok?
        from_raw_os_error(err.raw_os_error()).unwrap_or(ErrorCode::Io)
    }
}

impl From<std::num::TryFromIntError> for ErrorCode {
    fn from(_err: std::num::TryFromIntError) -> ErrorCode {
        ErrorCode::Overflow
    }
}

#[cfg(test)]
mod test {
    use crate::filesystem::ReaddirIterator;
    use wasmtime::component::ResourceTable;

    #[test]
    fn table_readdir_works() {
        let mut table = ResourceTable::new();
        let ix = table
            .push(ReaddirIterator::new(std::iter::empty()))
            .unwrap();
        let _ = table.get(&ix).unwrap();
        table.delete(ix).unwrap();
    }
}
