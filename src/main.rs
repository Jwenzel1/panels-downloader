use anyhow::{bail, Context, Result};
use bytes::Bytes;
use reqwest::Client;
use serde::Deserialize;
use std::cmp::max;
use std::ops::Index;
use std::{collections::HashMap, fs::create_dir_all, path::PathBuf};
use tokio::sync::mpsc::unbounded_channel;
use tokio::{fs::File, io::AsyncWriteExt, task::JoinSet};

#[derive(Deserialize, Debug, Clone)]
pub struct ManifestData {
    #[serde(alias = "as")]
    pub _as: Option<String>,
    pub am: Option<String>,
    pub dhd: Option<String>,
    pub dsd: Option<String>,
    pub e: Option<String>,
    pub fs: Option<String>,
    pub s: Option<String>,
    pub wcl0: Option<String>,
    pub wcl1: Option<String>,
    pub wcl2: Option<String>,
    pub wcs0: Option<String>,
    pub wcs1: Option<String>,
    pub wcs2: Option<String>,
    pub wfs: Option<String>,
    pub wft: Option<String>,
}

impl ManifestData {
    fn is_wallpaper(&self) -> bool {
        self.dhd.is_some() || self.dsd.is_some()
    }

    fn wallpaper_url(&self) -> Result<&str> {
        let wallpaper_url_str = if self.is_wallpaper() {
            self.dhd
                .as_ref()
                .unwrap_or_else(|| self.dsd.as_ref().unwrap())
                .as_str()
        } else {
            bail!("ManifestData does not contain wallpaper data")
        };
        Ok(wallpaper_url_str)
    }

    async fn download_wallpaper(&self, client: &Client) -> Result<Bytes> {
        client
            .get(self.wallpaper_url().unwrap())
            .send()
            .await
            .context("Failed to download wallpaper")?
            .bytes()
            .await
            .context("Failed to recieve data from the server")
    }
}

#[derive(Deserialize, Debug)]
pub struct Manifest {
    pub version: u8,
    pub data: HashMap<String, ManifestData>,
}

impl Manifest {
    async fn get(domain: &str) -> Result<Self> {
        let manifest_url = format!("{}/panels-api/data/20240916/media-1a-i-p~s", domain);
        let response = reqwest::get(manifest_url)
            .await
            .context("Unable to retrieve panels manifest data")?
            .json::<Self>()
            .await
            .context("Unable to parse the manifest json")?;
        Ok(response)
    }

    fn wallpapers(&self) -> Vec<&ManifestData> {
        self.data.values().filter(|&w| w.is_wallpaper()).collect()
    }
}

pub struct App {
    panels_domain: String,
    download_directory: PathBuf,
    workers: usize,
}

impl App {
    fn new(panels_domain: &str, download_directory: &str, workers: usize) -> Self {
        let workers = max(workers, 1);
        Self {
            panels_domain: String::from(panels_domain),
            download_directory: PathBuf::from(download_directory),
            workers,
        }
    }

    async fn run(&self) -> Result<()> {
        create_dir_all(&self.download_directory).context(
            "Failed to make download directory. Please make sure you have write permissions",
        )?;
        let manifest = Manifest::get(&self.panels_domain).await?;
        let wallpapers = manifest.wallpapers();
        let mut senders = Vec::with_capacity(self.workers);
        let mut recievers = Vec::with_capacity(self.workers);
        for _ in 0..self.workers {
            let (sender, reciever) = unbounded_channel::<ManifestData>();
            senders.push(sender);
            recievers.push(reciever);
        }
        for (i, wallpaper) in wallpapers.into_iter().enumerate() {
            senders
                .index(i % self.workers)
                .send(wallpaper.clone())
                .context("Failed to send ManifestData through channel")?;
        }
        let mut futures = JoinSet::new();
        for (thread_number, mut reciever) in recievers.into_iter().enumerate() {
            let download_dir = self.download_directory.clone();
            futures.spawn(async move {
                reciever.close();
                let mut count = 0;
                let client = Client::new();
                while let Some(wallpaper) = reciever.recv().await {
                    let mut wallpaper_path = download_dir.clone();
                    wallpaper_path.push(format!("{thread_number}_{count}"));
                    wallpaper_path.set_extension("jpg");
                    count += 1;
                    let Ok(wallpaper_bytes) = wallpaper.download_wallpaper(&client).await else {
                        continue;
                    };
                    let Ok(mut file_handle) = File::create_new(wallpaper_path).await else {
                        continue;
                    };
                    if let Err(_) = file_handle.write_all(&wallpaper_bytes).await {
                        continue;
                    }
                    if let Err(_) = file_handle.flush().await {
                        continue;
                    }
                }
            });
        }
        futures.join_all().await;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let app = App::new("http://localhost:8080", "wallpapers", 10);
    app.run().await
}
