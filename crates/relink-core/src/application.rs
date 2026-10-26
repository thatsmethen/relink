//! Stable application selection, separate from a currently running process.
//!
//! Native discovery must verify these identities and resolve fresh process
//! instances at activation. Lexical path validation does not verify a file,
//! signature, permission, application membership, or process lifetime.
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

const PREFIX: &str = "relink-app-v1:";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplicationSelector {
    MacBundle { bundle_id: String, path: String },
    WindowsExecutable { path: String },
}

#[derive(Debug)]
pub enum ApplicationSelectorError {
    InvalidPrefix,
    UnsupportedVersion(String),
    InvalidJson(serde_json::Error),
    EmptyField(&'static str),
    NulField(&'static str),
    InvalidBundleId,
    InvalidPath {
        platform: &'static str,
        reason: &'static str,
    },
}

impl fmt::Display for ApplicationSelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrefix => write!(f, "application identity must begin with {PREFIX}"),
            Self::UnsupportedVersion(version) => write!(f, "unsupported application identity version: {version}"),
            Self::InvalidJson(error) => write!(f, "invalid application identity: {error}"),
            Self::EmptyField(field) => write!(f, "application identity {field} is empty"),
            Self::NulField(field) => write!(f, "application identity {field} contains NUL"),
            Self::InvalidBundleId => f.write_str("bundle identity must contain nonempty ASCII letters, digits, hyphens or dot-separated components"),
            Self::InvalidPath { platform, reason } => write!(f, "invalid {platform} application path: {reason}"),
        }
    }
}

impl Error for ApplicationSelectorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidJson(error) => Some(error),
            _ => None,
        }
    }
}

impl ApplicationSelector {
    /// Validate portable identity syntax without touching the filesystem or OS.
    pub fn validate(&self) -> Result<(), ApplicationSelectorError> {
        match self {
            Self::MacBundle { bundle_id, path } => {
                text(bundle_id, "bundle_id")?;
                if !bundle_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
                    || bundle_id.split('.').any(str::is_empty)
                {
                    return Err(ApplicationSelectorError::InvalidBundleId);
                }
                text(path, "path")?;
                if !path.starts_with('/') || path.starts_with("//") {
                    return Err(path_error("macOS", "expected an absolute POSIX path"));
                }
                if path[1..].split('/').any(invalid_component) {
                    return Err(path_error("macOS", "empty or relative path component"));
                }
            }
            Self::WindowsExecutable { path } => validate_windows_path(path)?,
        }
        Ok(())
    }

    /// Persist this selection in `InputKind::Application::application_id`.
    /// PIDs, process handles and HAL object IDs are intentionally absent.
    pub fn encode(&self) -> Result<String, ApplicationSelectorError> {
        self.validate()?;
        let json = serde_json::to_string(self).map_err(ApplicationSelectorError::InvalidJson)?;
        Ok(format!("{PREFIX}{json}"))
    }

    pub fn decode(encoded: &str) -> Result<Self, ApplicationSelectorError> {
        let Some(json) = encoded.strip_prefix(PREFIX) else {
            return Err(if let Some((version, _)) = encoded.split_once(':') {
                if version.starts_with("relink-app-") {
                    ApplicationSelectorError::UnsupportedVersion(version.into())
                } else {
                    ApplicationSelectorError::InvalidPrefix
                }
            } else {
                ApplicationSelectorError::InvalidPrefix
            });
        };
        let selector: Self =
            serde_json::from_str(json).map_err(ApplicationSelectorError::InvalidJson)?;
        selector.validate()?;
        Ok(selector)
    }
}

fn text(value: &str, field: &'static str) -> Result<(), ApplicationSelectorError> {
    if value.is_empty() {
        Err(ApplicationSelectorError::EmptyField(field))
    } else if value.contains('\0') {
        Err(ApplicationSelectorError::NulField(field))
    } else {
        Ok(())
    }
}

fn path_error(platform: &'static str, reason: &'static str) -> ApplicationSelectorError {
    ApplicationSelectorError::InvalidPath { platform, reason }
}

fn invalid_component(component: &str) -> bool {
    component.is_empty() || matches!(component, "." | "..")
}

