use crate::distro_image::{DefaultImageFetcher, DistroImageFetcher, DistroImageList};
use anyhow::{bail, Context, Result};
use colored::*;
use std::{ffi::OsString, fmt::Debug, io::Write};
use tracing::metadata::LevelFilter;
use tracing_subscriber::{fmt::FormatEvent, prelude::*};

#[derive(Default, Debug)]
pub struct LoggerInitializer {
    logs_kmsg: bool,
    log_level: Option<String>,
    kmsg_log_level: Option<String>,
}

impl LoggerInitializer {
    pub fn with_kmsg(&mut self, logs_kmsg: bool) -> &mut Self {
        self.logs_kmsg = logs_kmsg;
        self
    }

    pub fn with_log_level(&mut self, log_level: String) -> &mut Self {
        self.log_level = Some(log_level);
        self
    }

    pub fn with_kmsg_log_level(&mut self, log_level: String) -> &mut Self {
        self.kmsg_log_level = Some(log_level);
        self
    }

    pub fn init(self, app_name: String) {
        let inner = || -> Result<()> {
            let terminal_formatter = TerminalLogFormatter::new(app_name.clone());
            let mut terminal_filter =
                tracing_subscriber::filter::Targets::new().with_default(LevelFilter::INFO);
            if let Some(target) = self
                .log_level
                .or_else(|| std::env::var("RUST_LOG").ok())
                .and_then(|level| {
                    level
                        .parse()
                        .map_err(|e| {
                            eprintln!("Invalid log level format {:?}", e);
                            e
                        })
                        .ok()
                })
            {
                terminal_filter = target;
            };
            let terminal_fmt_layer = tracing_subscriber::fmt::layer()
                .with_target(false)
                .event_format(terminal_formatter)
                .with_writer(std::io::stderr)
                .with_filter(terminal_filter);

            if !self.logs_kmsg {
                tracing::subscriber::set_global_default(
                    tracing_subscriber::registry().with(terminal_fmt_layer),
                )
                .with_context(|| "set_global_default failed.")?;
                tracing_log::LogTracer::init()
                    .with_context(|| "Failed to init LogTracer with terminal logger.")?;
                return Ok(());
            }

            let kmsg_formatter = KmsgLogFormatter::new(app_name);
            let mut kmsg_filter =
                tracing_subscriber::filter::Targets::new().with_default(LevelFilter::ERROR);
            if let Some(target) = self.kmsg_log_level.and_then(|level| {
                level
                    .parse()
                    .map_err(|e| {
                        eprintln!("Invalid kmsg log level format {:?}", e);
                        e
                    })
                    .ok()
            }) {
                kmsg_filter = target;
            };
            let kmsg_fmt_layer = tracing_subscriber::fmt::layer()
                .with_target(false)
                .event_format(kmsg_formatter)
                .with_writer(|| {
                    KmsgLogFormatter::get_writer()
                        .expect("Failed to get writer from TerminalLogFormatter")
                })
                .with_filter(kmsg_filter);

            tracing::subscriber::set_global_default(
                tracing_subscriber::registry()
                    .with(terminal_fmt_layer)
                    .with(kmsg_fmt_layer),
            )
            .with_context(|| "set_global_default for kmsg failed.")?;
            tracing_log::LogTracer::init().with_context(|| {
                "Failed to init LogTracer with terminal logger and kmsg logger."
            })?;

            Ok(())
        };
        inner()
            .with_context(|| "Failed to init the logger")
            .unwrap();
    }
}

pub fn init_logger(app_name: String, log_level: Option<String>) {
    let mut logger_initializer = LoggerInitializer::default();
    if let Some(log_level) = log_level {
        logger_initializer.with_log_level(log_level);
    }
    logger_initializer.init(app_name);
}

#[derive(Clone, Debug)]
struct TerminalLogFormatter {
    app_name: String,
}

impl TerminalLogFormatter {
    fn new(app_name: String) -> TerminalLogFormatter {
        #[cfg(target_os = "windows")]
        {
            if let Err(e) = ansi_term::enable_ansi_support() {
                eprintln!("Warn: ansi_term::enable_ansi_support failed. {:?}", e);
            }
        }
        TerminalLogFormatter { app_name }
    }
}

impl<S, N> FormatEvent<S, N> for TerminalLogFormatter
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let level = *event.metadata().level();
        write!(
            writer,
            "{}{} ",
            format!("[{}]", &self.app_name).bright_green(),
            match level {
                tracing::Level::INFO => "".to_string(),
                tracing::Level::ERROR | tracing::Level::WARN =>
                    format!("[{}]", level).red().to_string(),
                _ => format!("[{}]", level).bright_green().to_string(),
            }
        )?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)?;
        Ok(())
    }
}

#[derive(Debug)]
struct KmsgLogFormatter {
    app_name: String,
}

impl KmsgLogFormatter {
    pub fn new(app_name: String) -> KmsgLogFormatter {
        KmsgLogFormatter { app_name }
    }

    #[cfg(target_os = "linux")]
    fn get_writer() -> Result<Box<dyn Write>> {
        if nix::unistd::getegid().as_raw() == 0 {
            // Rust APIs set CLOEXEC by default
            Ok(Box::new(
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open("/dev/kmsg")
                    .with_context(|| "Failed to open /dev/kmsg")?,
            ))
        } else {
            Ok(Box::new(std::io::sink()))
        }
    }

    #[cfg(target_os = "windows")]
    fn get_writer() -> Result<Box<dyn Write>> {
        Ok(Box::new(std::io::sink()))
    }
}

impl<S, N> FormatEvent<S, N> for KmsgLogFormatter
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let level = *event.metadata().level();
        write!(writer.by_ref(), "{}: [{}] ", self.app_name, &level)?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer.by_ref())?;
        Ok(())
    }
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

pub fn build_progress_bar(total_size: u64) -> indicatif::ProgressBar {
    let bar = indicatif::ProgressBar::new(total_size);
    bar.set_style(indicatif::ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
                    .progress_chars("#>-"));
    bar
}
