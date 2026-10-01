// Thin wrapper around the `keepassxc-cli` binary.
// v1 shells out to the CLI and parses its XML export, rather than linking to a
// KeePass library or parsing the .kdbx file directly to keep the first version simple.

use quick_xml::escape::unescape;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Stable id from the export. Used as the list key; the CLI itself can't address entries by it.
    pub uuid: String,
    pub path: String,
    pub title: String,
    pub username: String,
    pub url: String,
    pub has_totp: bool,
    /// Custom icon (e.g. a favicon KeePassXC downloaded) as a PNG data URL, if the entry has one.
    pub icon: Option<String>,
    /// 0 for the first entry with this path, 1+ for later entries sharing it. `keepassxc-cli`
    /// addresses entries by path only, so it can only ever reach the first one.
    pub dup_index: u32,
}

/// Where KeePassXC usually puts its CLI. A Finder-launched app only gets a minimal
/// PATH (no Homebrew), so we look in these places before falling back to PATH.
const CLI_CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/keepassxc-cli",
    "/usr/local/bin/keepassxc-cli",
    "/Applications/KeePassXC.app/Contents/MacOS/keepassxc-cli",
];

fn cli_path() -> &'static str {
    static PATH: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        CLI_CANDIDATES
            .iter()
            .copied()
            .find(|p| std::path::Path::new(p).exists())
            .unwrap_or("keepassxc-cli")
    })
}

