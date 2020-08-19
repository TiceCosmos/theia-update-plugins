mod lib;
use lib::*;

use async_std::task;
use env_logger::Builder;
use futures::future::join_all;
use log::{info, warn};
use std::io::Write;
use std::path::PathBuf;
use std::{env, fs};
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt()]
struct Opt {
    /// plugins config
    #[structopt(short = "c", long = "config", default_value = "$HOME/.theia/plugins.toml")]
    config: PathBuf,
    /// theia config dir
    #[structopt(short = "t", long = "target", default_value = "$HOME/.theia/plugins")]
    target: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Builder::from_default_env()
        .format(|buf, record| {
            writeln!(
                buf,
                "{} [{}] - {}",
                chrono::Local::now().format("%FT%T"),
                buf.default_styled_level(record.level()),
                record.args()
            )
        })
        .init();

    let theia_root = PathBuf::from(env::var("HOME")?).join(".theia");

    let mut opt = Opt::from_args();
    if opt.config == PathBuf::from("$HOME/.theia/plugins.toml") {
        opt.config = theia_root.join("plugins.toml");
    }
    if opt.target == PathBuf::from("$HOME/.theia/plugins") {
        opt.target = theia_root.join("plugins");
    }
    info!("{:#?}", opt);

    // Get plugins configuration information
    let config = fs::read_to_string(opt.config)?.parse::<toml::Value>()?;
    let config = match config.as_table() {
        Some(x) => x,
        None => return Ok(()),
    };

    for (domain, table) in config {
        let plugin = match (
            table.get("prefix").and_then(|x| x.as_str()),
            table.get("version").and_then(|x| x.as_str()),
            table.get("download").and_then(|x| x.as_str()),
        ) {
            (Some(prefix), Some(version), Some(download)) => TheiaPlugin::new(prefix, version, download, &opt.target),
            _ => {
                warn!("{}: missing information", domain);
                continue;
            }
        };

        let null_table = toml::value::Table::new();
        let download_list = table.get("list").and_then(|x| x.as_table()).unwrap_or(&null_table);

        let mut future_list = vec![];
        for (name, path) in download_list
            .into_iter()
            .filter_map(|(name, path)| path.as_str().map(|path| (name, path)))
        {
            future_list.push(task::spawn(upgrade(
                plugin.clone(),
                format!("from {} get {}", domain, name),
                name.to_owned(),
                path.to_owned(),
            )));
        }
        task::block_on(async {
            join_all(future_list).await;
        })
    }
    Ok(())
}

async fn upgrade(plugin: TheiaPlugin, prefix: String, name: String, path: String) {
    let version_old = match plugin.get_install_info(&name) {
        Ok(x) => x,
        Err(e) => {
            warn!("{}, {}", prefix, e);
            return;
        }
    };
    let (version_new, download) = match plugin.get_last_version(path).await {
        Ok(x) => x,
        Err(e) => {
            warn!("{}, {}", prefix, e);
            return;
        }
    };
    match version_old {
        Some(version_old) if version_old == version_new => {
            info!("{}, latest is {}", prefix, version_new);
            return;
        }
        Some(version_old) => info!("{}, upgrade {} to {}", prefix, version_old, version_new),
        None => info!("{}, install {}", prefix, version_new),
    }
    info!("download form {}", download);
    if let Err(e) = plugin.upgrade(name, download).await {
        warn!("{}", e);
    }
}
