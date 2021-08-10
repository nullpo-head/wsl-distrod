use crate::distro_image::{DefaultImageFetcher, DistroImageFetcher, DistroImageList};
use anyhow::{bail, Context, Result};
use colored::*;
use std::{ffi::OsString, io::Write, str::FromStr};
use strum::{EnumString, EnumVariantNames};

#[derive(Copy, Clone, Debug, EnumString, EnumVariantNames)]
#[strum(serialize_all = "kebab-case")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

pub fn init_logger(app_name: String, log_level: LogLevel) {
    let mut env_logger_builder = env_logger::Builder::new();

    env_logger_builder.filter_level(
        log::LevelFilter::from_str(<LogLevel as strum::VariantNames>::VARIANTS[log_level as usize])
            .unwrap(),
    );

    env_logger_builder.format(move |buf, record| {
        writeln!(
            buf,
            "{}{} {}",
            format!("[{}]", app_name).bright_green(),
            match record.level() {
                log::Level::Info => "".to_string(),
                log::Level::Error | log::Level::Warn =>
                    format!("[{}]", record.level()).red().to_string(),
                _ => format!("[{}]", record.level()).bright_green().to_string(),
            },
            record.args()
        )
    });
    env_logger_builder.init();
}

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

pub fn prompt_path(message: &str, default: Option<&str>) -> Result<OsString> {
    log::info!("{}", message);
    print!(
        "[Input the path{}]: ",
        default
            .map(|s| format!(" (Default: '{}')", s))
            .unwrap_or_else(|| "".to_owned())
    );
    let _ = std::io::stdout().flush();
    let mut choice = String::new();
    std::io::stdin()
        .read_line(&mut choice)
        .with_context(|| "failed to read from the stdin.")?;
    choice = choice.trim_end().to_owned();
    Ok(OsString::from(choice))
}

pub fn prompt_string(message: &str, target_name: &str, default: Option<&str>) -> Result<String> {
    log::info!("{}", message);
    print!(
        "[Input {}{}]: ",
        target_name,
        default
            .map(|s| format!(" (Default: '{}')", s))
            .unwrap_or_else(|| "".to_owned())
    );
    let _ = std::io::stdout().flush();
    let mut choice = String::new();
    std::io::stdin()
        .read_line(&mut choice)
        .with_context(|| "failed to read from the stdin.")?;
    choice = choice.trim_end().to_owned();
    Ok(choice)
}