fn validate_windows_path(path: &str) -> Result<(), ApplicationSelectorError> {
    text(path, "path")?;
    // This intentionally does not use std::path: selectors must round-trip on
    // the other platform too. Native discovery supplies a verified full path.
    let ordinary = path.strip_prefix(r"\\?\").unwrap_or(path);
    let (tail, minimum_components) = if let Some(unc) = ordinary.strip_prefix(r"UNC\") {
        if ordinary.len() == path.len() {
            return Err(path_error(
                "Windows",
                "UNC prefix requires an extended absolute path",
            ));
        }
        (unc, 3)
    } else if let Some(unc) = ordinary.strip_prefix(r"\\") {
        (unc, 3)
    } else {
        let bytes = ordinary.as_bytes();
        if bytes.len() < 4 || !bytes[0].is_ascii_alphabetic() || bytes[1..3] != *b":\\" {
            return Err(path_error(
                "Windows",
                "expected an absolute drive or UNC executable path",
            ));
        }
        (&ordinary[3..], 1)
    };
    let mut count = 0;
    for component in tail.split('\\') {
        if invalid_component(component)
            || component.chars().any(|c| {
                c.is_control() || matches!(c, '/' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
            })
        {
            return Err(path_error("Windows", "invalid or relative path component"));
        }
        count += 1;
    }
    if count < minimum_components {
        return Err(path_error(
            "Windows",
            "UNC executable path requires server, share and file",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_selectors_round_trip_across_platforms_without_runtime_process_identity() {
        let selectors = [
            ApplicationSelector::MacBundle {
                bundle_id: "com.vendor.Editor-2".into(),
                path: "/Applications/Editor 2.app".into(),
            },
            ApplicationSelector::WindowsExecutable {
                path: r"C:\Program Files\Editor\editor.exe".into(),
            },
            ApplicationSelector::WindowsExecutable {
                path: r"\\server\share\Editor\editor.exe".into(),
            },
            ApplicationSelector::WindowsExecutable {
                path: r"\\?\C:\Editor\editor.exe".into(),
            },
            ApplicationSelector::WindowsExecutable {
                path: r"\\?\UNC\server\share\editor.exe".into(),
            },
        ];
        for selector in selectors {
            let encoded = selector.encode().unwrap();
            assert_eq!(ApplicationSelector::decode(&encoded).unwrap(), selector);
            assert_eq!(
                ApplicationSelector::decode(&encoded)
                    .unwrap()
                    .encode()
                    .unwrap(),
                encoded
            );
            assert!(!encoded.contains("pid") && !encoded.contains("process_id"));
        }
    }

    #[test]
    fn selectors_reject_unknown_versions_platforms_fields_and_pid_only_records() {
        for encoded in [
            r#"relink-app-v2:{"type":"windows_executable","path":"C:\\editor.exe"}"#,
            r#"relink-app-v1:{"type":"linux","path":"/bin/editor"}"#,
            r#"relink-app-v1:{"type":"mac_bundle","bundle_id":"com.editor","path":"/Editor.app","pid":42}"#,
            r#"relink-app-v1:{"type":"windows_executable","pid":42}"#,
            r#"relink-app-v1:{"type":"windows_executable","path":"C:\\a.exe","path":"C:\\b.exe"}"#,
            r#"{"type":"windows_executable","path":"C:\\editor.exe"}"#,
        ] {
            assert!(ApplicationSelector::decode(encoded).is_err(), "{encoded}");
        }
    }

    #[test]
    fn selector_paths_are_absolute_platform_specific_and_without_nul_or_traversal() {
        for path in [
            "",
            "Editor.app",
            "C:\\Editor.app",
            "/",
            "/Applications//Editor.app",
            "/Applications/../Editor.app",
            "/Applications/./Editor.app",
            "/Editor.app/",
            "/Editor\0.app",
        ] {
            assert!(
                ApplicationSelector::MacBundle {
                    bundle_id: "com.editor".into(),
                    path: path.into()
                }
                .encode()
                .is_err(),
                "{path:?}"
            );
        }
        for path in [
            "",
            "/Applications/Editor.app",
            "editor.exe",
            r"C:editor.exe",
            r"\editor.exe",
            r"C:\",
            r"C:\..\editor.exe",
            r"C:\.\editor.exe",
            r"C:\\editor.exe",
            r"\\server\share",
            r"\\.\device\editor.exe",
            r"C:\editor.exe:stream",
            "C:\\editor\0.exe",
        ] {
            assert!(
                ApplicationSelector::WindowsExecutable { path: path.into() }
                    .encode()
                    .is_err(),
                "{path:?}"
            );
        }
        for bundle_id in [
            "",
            "com..editor",
            ".com.editor",
            "com.editor.",
            "com_editor",
            "com.editor\0",
        ] {
            assert!(ApplicationSelector::MacBundle {
                bundle_id: bundle_id.into(),
                path: "/Editor.app".into()
            }
            .encode()
            .is_err());
        }
    }
}
