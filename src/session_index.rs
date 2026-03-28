use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;

use serde::Deserialize;

const READ_CHUNK_SIZE: usize = 8192;

#[derive(Debug, Deserialize)]
struct SessionIndexEntry {
    id: String,
    thread_name: String,
}

pub fn find_thread_names_by_ids(
    path: &Path,
    thread_ids: &HashSet<String>,
) -> anyhow::Result<HashMap<String, String>> {
    if thread_ids.is_empty() || !path.is_file() {
        return Ok(HashMap::new());
    }

    let mut file = File::open(path)?;
    let mut remaining = file.metadata()?.len();
    let mut names = HashMap::with_capacity(thread_ids.len());
    let mut unresolved = thread_ids.clone();
    let mut line_rev = Vec::new();
    let mut buf = vec![0u8; READ_CHUNK_SIZE];

    while remaining > 0 && !unresolved.is_empty() {
        let read_size = usize::try_from(remaining.min(READ_CHUNK_SIZE as u64))?;
        remaining -= read_size as u64;
        file.seek(SeekFrom::Start(remaining))?;
        file.read_exact(&mut buf[..read_size])?;

        for &byte in buf[..read_size].iter().rev() {
            if byte == b'\n' {
                if consume_reversed_line(&mut line_rev, &mut unresolved, &mut names)?
                    && unresolved.is_empty()
                {
                    return Ok(names);
                }
                continue;
            }
            line_rev.push(byte);
        }
    }

    let _ = consume_reversed_line(&mut line_rev, &mut unresolved, &mut names)?;
    Ok(names)
}

fn consume_reversed_line(
    line_rev: &mut Vec<u8>,
    unresolved: &mut HashSet<String>,
    names: &mut HashMap<String, String>,
) -> anyhow::Result<bool> {
    if line_rev.is_empty() {
        return Ok(false);
    }

    line_rev.reverse();
    let line = std::mem::take(line_rev);
    let Ok(mut line) = String::from_utf8(line) else {
        return Ok(false);
    };
    if line.ends_with('\r') {
        line.pop();
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }

    let Ok(entry) = serde_json::from_str::<SessionIndexEntry>(trimmed) else {
        return Ok(false);
    };
    let thread_name = entry.thread_name.trim();
    if thread_name.is_empty() || !unresolved.contains(&entry.id) {
        return Ok(false);
    }

    names.insert(entry.id.clone(), thread_name.to_string());
    unresolved.remove(&entry.id);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn find_thread_names_prefers_latest_non_empty_entry() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session_index.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"id\":\"thread-1\",\"thread_name\":\"first\",\"updated_at\":\"1\"}\n",
                "{\"id\":\"thread-2\",\"thread_name\":\"other\",\"updated_at\":\"2\"}\n",
                "{\"id\":\"thread-1\",\"thread_name\":\"latest\",\"updated_at\":\"3\"}\n"
            ),
        )
        .expect("write index");

        let names = find_thread_names_by_ids(&path, &HashSet::from(["thread-1".to_string()]))
            .expect("load names");

        assert_eq!(
            names,
            HashMap::from([("thread-1".to_string(), "latest".to_string())])
        );
    }

    #[test]
    fn find_thread_names_skips_empty_latest_entries() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session_index.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"id\":\"thread-1\",\"thread_name\":\"older\",\"updated_at\":\"1\"}\n",
                "{\"id\":\"thread-1\",\"thread_name\":\"\",\"updated_at\":\"2\"}\n"
            ),
        )
        .expect("write index");

        let names = find_thread_names_by_ids(&path, &HashSet::from(["thread-1".to_string()]))
            .expect("load names");

        assert_eq!(
            names,
            HashMap::from([("thread-1".to_string(), "older".to_string())])
        );
    }
}
