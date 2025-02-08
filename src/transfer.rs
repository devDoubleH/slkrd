use crate::error::SlkrdError;
use crate::file::FileManager;
use crate::network::NetworkManager;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

pub struct Transfer {
    network: NetworkManager,
    file_manager: FileManager,
    progress: ProgressBar,
}

impl Transfer {
    pub async fn new_sender(
        path: PathBuf,
        chunk_size: usize,
        total_size: u64,
    ) -> Result<Self, SlkrdError> {
        let network = NetworkManager::new(&Default::default()).await?;
        let file_manager = FileManager::new_reader(path, chunk_size).await?;
        let progress = create_progress_bar(total_size);

        Ok(Self {
            network,
            file_manager,
            progress,
        })
    }

    pub async fn new_receiver(
        path: PathBuf,
        chunk_size: usize,
        total_size: u64,
    ) -> Result<Self, SlkrdError> {
        let network = NetworkManager::new(&Default::default()).await?;
        let file_manager = FileManager::new_writer(path, chunk_size).await?;
        let progress = create_progress_bar(total_size);

        Ok(Self {
            network,
            file_manager,
            progress,
        })
    }
}

fn create_progress_bar(total_size: u64) -> ProgressBar {
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap(),
    );
    pb
}