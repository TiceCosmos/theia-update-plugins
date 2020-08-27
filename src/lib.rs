use async_std::fs;
use log::debug;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Version {
    major: usize,
    minor: usize,
    patch: usize,
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
            (Some(major), Some(minor), Some(patch)) => (major.parse()?, minor.parse()?, patch.parse()?),
            _ => (0, 0, 0),
        };
        Ok(Self { major, minor, patch })
    }
}

/// Theia plugins
#[derive(Clone)]
pub struct TheiaPlugin {
    api: TheiaPluginAPI,
    dir: PathBuf,
}
impl TheiaPlugin {
    pub fn new<P: AsRef<Path>, S: AsRef<str>>(
        prefix: S,    // http api prefix
        version: S,   // find version info from json file
        download: S,  // find download url from json file
        theia_dir: P, // theia plugins dir
    ) -> Self {
        Self {
            api: TheiaPluginAPI::new(prefix, version, download),
            dir: theia_dir.as_ref().into(),
        }
    }
    /// get lastest version from http url
    pub async fn get_last_version<T: AsRef<str>>(&self, path: T) -> Result<(Version, String), String> {
        self.api.last_version(path).await
    }
    /// get version information in extension.vsixmanifest
    pub async fn get_install_info<T: AsRef<Path>>(&self, name: T) -> Result<Option<Version>, String> {
        let content = match fs::read_to_string(self.dir.join(&name).join("extension.vsixmanifest")).await {
            Ok(x) => x,
            Err(_) => return Ok(None),
        };
        let mut reader = quick_xml::Reader::from_str(&content);
        let mut buffer = Vec::new();
        loop {
            let read_event = reader
                .read_event(&mut buffer)
                .map_err(|e| format!("extension.vsixmanifest: error at {}, {:?}", reader.buffer_position(), e))?;
            match read_event {
                quick_xml::events::Event::Eof => break,
                quick_xml::events::Event::Empty(e) if e.name() == b"Identity" => {
                    debug!("extension.vsixmanifest: {:?}", e);
                    for attribute in e.attributes().filter_map(|x| x.ok()) {
                        if attribute.key == b"Version" {
                            return String::from_utf8(attribute.value.into())
                                .unwrap()
                                .parse::<Version>()
                                .map(Some)
                                .map_err(|e| format!("extension.vsixmanifest: parse version, {}", e));
                        }
                    }
                    break;
                }
                _ => (),
            }
            buffer.clear();
        }
        Err("extension.vsixmanifest: not find Identity.Version".into())
    }
    pub async fn upgrade<P: AsRef<Path>, S: AsRef<str>>(&self, name: P, url: S) -> Result<(), String> {
        let url = url.as_ref();
        let target = self.dir.join(name);

        let data = surf::get(url)
            .recv_bytes()
            .await
            .map_err(|e| format!("{}, {}", url, e))?;

        Self::decompress(target, &data)
    }
    /// decompress from bytes::Bytes, create or rewrite file in target
    fn decompress<T: AsRef<Path>>(target: T, data: &[u8]) -> Result<(), String> {
        use zip::ZipArchive;

        let target = target.as_ref();
        let reader = std::io::Cursor::new(data);

        let mut archive = ZipArchive::new(reader).map_err(|e| format!("read zip archive: {}", e))?;

        for i in 0..archive.len() {
            if let Ok(mut file) = archive.by_index(i) {
                if file.is_file() {
                    let file_path = target.join(file.name());
                    // Create parent dir
                    let file_dir = file_path.parent().ok_or("not parent directory")?;
                    std::fs::create_dir_all(&file_dir).map_err(|e| format!("create dir: {}", e))?;
                    // Write file
                    let mut outfile = std::fs::File::create(&file_path)
                        // .await
                        .map_err(|e| format!("write file: {}", e))?;
                    std::io::copy(&mut file, &mut outfile).map_err(|e| format!("write file: {}", e))?;
                    // Get and Set permissions
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Some(mode) = file.unix_mode() {
                            std::fs::set_permissions(&file_path, fs::Permissions::from_mode(mode))
                                .map_err(|e| format!("set permission: {}", e))?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Theia plugins HTTP API
#[derive(Clone)]
struct TheiaPluginAPI {
    prefix: String,
    version: Vec<String>,
    download: Vec<String>,
}
impl TheiaPluginAPI {
    fn new<T: AsRef<str>>(prefix: T, version: T, download: T) -> Self {
        Self {
            prefix: prefix.as_ref().to_owned(),
            version: version.as_ref().split('.').map(|x| x.into()).collect(),
            download: download.as_ref().split('.').map(|x| x.into()).collect(),
        }
    }
    async fn last_version<T: AsRef<str>>(&self, path: T) -> Result<(Version, String), String> {
        let url = self.prefix.clone() + path.as_ref();
        let request = surf::get(&url)
            .recv_bytes()
            .await
            .map_err(|e| format!("{}, {}", url, e))?;
        self.parse_json(&request).map_err(|e| format!("{}, {}", url, e))
    }
    fn parse_json(&self, body: &[u8]) -> Result<(Version, String), String> {
        let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let mut version = &json;
        for item in self.version.iter() {
            version = match version.get(item) {
                Some(x) => x,
                None => return Err("not find version".into()),
            };
        }
        let mut download = &json;
        for item in self.download.iter() {
            download = match download.get(item) {
                Some(x) => x,
                None => return Err("not find download".into()),
            };
        }
        let version = version.as_str().ok_or("version error")?;
        let version = version.parse().map_err(|e| format!("version error, {}", e))?;
        let download = download.as_str().ok_or("download error")?;
        Ok((version, download.to_owned()))
    }
}

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
