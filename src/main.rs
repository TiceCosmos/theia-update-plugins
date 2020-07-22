mod lib;
use lib::*;

use log::{info, warn};
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
    env_logger::init();

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

        let download_plugin = |name, download| {
            info!("download form {}", download);
            if let Err(e) = plugin.upgrade(name, download) {
                warn!("{}", e);
            }
        };

        let null_table = toml::value::Table::new();
        let download_list = table.get("list").and_then(|x| x.as_table()).unwrap_or(&null_table);

        for (name, path) in download_list
            .into_iter()
            .filter_map(|(name, path)| path.as_str().map(|path| (name, path)))
        {
            let prefix = format!("from {} get {}", domain, name);
            match (plugin.get_install_info(name), plugin.get_last_version(path)) {
                (Ok(Some(version_old)), Ok((version_new, _))) if version_old == version_new => {
                    info!("{}, latest is {}", prefix, version_new);
                }
                (Ok(Some(version_old)), Ok((version_new, download))) => {
                    info!("{}, upgrade {} to {}", prefix, version_old, version_new);
                    download_plugin(name, download);
                }
                (Ok(None), Ok((version_new, download))) => {
                    info!("{}, install {}", prefix, version_new);
                    download_plugin(name, download);
                }
                (Err(e), _) | (_, Err(e)) => warn!("{}, {}", prefix, e),
            }
        }
    }

    Ok(())
}
