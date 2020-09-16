use async_std::fs;
use log::debug;
use std::{
    fmt, io,
    io::prelude::*,
    path::{Path, PathBuf},
    str::FromStr,
};

/// Theia plugins
#[derive(Clone)]
pub struct TheiaPlugin {
    remote: TheiaPluginAPI,
    native: TheiaPluginLCL,
}
impl TheiaPlugin {
    pub fn new<P: AsRef<Path>, S: AsRef<str>>(
        regular: S,   // http api regular
        version: S,   // find version info from json file
        download: S,  // find download url from json file
        theia_dir: P, // theia plugins dir
    ) -> Self {
        Self {
            remote: TheiaPluginAPI::new(regular, version, download),
            native: TheiaPluginLCL::new(theia_dir),
        }
    }
    /// get installed version
    pub async fn get_install_info<T: AsRef<str>>(&self, name: T) -> Result<Version, String> {
        self.native.get_version(name).await
    }
    /// get lastest version
    pub async fn get_last_version<T: AsRef<str>>(&self, path: T) -> Result<(Version, String), String> {
        self.remote.get_version(path).await
    }
    pub async fn upgrade<T: AsRef<str>>(&self, name: T, url: T) -> Result<(), String> {
        let url = url.as_ref();

        let data = surf::client()
            .with(surf::middleware::Redirect::default())
            .recv_bytes(surf::get(url))
            .await
            .map_err(|e| format!("{}, {}", url, e))?;

        self.native.installing(name, &data)
    }
}

/// Theia plugins HTTP API
#[derive(Clone)]
struct TheiaPluginAPI {
    prefix: String,
    suffix: String,
    version: Vec<String>,
    download: Vec<String>,
}
impl TheiaPluginAPI {
    fn new<T: AsRef<str>>(regular: T, version: T, download: T) -> Self {
        let mut split = regular.as_ref().splitn(2, "$$");
        Self {
            prefix: split.next().unwrap_or_default().to_owned(),
            suffix: split.next().unwrap_or_default().to_owned(),
            version: version.as_ref().split('.').map(|x| x.into()).collect(),
            download: download.as_ref().split('.').map(|x| x.into()).collect(),
        }
    }
    async fn get_version<T: AsRef<str>>(&self, name: T) -> Result<(Version, String), String> {
        let url = format!("{}{}{}", self.prefix, name.as_ref(), self.suffix);
        surf::get(&url)
            .recv_bytes()
            .await
            .map_err(|e| e.to_string())
            .and_then(|request| self.parse_json(&request))
            .map_err(|e| format!("{}: {}", url, e))
    }
    fn parse_json(&self, body: &[u8]) -> Result<(Version, String), String> {
        let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let version = self.search_version(&json).ok_or("not find version")?;
        let version = version.parse().map_err(|e| format!("version error, {}", e))?;
        let download = self.search_download(&json).ok_or("not find download")?;
        Ok((version, download.to_owned()))
    }
    fn search_version<'t>(&self, json: &'t serde_json::Value) -> Option<&'t str> {
        let mut version = json;
        for item in self.version.iter() {
            version = version.get(item)?;
            if version.is_array() {
                version = version.get(0)?;
            }
        }
        version.as_str()
    }
    fn search_download<'t>(&self, json: &'t serde_json::Value) -> Option<&'t str> {
        let mut download = json;
        for item in self.download.iter() {
            download = download.get(item)?;
            if download.is_array() {
                download = download.get(0)?;
            }
        }
        download.as_str()
    }
}

#[derive(Clone)]
struct TheiaPluginLCL {
    directory: PathBuf,
}
impl TheiaPluginLCL {
    fn new<T: AsRef<Path>>(directory: T) -> Self {
        Self {
            directory: directory.as_ref().into(),
        }
    }
    pub async fn get_version<T: AsRef<str>>(&self, name: T) -> Result<Version, String> {
        let path = self.directory.join(name.as_ref()).join("extension.vsixmanifest");
        fs::read_to_string(&path)
            .await
            .map_err(|e| format!("read vsixmanifest, {:?}", e))
            .and_then(|content| {
                let reader = quick_xml::Reader::from_str(&content);
                self.search_version(reader).ok_or_else(|| "not find version".into())
            })
            .and_then(|version| version.parse().map_err(|e| format!("version error, {}", e)))
            .map_err(|e| format!("{:?}: {}", path, e))
    }
    fn search_version<B: BufRead>(&self, mut reader: quick_xml::Reader<B>) -> Option<String> {
        let mut buffer = Vec::new();
        loop {
            let read_event = reader.read_event(&mut buffer).ok()?;
            match read_event {
                quick_xml::events::Event::Eof => break,
                quick_xml::events::Event::Empty(e) if e.name() == b"Identity" => {
                    debug!("extension.vsixmanifest: {:?}", e);
                    for attribute in e.attributes().filter_map(|x| x.ok()) {
                        if attribute.key == b"Version" {
                            return String::from_utf8(attribute.value.into()).ok();
                        }
                    }
                    break;
                }
                _ => (),
            }
            buffer.clear();
        }
        None
    }
    /// decompress from bytes::Bytes, create or rewrite file in target
    fn installing<T: AsRef<str>>(&self, name: T, data: &[u8]) -> Result<(), String> {
        use zip::ZipArchive;

        let target = self.directory.join(name.as_ref());
        let reader = io::Cursor::new(data);

        ZipArchive::new(reader)
            .map_err(|e| format!("read zip archive, {}", e))
            .and_then(|archive| self.savefile(archive, &target))
            .map_err(|e| format!("{:?}: {}", target, e))
    }
    fn savefile<Z: Read + Seek, T: AsRef<Path>>(
        &self,
        mut archive: zip::ZipArchive<Z>,
        target: T,
    ) -> Result<(), String> {
        let target = target.as_ref();
        for i in 0..archive.len() {
            if let Ok(mut file) = archive.by_index(i) {
                if file.is_file() {
                    let file_path = target.join(file.name());
                    // Create parent dir
                    file_path.parent().and_then(|x| std::fs::create_dir_all(x).ok());
                    // Write file
                    let mut outfile = std::fs::File::create(&file_path).map_err(|e| format!("create file, {}", e))?;
                    io::copy(&mut file, &mut outfile).map_err(|e| format!("write file, {}", e))?;
                    // Get and Set permissions
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Some(mode) = file.unix_mode() {
                            std::fs::set_permissions(&file_path, fs::Permissions::from_mode(mode))
                                .map_err(|e| format!("set permission, {}", e))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
impl FromStr for Version {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut iter = s.split('.');
        let (major, minor, patch) = match (iter.next(), iter.next(), iter.next()) {
            (Some(major), Some(minor), Some(patch)) => {
                let major = major.chars().filter(|x| x.is_ascii_digit()).collect::<String>();
                (major.parse()?, minor.parse()?, patch.parse()?)
            }
            _ => (0, 0, 0),
        };
        Ok(Self { major, minor, patch })
    }
}

/// test
#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn version_cmp() {
        let mut last = Version::from_str("0.0.0").unwrap();
        assert_eq!(last, last);
        for next in vec![
            Version::from_str("0.0.1").unwrap(),
            Version::from_str("0.1.0").unwrap(),
            Version::from_str("1.0.0").unwrap(),
            Version::from_str("1.1.1").unwrap(),
        ] {
            assert!(last < next);
            last = next;
        }
    }
}