fn run_cli(args: &[&str], stdin_data: &str, db_path: &str) -> Result<String, String> {
    if !std::path::Path::new(db_path).exists() {
        return Err(format!("Database not found at {db_path}"));
    }

    let mut child = Command::new(cli_path())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "keepassxc-cli not found. Please install KeePassXC.".to_string())?;

    child
        .stdin
        .as_mut()
        .ok_or("failed to open stdin")?
        .write_all(format!("{stdin_data}\n").as_bytes())
        .map_err(|e| e.to_string())?;

    let output = child.wait_with_output().map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err("Invalid master password".to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Verifies the master password against the database and loads every entry's
/// searchable metadata in a single decrypt (via `export`), instead of paying
/// the KDF cost (deliberately ~1s by design) once per entry.
pub fn unlock_and_load(db_path: &str, password: &str) -> Result<Vec<Entry>, String> {
    let xml = run_cli(&["export", "-q", "-f", "xml", db_path], password, db_path)?;
    parse_entries(&xml)
}

/// Fetches a plain (non-protected-in-memory) attribute's value, e.g. "UserName".
/// Note: `keepassxc-cli clip` also does this, but it blocks for the *entire*
/// clipboard-timeout duration before returning (it manages its own timed
/// clear internally) using `show` instead keeps this fast, so the caller
/// can write to the clipboard and clear it on its own schedule.
pub fn fetch_attribute(db_path: &str, password: &str, entry_path: &str, attribute: &str) -> Result<String, String> {
    let out = run_cli(&["show", "-q", "-s", "-a", attribute, db_path, entry_path], password, db_path)?;
    Ok(out.trim_end_matches('\n').to_string())
}

/// Fetches the entry's current TOTP code.
pub fn fetch_totp(db_path: &str, password: &str, entry_path: &str) -> Result<String, String> {
    let out = run_cli(&["show", "-q", "-t", db_path, entry_path], password, db_path)?;
    Ok(out.trim_end_matches('\n').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed shape of a real `keepassxc-cli export -f xml` output: a
    // top-level entry, a nested group, an entry with TOTP (redacted/empty
    // value, same as real exports — presence of the field is what matters),
    // and a <History> block (a stale prior revision) that must NOT be
    // parsed as an entry.
    const SAMPLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<KeePassFile>
  <Meta>
    <CustomIcons>
      <Icon><UUID>aWNvbjE=</UUID><Data>iVBORw0KGgo=</Data></Icon>
    </CustomIcons>
  </Meta>
  <Root>
    <Group>
      <UUID>root-uuid</UUID>
      <Name>Root</Name>
      <Notes/>
      <Entry>
        <UUID>e1</UUID>
        <String><Key>Notes</Key><Value/></String>
        <String><Key>Password</Key><Value ProtectInMemory="True"/></String>
        <String><Key>Title</Key><Value>Spotify</Value></String>
        <String><Key>URL</Key><Value/></String>
        <String><Key>UserName</Key><Value>user@spotify.com</Value></String>
        <History>
          <Entry>
            <UUID>e1-old</UUID>
            <String><Key>Title</Key><Value>Spotify Old</Value></String>
          </Entry>
        </History>
      </Entry>
      <Group>
        <UUID>work-uuid</UUID>
        <Name>Work</Name>
        <Entry>
          <UUID>e2</UUID>
          <String><Key>Title</Key><Value>Slack</Value></String>
          <String><Key>UserName</Key><Value>slack@work.com</Value></String>
          <String><Key>otp</Key><Value ProtectInMemory="True"/></String>
        <CustomIconUUID>aWNvbjE=</CustomIconUUID>
        </Entry>
      </Group>
    </Group>
  </Root>
</KeePassFile>"#;

    #[test]
    fn numbers_entries_that_share_a_path() {
        let xml = r#"<KeePassFile><Root><Group><Name>Root</Name>
          <Entry><String><Key>Title</Key><Value>github</Value></String></Entry>
          <Entry><String><Key>Title</Key><Value>github</Value></String></Entry>
        </Group></Root></KeePassFile>"#;
        let entries = parse_entries(xml).unwrap();
        assert_eq!(entries.iter().map(|e| e.dup_index).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn parses_export_into_flat_entries() {
        let entries = parse_entries(SAMPLE_XML).unwrap();

        assert_eq!(entries.len(), 2, "History revisions must not be counted as entries");

        let spotify = entries
            .iter()
            .find(|e| e.path == "Spotify")
            .expect("top-level entry keeps a bare path");
        assert_eq!(spotify.username, "user@spotify.com");
        assert!(!spotify.has_totp);

        let slack = entries
            .iter()
            .find(|e| e.path == "Work/Slack")
            .expect("nested entry is prefixed with its group");
        assert_eq!(slack.username, "slack@work.com");
        assert!(slack.has_totp);

        assert_eq!(slack.icon.as_deref(), Some("data:image/png;base64,iVBORw0KGgo="));
        assert!(spotify.icon.is_none());
        assert!(entries.iter().all(|e| e.dup_index == 0));
        assert_eq!(spotify.uuid, "e1");

        assert!(
            !entries.iter().any(|e| e.title == "Root"),
            "the Root group itself must not become an entry"
        );
        assert!(
            !entries.iter().any(|e| e.title.contains("Old")),
            "entries from <History> must not leak into the results"
        );
    }
}

/// Walks the KDBX XML export (Group/Entry tree) and flattens it into a list
/// of entries with their group path, skipping the Root group name and any
/// <History> (previous revisions of an entry, not real separate entries).
fn parse_entries(xml: &str) -> Result<Vec<Entry>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut entries = Vec::new();
    let mut group_stack: Vec<String> = Vec::new();
    let mut in_root_group = false;
    let mut icons: HashMap<String, String> = HashMap::new();
    let mut entry_icon_ids: Vec<Option<String>> = Vec::new();

    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"Group" => {
                let name = read_child_text(&mut reader, b"Name").unwrap_or_default();
                if !in_root_group && name == "Root" {
                    in_root_group = true;
                } else {
                    group_stack.push(name);
                }
            }
            Event::End(e) if e.name().as_ref() == b"Group" => {
                group_stack.pop();
            }
            Event::Start(e) if e.name().as_ref() == b"History" => {
                skip_element(&mut reader, b"History")?;
            }
            Event::Start(e) if e.name().as_ref() == b"CustomIcons" => {
                parse_custom_icons(&mut reader, &mut icons)?;
            }
            Event::Start(e) if e.name().as_ref() == b"Entry" => {
                let (entry, icon_id) = parse_entry(&mut reader, &group_stack)?;
                entries.push(entry);
                entry_icon_ids.push(icon_id);
            }
            _ => {}
        }
    }

    let mut seen: HashMap<String, u32> = HashMap::new();
    for entry in entries.iter_mut() {
        let n = seen.entry(entry.path.clone()).or_insert(0);
        entry.dup_index = *n;
        *n += 1;
    }

    for (entry, icon_id) in entries.iter_mut().zip(entry_icon_ids) {
        entry.icon = icon_id
            .and_then(|id| icons.get(&id))
            .map(|b64| format!("data:image/png;base64,{b64}"));
    }

    Ok(entries)
}

fn parse_entry(reader: &mut Reader<&[u8]>, group_stack: &[String]) -> Result<(Entry, Option<String>), String> {
    let mut title = String::new();
    let mut username = String::new();
    let mut url = String::new();
    let mut has_totp = false;
    let mut icon_id: Option<String> = None;
    let mut uuid = String::new();

    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Eof => return Err("unexpected end of export".to_string()),
            Event::End(e) if e.name().as_ref() == b"Entry" => break,
            Event::Start(e) if e.name().as_ref() == b"History" => {
                skip_element(reader, b"History")?;
            }
            Event::Start(e) if e.name().as_ref() == b"UUID" => {
                if let Ok(Event::Text(t)) = reader.read_event() {
                    uuid = String::from_utf8_lossy(&t).trim().to_string();
                }
            }
            Event::Start(e) if e.name().as_ref() == b"CustomIconUUID" => {
                if let Some(Event::Text(t)) = reader.read_event().ok() {
                    icon_id = Some(String::from_utf8_lossy(&t).trim().to_string());
                }
            }
            Event::Start(e) if e.name().as_ref() == b"String" => {
                let key = read_child_text(reader, b"Key").unwrap_or_default();
                let value = read_child_text(reader, b"Value").unwrap_or_default();
                match key.as_str() {
                    "Title" => title = value,
                    "UserName" => username = value,
                    "URL" => url = value,
                    // The otp field is protected-in-memory, so its exported value is
                    // always redacted/empty; presence of the field is the real signal.
                    k if k == "otp" || k.starts_with("TOTP") || k.starts_with("TimeOtp") => {
                        has_totp = true;
                    }
                    _ => {}
                }
                skip_element(reader, b"String")?;
            }
            _ => {}
        }
    }

    let mut path_parts = group_stack.to_vec();
    path_parts.push(if title.is_empty() {
        "(untitled)".to_string()
    } else {
        title.clone()
    });

    let entry = Entry {
        uuid,
        path: path_parts.join("/"),
        title: if title.is_empty() { "(untitled)".to_string() } else { title },
        username,
        url,
        has_totp,
        icon: None,
        dup_index: 0,
    };
    Ok((entry, icon_id))
}

