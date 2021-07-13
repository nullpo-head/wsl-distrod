use crate::distro_image::{DefaultImageFetcher, DistroImageFetcher, DistroImageList};
use anyhow::{bail, Context, Result};
use colored::*;
use std::io::Write;

pub fn choose_from_list(list: DistroImageList) -> Result<Box<dyn DistroImageFetcher>> {
    match list {
        DistroImageList::Fetcher(list_item_kind, fetchers, default) => {
            if fetchers.is_empty() {
                bail!("Empty list of {}.", &list_item_kind);
            }
            let default = match default {
                DefaultImageFetcher::Index(index) => fetchers[index].get_name().to_owned(),
                DefaultImageFetcher::Name(name) => name,
            };
            for (i, fetcher) in fetchers.iter().enumerate() {
                println!("{} {}", format!("[{}]", i + 1).cyan(), fetcher.get_name());
            }
            log::info!("Choose {} from the list above.", &list_item_kind);
            loop {
                log::info!("Type the name or the index of your choice.");
                print!("[Default: {}]: ", &default);
                let _ = std::io::stdout().flush();
                let mut choice = String::new();
                std::io::stdin()
                    .read_line(&mut choice)
                    .with_context(|| "failed to read from the stdin.")?;
                choice = choice.trim_end().to_owned();
                if choice.is_empty() {
                    choice = default.to_owned();
                }
                let index = fetchers
                    .iter()
                    .position(|fetcher| fetcher.get_name() == choice.as_str());
                if let Some(index) = index {
                    return Ok(fetchers.into_iter().nth(index).unwrap());
                }
                if let Ok(index) = choice.parse::<usize>() {
                    if index <= fetchers.len() && index >= 1 {
                        return Ok(fetchers.into_iter().nth(index - 1).unwrap());
                    }
                }
                log::info!("{} is off the list.", choice);
            }
        }
        DistroImageList::Image(_) => bail!("Image should not be passed to choose_from_list."),
    }
}
