use crate::error::SlkrdError;
use bytes::BytesMut;
use std::path::Path;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct FileManager {
    file: File,
    chunk_size: usize,
}

impl FileManager {
    pub async fn new_reader<P: AsRef<Path>>(path: P, chunk_size: usize) -> Result<Self, SlkrdError> {
        let file = File::open(path).await.map_err(SlkrdError::Io)?;
        Ok(Self { file, chunk_size })
    }

    pub async fn new_writer<P: AsRef<Path>>(path: P, chunk_size: usize) -> Result<Self, SlkrdError> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(path)
            .await
            .map_err(SlkrdError::Io)?;
        Ok(Self { file, chunk_size })
    }

    pub async fn read_chunk(&mut self) -> Result<Option<BytesMut>, SlkrdError> {
        let mut buffer = BytesMut::with_capacity(self.chunk_size);
        let n = self
            .file
            .read_buf(&mut buffer)
            .await
            .map_err(SlkrdError::Io)?;
        if n == 0 {
            Ok(None)
        } else {
            Ok(Some(buffer))
        }
    }

    pub async fn write_chunk(&mut self, chunk: BytesMut) -> Result<(), SlkrdError> {
        self.file
            .write_all(&chunk)
            .await
            .map_err(SlkrdError::Io)?;
        Ok(())
    }
}