/// Reads the `<CustomIcons>` block (UUID -> base64 PNG), which precedes the entries in the export.
fn parse_custom_icons(reader: &mut Reader<&[u8]>, icons: &mut HashMap<String, String>) -> Result<(), String> {
    let (mut uuid, mut data) = (None, None);
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::End(e) if e.name().as_ref() == b"CustomIcons" => return Ok(()),
            Event::Eof => return Ok(()),
            Event::Start(e) if e.name().as_ref() == b"UUID" => {
                if let Ok(Event::Text(t)) = reader.read_event() {
                    uuid = Some(String::from_utf8_lossy(&t).trim().to_string());
                }
            }
            Event::Start(e) if e.name().as_ref() == b"Data" => {
                if let Ok(Event::Text(t)) = reader.read_event() {
                    data = Some(String::from_utf8_lossy(&t).split_whitespace().collect::<String>());
                }
            }
            Event::End(e) if e.name().as_ref() == b"Icon" => {
                if let (Some(u), Some(d)) = (uuid.take(), data.take()) {
                    icons.insert(u, d);
                }
            }
            _ => {}
        }
    }
}

/// Scans forward for the next `<tag>...</tag>` sibling within the current
/// element and returns its text content. Tracks nesting depth so that end
/// tags of *other* sibling elements (e.g. `</UUID>` before `<Name>`) don't
/// get mistaken for the end of our own container.
/// Leaves the reader positioned right after the matched child's end tag.
fn read_child_text(reader: &mut Reader<&[u8]>, tag: &[u8]) -> Option<String> {
    let mut depth = 0i32;
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) if depth == 0 && e.name().as_ref() == tag => {
                return match reader.read_event().ok()? {
                    Event::Text(t) => {
                        let _ = reader.read_event();
                        let raw = String::from_utf8_lossy(&t).to_string();
                        unescape(&raw).ok().map(|s| s.to_string())
                    }
                    Event::End(_) => Some(String::new()),
                    _ => None,
                };
            }
            Event::Empty(e) if depth == 0 && e.name().as_ref() == tag => return Some(String::new()),
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// Consumes and discards everything up to and including the matching end tag.
fn skip_element(reader: &mut Reader<&[u8]>, tag: &[u8]) -> Result<(), String> {
    let mut depth = 1;
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) if e.name().as_ref() == tag => depth += 1,
            Event::End(e) if e.name().as_ref() == tag => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}
