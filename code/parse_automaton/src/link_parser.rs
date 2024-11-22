use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
#[allow(unused_imports)]
use colored::Colorize;
use select::document::Document;
use select::predicate::Name;
use url::Url;

pub struct LinkParser;

impl LinkParser {
    pub fn parse_links(
        html: &Document,
        target_base_url: &Url,
        excluded_links: &Vec<String>,
    ) -> Result<HashSet<Url>> {
        let mut links_found = HashSet::<Url>::new();
        println!("[parser] started");
        println!("[parser] excluded_links: {:#?}", excluded_links);
        for node in html.find(Name("a")) {
            let Some(href) = node.attr("href") else {
                continue;
            };
            let url = match href.chars().next() {
                Some(char) => match char {
                    '/' => {
                        let mut url = target_base_url.clone();
                        url.set_path(href);
                        url
                    }
                    _ => {
                        let Ok(url) = Url::parse(href) else {
                            continue;
                        };
                        url
                    }
                },
                None => continue,
            };
            match url.domain() {
                Some(_domain) => {
                    let scheme = url.scheme();
                    let filepath = PathBuf::from(url.path());
                    match filepath.extension() {
                        Some(ext) => {
                            if ext != "html" && ext != "htm" {
                                println!("LOOK AT DIS: {}", ext.to_str().unwrap());
                                continue;
                            }
                        }
                        None => (),
                    }
                    if scheme != "https" && scheme != "http" {
                        continue;
                    }
                }
                None => continue,
            }
            if !url.as_str().starts_with(target_base_url.as_str()) {
                continue;
            }
            if excluded_links
                .iter()
                .any(|excluded_link| url.as_str().starts_with(excluded_link))
            {
                continue;
            }
            let mut new_link = url.as_str();
            // let link_without_query = &link_string[0..link_string.find("?").unwrap_or(link_string.len())];
            if let Some((link_without_query, _)) = new_link.split_once("?") {
                new_link = link_without_query;
            }
            if let Some((link_without_query, _)) = new_link.split_once("%3F") {
                new_link = link_without_query;
            }
            new_link = new_link
                .strip_suffix("\r\n")
                .or(new_link.strip_suffix("\n"))
                .unwrap_or(new_link)
                .trim_end_matches('/');
            let inner_url = Url::parse(new_link)?;
            links_found.insert(inner_url);
        }
        println!("[parser] ended");
        Ok(links_found)
    }
}
