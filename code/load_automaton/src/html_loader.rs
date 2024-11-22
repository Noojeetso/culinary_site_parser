use anyhow::Result;
use colored::Colorize;
use reqwest;
use url::Url;

use fs_err as fs;
use std::path::{Path, PathBuf};

pub trait HtmlLoader {
    fn new(store_path: Option<&Path>) -> Self;
    async fn load(&self, url: &Url) -> Result<String>;
}

pub struct HtmlDownloader {
    download_path: Option<PathBuf>,
}

impl HtmlLoader for HtmlDownloader {
    fn new(download_path: Option<&Path>) -> Self {
        Self {
            download_path: download_path.map(|p| p.to_owned()),
        }
    }
    async fn load(&self, url: &Url) -> Result<String> {
        let resp = reqwest::get(url.clone()).await.unwrap();
        let html_doc_string = resp.text().await.unwrap();
        if let Some(search_path) = &self.download_path {
            let domain = url.domain().unwrap();
            println!("{} {}", "domain:".green().bold(), domain);
            let resource_path =
                PathBuf::from(url.path().strip_prefix(std::path::MAIN_SEPARATOR).unwrap());
            println!(
                "{} {}",
                "resource_path:".green().bold(),
                resource_path.display()
            );
            let (resource_dir, resource_filename) = match resource_path.extension() {
                Some(_) => {
                    let resource_dir = match resource_path.parent() {
                        Some(resource_dir) => resource_dir.to_owned(),
                        None => PathBuf::from(""),
                    };
                    let resource_filename = PathBuf::from(resource_path.file_name().unwrap());
                    (resource_dir, resource_filename)
                }
                None => {
                    let resource_dir = resource_path;
                    let resource_filename = PathBuf::from("index.html");
                    (resource_dir, resource_filename)
                }
            };
            println!(
                "{} {}",
                "resource_dir:".green().bold(),
                resource_dir.display()
            );
            println!(
                "{} {}",
                "resource_filename:".green().bold(),
                resource_filename.display()
            );
            let save_dir = search_path
                .join(PathBuf::from(domain))
                .join(PathBuf::from(resource_dir));
            fs::create_dir_all(&save_dir).unwrap();
            let download_path = save_dir.join(PathBuf::from(resource_filename));
            println!("{} {:#?}", "download_path:".green().bold(), download_path);
            fs::write(&download_path, &html_doc_string)?;
        };
        Ok(html_doc_string)
    }
}

pub struct HtmLoaderFS {
    search_path: Option<PathBuf>,
}

impl HtmlLoader for HtmLoaderFS {
    fn new(search_path: Option<&Path>) -> Self {
        Self {
            search_path: search_path.map(|p| p.to_owned()),
        }
    }
    async fn load(&self, url: &Url) -> Result<String> {
        let domain = PathBuf::from(url.domain().unwrap());
        let url_path = PathBuf::from(url.path());
        let dir_path = url_path.strip_prefix("/").unwrap();
        println!("dir_path: {}", dir_path.display());

        let mut relative_path = match &self.search_path {
            Some(search_path) => search_path.join(domain).join(dir_path),
            None => PathBuf::from("downloads").join(domain).join(dir_path),
        };

        match dir_path.extension() {
            Some(_) => (),
            None => relative_path = relative_path.join(PathBuf::from("index.html")),
        }

        println!("relative_path: {}", relative_path.display());

        let html_doc_string = fs::read_to_string(relative_path)?;
        Ok(html_doc_string)
    }
}
