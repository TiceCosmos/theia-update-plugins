mod lib;
use lib::*;

use async_std::task;
use chrono::Local;
use env_logger::Builder;
use futures::future;
use log::{debug, info, warn};
use std::{
    error,
    io::Write,
    path::PathBuf,
    {env, fs},
};
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

fn main() -> Result<(), Box<dyn error::Error>> {
    Builder::from_default_env()
        .format(|buf, record| {
            writeln!(
                buf,
                "{} [{}] - {}",
                Local::now().format("%FT%T"),
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

    let mut future_list = vec![];

    for (domain, table) in config {
        let plugin = match (
            table.get("regular").and_then(|x| x.as_str()),
            table.get("version").and_then(|x| x.as_str()),
            table.get("download").and_then(|x| x.as_str()),
        ) {
            (Some(regular), Some(version), Some(download)) => TheiaPlugin::new(regular, version, download, &opt.target),
            _ => {
                warn!("{}: missing information", domain);
                continue;
            }
        };

        let null_table = toml::value::Table::new();
        let download_list = table.get("list").and_then(|x| x.as_table()).unwrap_or(&null_table);

        for (name, path) in download_list
            .into_iter()
            .filter_map(|(name, path)| path.as_str().map(|path| (name.to_owned(), path.to_owned())))
        {
            future_list.push(task::spawn(upgrade(plugin.clone(), name, path)));
        }
    }

    task::block_on(async {
        for warn in future::join_all(future_list).await {
            if let Err(warn) = warn {
                warn!("{}", warn);
            }
        }
    });

    Ok(())
}

async fn upgrade(plugin: TheiaPlugin, name: String, path: String) -> Result<(), String> {
    let prefix = format!("{}: ", name);

    let (version_old, version_new) = future::join(plugin.get_install_info(&name), plugin.get_last_version(path)).await;

    let version_old = version_old.map_err(|e| prefix.clone() + &e)?;
    let (version_new, download) = version_new.map_err(|e| prefix.clone() + &e)?;

    if version_old.as_ref() == Some(&version_new) {
        debug!("{}latest {} is installed", prefix, version_new);
        return Ok(());
    }

    info!(
        "{}{} {} from {}",
        prefix,
        version_old.map_or("install".to_owned(), |x| format!("upgrade {} to", x)),
        version_new,
        download
    );

    plugin.upgrade(name, download).await.map_err(|e| prefix.clone() + &e)
}
