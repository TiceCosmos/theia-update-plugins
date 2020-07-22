use easy_http_request::DefaultHttpRequest;
use log::debug;
use std::num::ParseIntError;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{fmt, fs, io};
use zip::ZipArchive;

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
    type Err = ParseIntError;

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
    pub fn get_last_version<T: AsRef<str>>(&self, path: T) -> Result<(Version, String), String> {
        self.api.get_last_version(path)
    }
    pub fn get_install_info<T: AsRef<Path>>(&self, name: T) -> Result<Option<Version>, String> {
        let content = match fs::read_to_string(self.dir.join(name).join("extension.vsixmanifest")) {
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
    pub fn upgrade<P: AsRef<Path>, S: AsRef<str>>(&self, name: P, url: S) -> Result<(), String> {
        let url = url.as_ref();
        let target = self.dir.join(name);

        let file = Self::download(url).map_err(|e| format!("{}, download: {}", url, e))?;
        let file = io::Cursor::new(file);

        let mut archive = ZipArchive::new(file).map_err(|e| format!("{}, read file: {}", url, e))?;
        for i in 0..archive.len() {
            if let Ok(mut file) = archive.by_index(i) {
                if file.is_file() {
                    let file_path = target.join(file.name());
                    // Create parent dir
                    let file_dir = file_path.parent().ok_or(format!("{}, not get directory", url))?;
                    fs::create_dir_all(&file_dir).map_err(|e| format!("{}, create dir: {}", url, e))?;
                    // Write file
                    let mut outfile =
                        fs::File::create(&file_path).map_err(|e| format!("{}, write file: {}", url, e))?;
                    io::copy(&mut file, &mut outfile).map_err(|e| format!("{}, write file: {}", url, e))?;
                    // Get and Set permissions
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Some(mode) = file.unix_mode() {
                            fs::set_permissions(&file_path, fs::Permissions::from_mode(mode))
                                .map_err(|e| format!("{}, set permission: {}", url, e))?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn download<T: AsRef<str>>(url: T) -> Result<Vec<u8>, String> {
        let url = url.as_ref();
        let request = DefaultHttpRequest::get_from_url_str(url).map_err(|e| e.to_string())?;
        let response = request.send().map_err(|e| e.to_string())?;
        if response.status_code != 200 {
            return Err(format!("status_code: {:?}", response.status_code));
        }
        let content_type = response.headers.get("content-type").ok_or("content-type: not find")?;
        if content_type == "application/octet-stream" {
            Ok(response.body)
        } else {
            Err(format!("content-type: {}", content_type))
        }
    }
}

/// Theia plugins HTTP API
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
    fn get_last_version<T: AsRef<str>>(&self, path: T) -> Result<(Version, String), String> {
        let path = self.prefix.clone() + path.as_ref();
        let request =
            DefaultHttpRequest::get_from_url_str(&path).map_err(|e| format!("{}, {}", path, e.to_string()))?;
        let response = request.send().map_err(|e| format!("{}, {}", path, e.to_string()))?;
        if response.status_code != 200 {
            return Err(format!("{}, status_code: {:?}", path, response.status_code));
        }
        let content_type = response
            .headers
            .get("content-type")
            .ok_or(format!("{}, content-type: not find", path))?;
        match content_type.as_ref() {
            "application/json" => self.parse_json(response.body).map_err(|e| format!("path, body: {}", e)),
            _ => Err(format!("{}, content-type: {}", path, content_type)),
        }
    }
    fn parse_json(&self, body: Vec<u8>) -> Result<(Version, String), String> {
        let body = String::from_utf8(body).map_err(|e| e.to_string())?;
        let json = body.parse::<serde_json::Value>().map_err(|e| e.to_string())?;
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
        let version = version.as_str().ok_or("not find version")?;
        let version = version.parse().map_err(|e| format!("not parse version, {}", e))?;
        let download = download.as_str().ok_or("not find download")?;
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
